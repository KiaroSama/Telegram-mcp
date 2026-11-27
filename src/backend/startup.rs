//! Startup helpers that keep `src/backend.rs` concise and verify imported sessions
//! before any secret-chat custody migration or update consumption starts.
use super::config::{self, ClientIdentity};
use crate::{account_manager::ConfigSnapshot, session::PersistentSession, state::State};
use anyhow::{Context, Result, bail, ensure};
use grammers_client::Client;
use grammers_mtsender::{ConnectionParams, NoRetries, SenderPool};
use grammers_session::{
    Session,
    types::{PeerAuth, PeerId, PeerInfo},
};
use grammers_tl_types as tl;
use std::{sync::Arc, time::Duration};

/// Acquire an exclusive session OS lease honoring `TELEGRAM_LOCK_GRACE_SECONDS`,
/// selecting the first free deduplicated entry from `TELEGRAM_SESSION_STRINGS`
/// when opening the default pooled slot.
pub(super) async fn open_leased_session(
    state: &Arc<State>,
    label: &str,
    snapshot: &ConfigSnapshot,
    staged: bool,
) -> Result<(Arc<PersistentSession>, Option<(String, String)>, String)> {
    let grace = crate::account_manager::lock_grace_seconds(Some(snapshot))?;
    let source = config::source_from(snapshot, label);
    if !staged && source.is_none() {
        bail!("Account is no longer configured; no authorization was opened");
    }
    if let Some((key, _)) = &source
        && key == "TELEGRAM_SESSION_STRINGS"
    {
        let pool = config::pooled_sessions(snapshot);
        if pool.is_empty() {
            bail!("No eligible pooled sessions remain in TELEGRAM_SESSION_STRINGS");
        }
        ensure!(
            pool.iter().all(
                |candidate| crate::account_manager::session_validate(&candidate.value) == "valid"
            ),
            "TELEGRAM_SESSION_STRINGS contains an invalid session; no authorization was opened"
        );
        let mut custody_config = ConfigSnapshot {
            values: snapshot.values.clone(),
            digest: snapshot.digest.clone(),
            path: snapshot.path.clone(),
            process_keys: snapshot.process_keys.clone(),
        };
        for candidate in &pool {
            custody_config.values.insert(
                format!(
                    "TELEGRAM_SESSION_STRING_{}",
                    candidate.slot_label.to_ascii_uppercase()
                ),
                candidate.value.clone(),
            );
        }
        // Resolve each candidate's authorization before creating or importing a slot.
        for candidate in &pool {
            PersistentSession::check_custody_identity(
                state,
                &candidate.slot_label,
                &custody_config,
            )?;
        }
        let (session, chosen) = crate::account_manager::acquire_with_grace(grace, None, || {
            let mut last_error = None;
            for candidate in &pool {
                let session =
                    match PersistentSession::open(Arc::clone(state), &candidate.slot_label) {
                        Ok(session) => session,
                        Err(error) => {
                            last_error = Some(error);
                            continue;
                        }
                    };
                if !state
                    .root()
                    .join(format!("{}.session.json", candidate.slot_label))
                    .exists()
                    && let Err(error) = session.import_telethon(&candidate.value)
                {
                    // The authorization lease is acquired during first import, not slot creation.
                    last_error = Some(error);
                    continue;
                }
                PersistentSession::check_custody_identity(
                    state,
                    &candidate.slot_label,
                    &custody_config,
                )?;
                return Ok((Arc::new(session), candidate.clone()));
            }
            Err(last_error.unwrap_or_else(|| {
                anyhow::anyhow!("No eligible pooled authorization is available")
            }))
        })
        .await
        .context("No eligible pooled authorization could be opened")?;
        return Ok((
            session,
            Some(("TELEGRAM_SESSION_STRINGS".into(), chosen.value)),
            chosen.slot_label,
        ));
    }
    PersistentSession::check_custody_identity(state, label, snapshot)?;
    let session = Arc::new(
        crate::account_manager::acquire_with_grace(grace, None, || {
            PersistentSession::open(Arc::clone(state), label)
        })
        .await?,
    );
    if !state.root().join(format!("{label}.session.json")).exists()
        && let Some((key, value)) = &source
    {
        import_legacy_session(state, snapshot, &session, key, value)?;
    }
    // Recheck the exact imported authorization before creating any sender;
    // the selected legacy source could have changed after the first probe.
    PersistentSession::check_custody_identity(state, label, snapshot)?;
    Ok((session, source, label.to_owned()))
}

