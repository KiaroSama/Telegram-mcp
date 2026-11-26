mod accept;
mod arguments;
mod calls;

use super::{
    ExportFormat, MAX_BYTES, export,
    history::{SecretHistory, message_id, row},
    write_atomic,
};
use crate::{
    backend::Telegram,
    session::PersistentSession,
    state::{FileAccess, State},
};
use anyhow::{Context, Result, bail, ensure};
use grammers_client::Client;
use grammers_mtsender::SenderPoolFatHandle;
use grammers_session::{
    Session,
    types::{PeerId, PeerInfo},
    updates::MessageBoxes,
};
use grammers_tl_types as tl;
use kiaro_secret_chat::{
    binding::AuthorizationBinding,
    chat::{ChatSnapshot, ChatState},
    custody::{CustodyLimits, IngressLimits},
    error::Error as SecretError,
    ingress::DurableIngress,
    manager::{Event, SendFileOptions, StrongSecretChatManager},
    media::{FileSource, MediaReference},
    storage::Storage,
};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct SecretRuntime {
    manager: StrongSecretChatManager,
    history: Arc<SecretHistory>,
    state: Arc<State>,
    client: Client,
    label: String,
    acceptance: Arc<accept::Acceptance>,
    mutations: Arc<tokio::sync::Mutex<()>>,
}

pub(crate) struct Prepared {
    ingress: Arc<DurableIngress>,
    baseline: Option<Vec<u8>>,
    history: Arc<SecretHistory>,
    state: Arc<State>,
    label: String,
}

pub(crate) fn session_binding(
    user: i64,
    dc: i32,
    auth: &[u8; 256],
) -> Result<AuthorizationBinding> {
    let digest = Zeroizing::new(<[u8; 20]>::from(Sha1::digest(auth)));
    let id = i64::from_le_bytes(digest[12..].try_into()?);
    Ok(AuthorizationBinding::from_session_parts(
        user,
        id,
        dc,
        "telegram-mcp-secret-v1".into(),
    )?)
}

impl SecretRuntime {
    pub(crate) async fn prepare(
        state: Arc<State>,
        label: &str,
        session: &PersistentSession,
    ) -> Result<Option<Prepared>> {
        let directory = super::custody_dir(state.root())?;
        let legacy = super::has_legacy(&directory, label)?;
        let existing = super::plain_file(&directory.join(format!("{label}.v1.enc")))?;
        let owner = super::legacy_owner(&directory, label)?;
        let self_peer = session.peer(PeerId::self_user()).await?;
        let user = match self_peer {
            Some(PeerInfo::User {
                bot: Some(true), ..
            }) => {
                ensure!(
                    !legacy && !existing,
                    "Bot authorization cannot open secret-chat custody"
                );
                return Ok(None);
            }
            Some(PeerInfo::User { id, .. }) => Some(id),
            _ => owner,
        };
        let dc = session.home_dc_id()?;
        let auth = session
            .dc_option(dc)?
            .and_then(|option| option.auth_key)
            .map(Zeroizing::new);
        let (Some(user), Some(auth)) = (user, auth) else {
            ensure!(
                !legacy && !existing,
                "Cannot establish local Telegram authorization for secret-chat custody"
            );
            return Ok(None);
        };
        let binding = session_binding(user, dc, &auth)?;
        let store = super::open_store(state.root(), label, &binding, true)?
            .context("Secret-chat custody store is unavailable")?;
        let snapshot = store
            .load()?
            .context("Secret-chat custody store has no state")?;
        let history = Arc::new(SecretHistory::open(&directory, label, &snapshot.settings)?);
        let limits = IngressLimits::new(4096, MAX_BYTES, 30.0 * 86400.0)?;
        let ingress = DurableIngress::new(store, limits)?;
        let baseline = if ingress.checkpoint().is_ok() {
            None
        } else {
            Some(
                MessageBoxes::load(session.updates_state().await?)
                    .durable_checkpoint()
                    .context("Cannot encode secret-chat update checkpoint")?,
            )
        };
        Ok(Some(Prepared {
            ingress,
            baseline,
            history,
            state,
            label: label.into(),
        }))
    }

    pub fn manager(&self) -> &StrongSecretChatManager {
        &self.manager
    }
    pub fn history(&self) -> &SecretHistory {
        &self.history
    }

    pub(crate) async fn stop(&self) -> Result<()> {
        self.acceptance.retire();
        Ok(self.manager.stop().await?)
    }

    pub async fn key_picture(&self, chat_id: i32) -> Result<Option<PathBuf>> {
        let snapshot = self.status(chat_id)?;
        snapshot
            .key_hash
            .as_deref()
            .map(|hash| {
                export::key_picture(
                    &self.state,
                    &format!("secret-{}-{chat_id}-key", self.label),
                    hash,
                )
            })
            .transpose()
    }

