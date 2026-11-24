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
        self.write_media(&reference, id, destination).await
    }

    async fn write_media(
        &self,
        reference: &MediaReference,
        id: i64,
        destination: Option<&Path>,
    ) -> Result<PathBuf> {
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
        write_atomic(&path, &bytes, false)?;
        Ok(path)
    }

    pub async fn export(
        &self,
        chat_id: i32,
        format: ExportFormat,
        destination: Option<&Path>,
        include_media: bool,
    ) -> Result<PathBuf> {
        self.status(chat_id)?;
        let mut rows = self.history.read(chat_id, usize::MAX)?;
        rows.reverse();
        let path = export::write_history(
            &self.state,
            &self.label,
            chat_id,
            &rows,
            format,
            destination,
        )?;
        if include_media {
            let assets = path.with_extension("media");
            for row in rows {
                if row["type"]
                    .as_str()
                    .is_some_and(|kind| !matches!(kind, "text" | "service"))
                    && let Some(id) = message_id(&row)
                {
                    let directory = super::history::mkdir(&self.state, &assets)?;
                    self.save_media(chat_id, id, Some(&directory)).await.context("Secret-chat export media could not be saved; history file was retained")?;
                }
            }
        }
        Ok(path)
    }
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

    pub(crate) async fn start(self, client: Client) -> Result<Arc<SecretRuntime>> {
        let initialize = |error| startup_error("verify and initialize ingress", error);
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
            mutations: Arc::new(tokio::sync::Mutex::new(())),
        });
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
                    history_deleted: true,
                    ..
                } = event
                {
                    history.clear(chat_id).map_err(|_| SecretError::Storage)?;
                }
                Ok(())
            })
            .map_err(register)?;
        if let Err(error) = runtime.manager.start().await {
            let _ = runtime.stop().await;
            return Err(startup_error("recover durable work", error));
        }
        Ok(runtime)
    }
}
