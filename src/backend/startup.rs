//! Startup helpers that keep `src/backend.rs` concise and verify imported sessions
//! before any secret-chat custody migration or update consumption starts.
use crate::session::PersistentSession;
use anyhow::{Context, Result, bail};
use grammers_client::Client;
use grammers_mtsender::{ConnectionParams, NoRetries, SenderPool};
use grammers_session::{
    Session,
    types::{PeerAuth, PeerId, PeerInfo},
};
use grammers_tl_types as tl;
use std::{sync::Arc, time::Duration};

/// Verify and cache the authorization's live Telegram self user when an imported
/// session lacks a cached `is_self` peer, without consuming updates.
pub(super) async fn ensure_self_peer(
    session: &Arc<PersistentSession>,
    api_id: i32,
    route: &crate::proxy::Route,
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