    fn status(&self, id: i32) -> Result<ChatSnapshot> {
        ensure!(
            !self.history.is_forgotten(id)?,
            "Secret chat was forgotten locally"
        );
        Ok(self.manager.status(id)?)
    }

    pub async fn save_media(
        &self,
        chat_id: i32,
        id: i64,
        destination: Option<&Path>,
    ) -> Result<PathBuf> {
        let reference = self.history.media(chat_id, id)?;
        Ok(self.write_media(&reference, id, destination).await?.0)
    }

    async fn write_media(
        &self,
        reference: &MediaReference,
        id: i64,
        destination: Option<&Path>,
    ) -> Result<(PathBuf, bool)> {
        let path = export::output(
            &self.state,
            destination,
            &format!("secret-{}-{id}.bin", reference.chat_id),
        )?;
        ensure!(
            !super::plain_file(&path)?,
            "Secret-chat media destination already exists"
        );
        let bytes = Zeroizing::new(
            self.manager
                .receive_file_with_limit(reference, MAX_BYTES)
                .await?,
        );
        match write_atomic(&path, &bytes, false) {
            Ok(()) => Ok((path, true)),
            Err(error)
                if error
                    .to_string()
                    .contains("Secret-chat file was published but directory durability could not be confirmed") =>
            {
                Ok((path, false))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn export(
        &self,
        chat_id: i32,
        peer_user_id: i64,
        format: ExportFormat,
        destination: Option<&Path>,
        filter: &export::ExportFilter,
        cancel: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<(Vec<PathBuf>, usize)> {
        let archive = self.history.archive_rows(&self.state, chat_id)?;
        ensure!(
            !archive.is_empty(),
            "No auto-saved messages for this secret chat"
        );
        let filtered: Vec<_> = archive
            .into_iter()
            .filter(|(row, _)| filter.matches_row(row))
            .collect();
        if cancel.is_some_and(|rx| *rx.borrow()) {
            return Err(crate::export::jobs::Cancelled.into());
        }
        let (rows, sources): (Vec<_>, Vec<_>) = filtered.into_iter().unzip();
        let exported_count = rows.len();
        let paths = export::write_history(
            &self.state,
            &self.label,
            (chat_id, Some(peer_user_id)),
            &rows,
            format,
            destination,
        )?;
        let assets = paths[0].with_extension("media");
        for (row, source) in rows.iter().zip(sources) {
            if cancel.is_some_and(|rx| *rx.borrow()) {
                return Err(crate::export::jobs::Cancelled.into());
            }
            if row["type"]
                .as_str()
                .is_some_and(|kind| !matches!(kind, "text" | "service"))
                && let Some(id) = message_id(row)
            {
                let name = format!("secret-{chat_id}-{id}.bin");
                let source = source.join(&name);
                if !super::plain_file(&source)? && row["is_outgoing"] == true {
                    continue;
                }
                let source = self.state.resolve_file(&source, FileAccess::Read).context(
                    "Secret-chat archived media is unavailable; history file was retained",
                )?;
                let bytes =
                    Zeroizing::new(super::read_private(&source, MAX_BYTES)?.context(
                        "Secret-chat archived media is missing; history file was retained",
                    )?);
                if !filter.allows_media(row, bytes.len()) {
                    continue;
                }
                let directory = super::history::mkdir(&self.state, &assets)?;
                let destination = export::output(&self.state, Some(&directory), &name)?;
                write_atomic(&destination, &bytes, false)?;
            }
        }
        Ok((paths, exported_count))
    }

    fn start_export(&self, id: i32, args: &Value) -> Result<Value> {
        let missing = crate::tools::secret::autosave::missing_export_choices(args);
        if !missing.is_empty() {
            return Ok(serde_json::json!({"status":"choices_required","required":missing}));
        }
        let supplied = args["peer_user"].as_i64().filter(|id| *id > 0);
        let peer_user_id = if self.history.is_forgotten(id)? {
            supplied
        } else {
            match self.manager.status(id) {
                Ok(chat) => {
                    ensure!(
                        supplied.is_none_or(|id| id == chat.peer_user_id),
                        "peer_user does not match the secret chat"
                    );
                    Some(chat.peer_user_id)
                }
                Err(SecretError::UnknownChat { .. } | SecretError::ChatClosed { .. }) => supplied,
                Err(_) => bail!("Secret-chat export status is unavailable"),
            }
        }
        .context("Pass peer_user (ID or username) for a deleted secret chat")?;
        let filter = export::ExportFilter::from_args(args)?;
        let dest = args["destination"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let format = ExportFormat::parse(
            args["format"]
                .as_str()
                .context("Missing secret-chat export format")?,
        )?;
        let folder = dest
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| {
                self.state
                    .default_downloads()
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_default();
        let runtime = self.clone();
        crate::export::jobs::start_secret(
            serde_json::json!(i64::from(id) - 2_000_000_000_000),
            folder,
            move |cancel| async move {
                runtime
                    .export(
                        id,
                        peer_user_id,
                        format,
                        dest.as_deref(),
                        &filter,
                        Some(&cancel),
                    )
                    .await
            },
        )
    }
}

fn delivery_snapshot(
    delivery: &kiaro_secret_chat::custody::RelayDelivery,
) -> Result<Option<(Value, Option<MediaReference>)>> {
    use grammers_tl_types::Deserializable;
    use kiaro_secret_chat::enums::DecryptedMessage as Message;
    let message = kiaro_secret_chat::framing::unwrap(&delivery.message.body)?.message;
    let (id, text, ttl, media) = match message {
        Message::Message(m) => (m.random_id, m.message, m.ttl, m.media),
        Message::Legacy36b091de(m) => (m.random_id, m.message, m.ttl, m.media),
        Message::Legacy204d3878(m) => (m.random_id, m.message, m.ttl, Some(m.media)),
        Message::Legacy1f814f1f(m) => (m.random_id, m.message, 0, Some(m.media)),
        _ => return Ok(None),
    };
    let reference = match (media.as_ref(), delivery.message.file.as_deref()) {
        (Some(media), Some(file)) if kiaro_secret_chat::media::file_parts(media).is_ok() => {
            match tl::enums::EncryptedFile::from_bytes(file).map_err(|_| SecretError::Media)? {
                tl::enums::EncryptedFile::File(file) => {
                    Some(MediaReference::new(delivery.chat_id, &file, media)?)
                }
                _ => None,
            }
        }
        _ => None,
    };
    let mut row = super::history::received_row(id, &text, ttl, media.as_ref(), reference.is_some());
    row["date"] = serde_json::json!(
        chrono::DateTime::from_timestamp(delivery.message.date.into(), 0)
            .context("Invalid secret-chat delivery date")?
            .to_rfc3339()
    );
    Ok(Some((row, reference)))
}

fn startup_error(stage: &str, error: SecretError) -> anyhow::Error {
    let cause = match error {
        SecretError::Authorization => "Telegram authorization or connection proof failed",
        SecretError::IngressUnavailable => "durable update ingress is unavailable",
        SecretError::Storage => "the protected store could not be read or committed",
        SecretError::InvalidState | SecretError::Context(_) => "protected state was rejected",
        SecretError::Capacity => "a custody limit was exceeded",
        SecretError::Randomness => "system randomness is unavailable",
        SecretError::ManagerStopping => "the manager is stopping",
        SecretError::ChatState => "the chat state or callback registration was rejected",
        SecretError::Transport => "a Telegram request did not complete",
        _ => "the secret-chat manager refused the operation",
    };
    anyhow::anyhow!("Cannot start the account's durable secret-chat manager ({stage}): {cause}")
}

impl Prepared {
    pub(crate) async fn attach(&mut self, pool: &SenderPoolFatHandle) -> Result<()> {
        self.ingress
            .prepare(pool, self.baseline.take())
            .await
            .map_err(|error| startup_error("attach ingress", error))
    }

    pub(crate) async fn start<F, Fut>(
        self,
        client: Client,
        on_recovered: F,
    ) -> Result<Arc<SecretRuntime>>
    where
        F: FnOnce(Vec<kiaro_secret_chat::ingress::IngressBatch>) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let initialize = |error| startup_error("verify and initialize ingress", error);
        let ingress = Arc::clone(&self.ingress);
        let manager = StrongSecretChatManager::new(
            client.clone(),
            self.ingress,
            CustodyLimits::new(4096, MAX_BYTES, 30.0 * 86400.0).map_err(initialize)?,
        )
        .await
        .map_err(initialize)?;
        let register = |error| startup_error("register history callbacks", error);
        let label = super::HISTORY_HANDLER_LABEL;
        let runtime = Arc::new(SecretRuntime {
            manager,
            history: self.history,
            state: self.state,
            client,
            label: self.label,
            acceptance: Arc::default(),
            mutations: Arc::new(tokio::sync::Mutex::new(())),
        });
        let weak = Arc::downgrade(&runtime);
        runtime
            .manager
            .on_async(
                "ChatRequested",
                super::HISTORY_HANDLER_LABEL,
                move |event| {
                    let weak = weak.clone();
                    async move {
                        if let Event::ChatRequested {
                            chat_id,
                            peer_user_id,
                        } = event
                            && let Some(runtime) = weak.upgrade()
                        {
                            let result = runtime.accept_incoming(chat_id, Some(peer_user_id)).await;
                            if result["status"] == "ready" {
                                tracing::info!("Accepted incoming secret chat");
                            } else {
                                tracing::warn!("Incoming secret chat was not confirmed");
                            }
                        }
                        Ok(())
                    }
                },
            )
            .map_err(register)?;
        let weak = Arc::downgrade(&runtime);
        runtime
            .manager
            .on_async("MessageReceived", label, move |event| {
                let weak = weak.clone();
                async move {
                    let Some(runtime) = weak.upgrade() else {
                        return Err(SecretError::ManagerStopping);
                    };
                    let Event::MessageReceived(message) = event else {
                        return Ok(());
                    };
                    let row = runtime
                        .history
                        .record_message(&message)
                        .map_err(|_| SecretError::Storage)?;
                    if let Some(folder) = runtime
                        .history
                        .archive_row(&runtime.state, message.chat_id, &row)
                        .map_err(|_| SecretError::Storage)?
                        && let Some(reference) = &message.media_reference
                    {
                        let name = format!("secret-{}-{}.bin", message.chat_id, message.random_id);
                        if !super::plain_file(&folder.join(name))
                            .map_err(|_| SecretError::Storage)?
                        {
                            runtime
                                .write_media(reference, message.random_id, Some(&folder))
                                .await
                                .map_err(|_| SecretError::Storage)?;
                        }
                    }
                    Ok(())
                }
            })
            .map_err(register)?;
        let history = Arc::clone(&runtime.history);
        runtime
            .manager
            .on("ServiceActionReceived", label, move |event| {
                history
                    .record_service(event)
                    .map_err(|_| SecretError::Storage)
            })
            .map_err(register)?;
        let history = Arc::clone(&runtime.history);
        runtime
            .manager
            .on("ChatClosed", label, move |event| {
                if let Event::ChatClosed {
                    chat_id,
                    history_deleted,
                    ..
                } = event
                {
                    if history_deleted {
                        history.clear(chat_id).map_err(|_| SecretError::Storage)?;
                    } else {
                        history
                            .clear_media(chat_id)
                            .map_err(|_| SecretError::Storage)?;
                    }
                }
                Ok(())
            })
            .map_err(register)?;
        let pending = ingress
            .pending_batches()
            .map_err(|error| startup_error("read pending ingress batches", error))?;
        if let Err(error) = on_recovered(pending).await {
            let _ = runtime.stop().await;
            return Err(error);
        }
        if let Err(error) = runtime.manager.start().await {
            let _ = runtime.stop().await;
            return Err(startup_error("recover durable work", error));
        }
        Ok(runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_tl_types::Serializable;
    use kiaro_secret_chat::{
        chat::Envelope,
        custody::RelayDelivery,
        files::FileKey,
        media::{DocumentMediaInput, MediaKind, Metadata, document_media},
        types::DecryptedMessage,
    };

    #[test]
    fn delivery_snapshot_preserves_media_caption_and_kind() {
        let key = FileKey::generate().unwrap();
        let media = document_media(DocumentMediaInput {
            kind: MediaKind::VoiceNote,
            name: "voice.ogg",
            mime: "audio/ogg",
            size: 16,
            layer: 143,
            key: &key,
            caption: "A recorded reminder",
            metadata: &Metadata::default(),
        })
        .unwrap();
        let message = DecryptedMessage {
            no_webpage: false,
            silent: false,
            random_id: 102,
            ttl: 0,
            message: String::new(),
            media: Some(media),
            entities: None,
            via_bot_name: None,
            reply_to_random_id: None,
            grouped_id: None,
        };
        let file: tl::enums::EncryptedFile = tl::types::EncryptedFile {
            id: 501,
            access_hash: 999,
            size: 16,
            dc_id: 2,
            key_fingerprint: key.fingerprint(),
        }
        .into();
        let delivery = RelayDelivery {
            chat_id: 10,
            seq_no: 1,
            message: Envelope {
                body: kiaro_secret_chat::framing::wrap(message.into(), 143, 0, 1)
                    .unwrap()
                    .to_bytes(),
                file: Some(file.to_bytes()),
                date: 1_767_225_600,
            },
        };
        let (row, reference) = delivery_snapshot(&delivery).unwrap().unwrap();
        assert_eq!(row["caption"], "A recorded reminder");
        assert_eq!(row["type"], "voice_note");
        assert_eq!(row["date"], "2026-01-01T00:00:00+00:00");
        assert_eq!(reference.unwrap().file_id, 501);
    }
}