fn import_legacy_session(
    state: &State,
    snapshot: &ConfigSnapshot,
    session: &PersistentSession,
    key: &str,
    value: &str,
) -> Result<()> {
    if key.starts_with("TELEGRAM_SESSION_STRING") {
        return session.import_telethon(value);
    }
    let base = snapshot
        .path
        .parent()
        .context("Configuration file has no parent")?;
    let raw = std::path::PathBuf::from(value);
    let mut candidates = vec![base.join(&raw), state.root().join(&raw)];
    candidates.extend(
        candidates
            .clone()
            .into_iter()
            .map(|path| std::path::PathBuf::from(format!("{}.session", path.display()))),
    );
    let mut found = candidates
        .into_iter()
        .filter(|path| path.is_file())
        .map(|path| path.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    found.sort();
    found.dedup();
    match found.as_slice() {
        [path] => session.import_telethon_sqlite(path),
        [] => bail!(
            "No native or legacy session exists for this account; use the account manager to log in"
        ),
        _ => bail!("Several legacy session files match this account; configure one absolute path"),
    }
}

/// Verify and cache the authorization's live Telegram self user when an imported
/// session lacks a cached `is_self` peer, without consuming updates.
pub(super) async fn ensure_self_peer(
    session: &Arc<PersistentSession>,
    api_id: i32,
    route: &crate::proxy::Route,
    identity: &ClientIdentity,
) -> Result<()> {
    if session.peer(PeerId::self_user()).await?.is_some() {
        return Ok(());
    }
    let home = session.home_dc_id()?;
    if session
        .dc_option(home)?
        .and_then(|dc| dc.auth_key)
        .is_none()
    {
        return Ok(());
    }
    let mut connection = ConnectionParams {
        app_version: format!("telegram-mcp/{}", env!("CARGO_PKG_VERSION")),
        retry_policy: Box::new(NoRetries),
        ..Default::default()
    };
    identity.apply_to(&mut connection);
    route.configure(&mut connection);
    let SenderPool {
        runner,
        handle,
        updates: _updates,
    } = SenderPool::with_configuration(Arc::clone(session), api_id, connection);
    let client = Client::new(handle.clone());
    let task = tokio::spawn(async move {
        let _ = runner.run().await;
    });
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        client.invoke(&tl::functions::InvokeWithoutUpdates {
            query: tl::functions::users::GetUsers {
                id: vec![tl::enums::InputUser::UserSelf],
            },
        }),
    )
    .await;
    handle.quit();
    task.abort();
    let _ = task.await;
    let users = result
        .context("Timed out verifying imported session user identity")?
        .context("Failed to verify imported session user identity")?;
    let user = users
        .into_iter()
        .find_map(|entry| match entry {
            tl::enums::User::User(user) if user.is_self => Some(user),
            _ => None,
        })
        .context("Telegram returned no self user for the imported authorization")?;
    if user.id <= 0 {
        bail!("Telegram returned an invalid user identity for the imported authorization");
    }
    session
        .cache_peer(PeerInfo::User {
            id: user.id,
            auth: user.access_hash.map(PeerAuth::from_hash),
            bot: Some(user.bot),
            is_self: Some(true),
        })
        .await?;
    Ok(())
}

pub(super) async fn start_secret(
    prepared: Option<crate::secret::Prepared>,
    client: Client,
    state: Arc<crate::state::State>,
    session: Arc<PersistentSession>,
    label: String,
    handle: &grammers_mtsender::SenderPoolFatHandle,
    task: &mut tokio::task::JoinHandle<()>,
) -> Result<Option<Arc<crate::secret::SecretRuntime>>> {
    let Some(prepared) = prepared else {
        return Ok(None);
    };
    let rec_client = client.clone();
    match prepared
        .start(client, move |batches| async move {
            super::updates::publish_recovered_cloud(&rec_client, &state, &session, &label, &batches)
                .await
        })
        .await
    {
        Ok(runtime) => Ok(Some(runtime)),
        Err(error) => {
            handle.quit();
            task.abort();
            let _ = task.await;
            Err(error)
        }
    }
}
