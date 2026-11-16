//! Native Telegram transport and mockable domain boundary.
mod config;
mod files;
mod peers;
mod updates;
use crate::{session::PersistentSession, state::State, tl::Schema};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
pub use config::public_configuration_json;
pub use config::{account_labels, generation as account_generation};
pub use config::{accounts_snapshot, health};
use grammers_client::{Client, SignInError};
use grammers_mtsender::{
    ConnectionParams, InvocationError, NoRetries, SenderPool, SenderPoolFatHandle,
};
use grammers_session::Session;
use grammers_tl_types::{self as tl, Serializable};
use serde_json::{Value, json};
use std::{
    io::{IsTerminal, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
    time::Duration,
};

#[async_trait]
pub trait Telegram: Send + Sync {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value>;
    async fn resolve_peer(&self, input: &Value) -> Result<Value>;
    /// Stable identity previously proved for an exact private peer reference.
    /// Implementations must never derive this authority from caller metadata.
    fn pinned_identity(&self, _peer: &Value) -> Option<String> {
        None
    }
    async fn upload(&self, _path: &Path) -> Result<Value> {
        bail!("Upload is unavailable for this backend")
    }
    async fn download(
        &self,
        _location: Value,
        _destination: &Path,
        _max_bytes: u64,
    ) -> Result<Value> {
        bail!("Download is unavailable for this backend")
    }
    async fn download_memory(&self, location: Value, max_bytes: u64) -> Result<Vec<u8>> {
        if max_bytes == 0 || max_bytes > 512 * 1024 * 1024 {
            bail!("In-memory download byte budget must be within 1 byte and 512 MiB");
        }
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("media.bin");
        self.download(location, &path, max_bytes).await?;
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > max_bytes {
            bail!("Media exceeded its in-memory byte budget");
        }
        use std::io::Read;
        let mut data = Vec::new();
        file.take(max_bytes + 1).read_to_end(&mut data)?;
        if data.len() as u64 > max_bytes {
            bail!("Media exceeded its in-memory byte budget");
        }
        Ok(data)
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        None
    }
    /// Explicitly retain an admitted mutation's serialization while its owned
    /// cleanup continues after cancellation. Ordinary background clones do not.
    fn operation_guard(&self) -> Option<Arc<dyn Send + Sync>> {
        None
    }
    fn cache_key(&self) -> Option<String> {
        None
    }
    fn alias_matches(&self, _query: &str) -> Result<Value> {
        Ok(json!([]))
    }
}
struct RawRequest(Vec<u8>);
struct RawResponse(Vec<u8>);
impl tl::Serializable for RawRequest {
    fn serialize(&self, sink: &mut impl FnMut(&[u8])) {
        sink(&self.0)
    }
}
impl tl::RemoteCall for RawRequest {
    type Return = RawResponse;
}
impl tl::Deserializable for RawResponse {
    fn deserialize(cursor: &mut tl::Cursor<'_>) -> tl::deserialize::Result<Self> {
        let mut bytes = Vec::new();
        cursor.read_to_end(&mut bytes)?;
        Ok(Self(bytes))
    }
}
struct Inner {
    client: Client,
    handle: SenderPoolFatHandle,
    session: Arc<PersistentSession>,
    state: Arc<State>,
    label: String,
    stats_dc: AtomicI32,
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    source: Option<(String, String)>,
    nonce: u128,
    authorized_dcs: tokio::sync::Mutex<std::collections::HashSet<i32>>,
    route: crate::proxy::Route,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.handle.quit();
        if let Ok(tasks) = self.tasks.lock() {
            for task in tasks.iter() {
                task.abort();
            }
        }
    }
}
#[derive(Clone)]
pub struct Native(Arc<Inner>);
impl Native {
    pub async fn open(label: &str) -> Result<Self> {
        let state = State::global()?;
        crate::account_manager::recover_rename(&state)?;
        crate::account_manager::recover_logins(&state)?;
        let snapshot = crate::account_manager::current_config()?;
        Self::open_snapshot(state, label, label, &snapshot, false).await
    }
    async fn open_snapshot(
        state: Arc<State>,
        label: &str,
        policy_label: &str,
        snapshot: &crate::account_manager::ConfigSnapshot,
        staged: bool,
    ) -> Result<Self> {
        crate::session::validate_label(label)?;
        crate::session::validate_label(policy_label)?;
        let source = config::source_from(snapshot, label);
        if !staged && source.is_none() {
            bail!("Account is no longer configured; no authorization was opened");
        }
        let api_id = config::api_id(snapshot)?;
        crate::session::check_deferred_custody_with_config(&state, label, snapshot)?;
        let session = Arc::new(PersistentSession::open(Arc::clone(&state), label)?);
        if !state.root().join(format!("{label}.session.json")).exists()
            && let Some((key, value)) = &source
        {
            if key.starts_with("TELEGRAM_SESSION_STRING") {
                session.import_telethon(value)?;
            } else {
                let base = snapshot
                    .path
                    .parent()
                    .context("Configuration file has no parent")?;
                let raw = std::path::PathBuf::from(value);
                let mut candidates = vec![base.join(&raw), state.root().join(&raw)];
                candidates.extend(
                    candidates.clone().into_iter().map(|path| {
                        std::path::PathBuf::from(format!("{}.session", path.display()))
                    }),
                );
                let mut found = candidates
                    .into_iter()
                    .filter(|path| path.is_file())
                    .map(|path| path.canonicalize())
                    .collect::<std::io::Result<Vec<_>>>()?;
                found.sort();
                found.dedup();
                match found.as_slice() {
                    [path] => session.import_telethon_sqlite(path)?,
                    [] => bail!(
                        "No native or legacy session exists for this account; use the account manager to log in"
                    ),
                    _ => bail!(
                        "Several legacy session files match this account; configure one absolute path"
                    ),
                }
            }
        }
        // Recheck the exact imported authorization before creating a sender;
        // the selected legacy source could have changed after the first probe.
        crate::session::check_deferred_custody_with_config(&state, label, snapshot)?;
        let source =
            source.map(|(key, value)| (key, crate::account_manager::config_digest(&value)));
        session.bind_source(
            source
                .as_ref()
                .map(|(key, digest)| format!("{key}:{digest}")),
        )?;
        let mut connection = ConnectionParams {
            app_version: format!("telegram-mcp/{}", env!("CARGO_PKG_VERSION")),
            retry_policy: Box::new(NoRetries),
            ..Default::default()
        };
        session.check_health()?;
        let route = crate::proxy::route(Arc::clone(&state), snapshot, policy_label).await?;
        route.configure(&mut connection);
        let SenderPool {
            runner,
            handle,
            updates,
        } = SenderPool::with_configuration(Arc::clone(&session), api_id, connection);
        let client = Client::new(handle.clone());
        let task = tokio::spawn(async move {
            let _ = runner.run().await;
        });
        let native = Self(Arc::new(Inner {
            client: client.clone(),
            handle,
            session,
            state: Arc::clone(&state),
            label: label.into(),
            source,
            nonce: rand::random(),
            route,
            authorized_dcs: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            stats_dc: AtomicI32::new(0),
            tasks: std::sync::Mutex::new(vec![task]),
        }));
        let watched = Arc::clone(&native.0.session);
        let stop = native.0.handle.thin.clone();
        let health_task = tokio::spawn(async move {
            watched.wait_for_write_uncertainty().await;
            stop.quit();
        });
        let update_task = tokio::spawn(updates::run(
            client,
            native.0.handle.clone(),
            updates,
            state,
            Arc::clone(&native.0.session),
            label.to_owned(),
        ));
        native
            .0
            .tasks
            .lock()
            .map_err(|_| anyhow::anyhow!("Session task lock unavailable"))?
            .extend([health_task, update_task]);
        Ok(native)
    }
    pub fn generation(&self) -> Option<String> {
        self.0
            .source
            .as_ref()
            .map(|(key, digest)| format!("{key}:{digest}"))
    }
    pub async fn shutdown(self) -> Result<()> {
        self.0.handle.quit();
        let tasks = {
            std::mem::take(
                &mut *self
                    .0
                    .tasks
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Session task lock unavailable"))?,
            )
        };
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .context(
                    "A session worker did not stop; the staged authorization was not published",
                )?
                .ok();
        }
        Ok(())
    }
    async fn authorize_dc(&self, dc: i32) -> Result<()> {
        if dc == self.0.session.home_dc_id()? {
            return Ok(());
        }
        let mut authorized = self.0.authorized_dcs.lock().await;
        if authorized.contains(&dc) {
            return Ok(());
        }
        let tl::enums::auth::ExportedAuthorization::Authorization(export) = self
            .0
            .client
            .invoke(&tl::functions::auth::ExportAuthorization { dc_id: dc })
            .await
            .context("Could not authorize the requested Telegram datacenter")?;
        self.0
            .client
            .invoke_in_dc(
                dc,
                &tl::functions::auth::ImportAuthorization {
                    id: export.id,
                    bytes: export.bytes,
                },
            )
            .await
            .context("Could not authorize the requested Telegram datacenter")?;
        authorized.insert(dc);
        Ok(())
    }
    async fn restore_ghost(&self) {
        if self.0.session.check_health().is_err() {
            return;
        }
        if self.0.state.ghost_mode(Some(&self.0.label), None)["effective"].as_bool() == Some(true) {
            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                self.0
                    .client
                    .invoke(&tl::functions::account::UpdateStatus { offline: true }),
            )
            .await;
            if !matches!(outcome, Ok(Ok(_))) {
                tracing::warn!("Could not restore offline presence after account activity");
            }
        }
    }
    async fn invoke_raw(&self, method: &str, params: Value) -> Result<Value> {
        self.0.session.check_health()?;
        let request = RawRequest(Schema::global().encode_call(method, &params)?);
        let dc = if method.starts_with("stats.") {
            self.0.stats_dc.load(Ordering::Relaxed)
        } else {
            0
        };
        let result = if dc > 0 {
            self.authorize_dc(dc).await?;
            self.0.client.invoke_in_dc(dc, &request).await
        } else {
            self.0.client.invoke(&request).await
        };
        let result = match result {
            Err(InvocationError::Rpc(error))
                if matches!(
                    error.name.as_str(),
                    "STATS_MIGRATE" | "USER_MIGRATE" | "NETWORK_MIGRATE"
                ) =>
            {
                let dc = i32::try_from(
                    error
                        .value
                        .context("Migration did not specify a datacenter")?,
                )?;
                if !(1..=5).contains(&dc) {
                    bail!("Telegram requested an unknown datacenter");
                }
                if method.starts_with("stats.") {
                    self.0.stats_dc.store(dc, Ordering::Relaxed);
                }
                self.authorize_dc(dc).await?;
                self.0.client.invoke_in_dc(dc, &request).await
            }
            result => result,
        };
        let bytes = result.map_err(|error| match error {
            InvocationError::Rpc(rpc) => anyhow::anyhow!(
                "Telegram RPC {} {}{}",
                rpc.code,
                rpc.name,
                rpc.value.map(|n| format!(" ({n})")).unwrap_or_default()
            ),
            _ => anyhow::anyhow!(
                "Telegram transport failed; operation outcome may be unknown and was not retried"
            ),
        })?;
        let result = Schema::global().decode_response(method, &params, &bytes.0)?;
        peers::cache_rpc_outcome(self.0.session.as_ref(), result).await
    }
}
#[async_trait]
impl Telegram for Native {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        let result = match method {
            "local.accounts" => accounts_snapshot(),
            "local.session_health" => {
                self.0.session.check_health()?;
                let requested = params["config_key"]
                    .as_str()
                    .zip(params["config_digest"].as_str());
                let current = self
                    .0
                    .source
                    .as_ref()
                    .map(|(key, digest)| (key.as_str(), digest.as_str()));
                if requested != current {
                    return Ok(
                        json!({"status":"unknown","account":self.0.label,"reason":"Requested configuration is not owned by this live session"}),
                    );
                }
                let authorized = self.0.client.is_authorized().await?;
                let user_id = if authorized {
                    Some(
                        self.0
                            .client
                            .get_me()
                            .await?
                            .id()
                            .bare_id_unchecked()
                            .to_string(),
                    )
                } else {
                    None
                };
                Ok(
                    json!({"account":self.0.label,"authorized":authorized,"user_id":user_id,"connected":true,"config_key":current.map(|s|s.0),"config_digest":current.map(|s|s.1)}),
                )
            }
            "local.connection_route" => Ok(self.0.route.metadata()),
            _ => self.invoke_raw(method, params).await,
        };
        if method != "account.updateStatus" {
            self.restore_ghost().await;
        }
        result
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.0.session.check_health()?;
        let result = self.resolve(input).await;
        self.restore_ghost().await;
        result
    }
    async fn upload(&self, path: &Path) -> Result<Value> {
        self.0.session.check_health()?;
        let result = self.upload_file(path).await;
        self.restore_ghost().await;
        result
    }
    async fn download(&self, location: Value, destination: &Path, max_bytes: u64) -> Result<Value> {
        self.0.session.check_health()?;
        let result = self.download_file(location, destination, max_bytes).await;
        self.restore_ghost().await;
        result
    }
    async fn download_memory(&self, location: Value, max_bytes: u64) -> Result<Vec<u8>> {
        self.0.session.check_health()?;
        let result = self.download_in_memory(location, max_bytes).await;
        self.restore_ghost().await;
        result
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        Some(Arc::new(self.clone()))
    }
    fn cache_key(&self) -> Option<String> {
        Some(format!("{}:{:032x}", self.0.label, self.0.nonce))
    }
    fn alias_matches(&self, query: &str) -> Result<Value> {
        self.0.state.alias_matches(&self.0.label, query)
    }
}
pub fn parse_text(text: &str, mode: Option<&str>) -> Result<(String, Vec<Value>)> {
    let (text, entities) = match mode.unwrap_or("plain").to_ascii_lowercase().as_str() {
        "plain" | "none" | "" => (text.to_owned(), Vec::new()),
        "markdown" | "md" => grammers_client::parsers::parse_markdown_message(text),
        "html" => grammers_client::parsers::parse_html_message(text),
        _ => bail!("parse_mode must be plain, markdown or html"),
    };
    Ok((
        text,
        entities
            .into_iter()
            .map(|entity| Schema::global().decode("MessageEntity", &entity.to_bytes()))
            .collect::<Result<_>>()?,
    ))
}
pub async fn login(label: &str) -> Result<()> {
    login_transaction(label, Path::new(".env"), None, None, None).await?;
    Ok(())
}
pub async fn login_transaction(
    label: &str,
    env_file: &Path,
    replace_key: Option<&str>,
    expected_config_digest: Option<&str>,
    expected_user_id: Option<i64>,
) -> Result<Value> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("Login requires a genuine interactive terminal");
    }
    let transaction = crate::account_manager::begin_login(
        State::global()?,
        label,
        env_file,
        replace_key,
        expected_config_digest,
        expected_user_id,
    )?;
    let snapshot = crate::account_manager::read_config(env_file)?;
    let hash = snapshot
        .values
        .get("TELEGRAM_API_HASH")
        .context("TELEGRAM_API_HASH is required; obtain it from my.telegram.org/apps")?;
    let native = Native::open_snapshot(
        State::global()?,
        transaction.stage_label(),
        label,
        &snapshot,
        true,
    )
    .await?;
    let phone = prompt("Phone number (international format): ")?;
    native.0.session.check_health()?;
    let token = native
        .0
        .client
        .request_login_code(&phone, hash)
        .await
        .context("Could not request login code")?;
    let code = prompt("Telegram login code: ")?;
    native.0.session.check_health()?;
    match native.0.client.sign_in(&token, &code).await {
        Ok(_) => {}
        Err(SignInError::PasswordRequired(token)) => {
            let password = rpassword::prompt_password("Two-step verification password: ")?;
            native.0.session.check_health()?;
            native
                .0
                .client
                .check_password(token, password)
                .await
                .context("Two-step verification failed")?;
        }
        Err(_) => bail!("Telegram sign-in failed; verify the code before trying again"),
    }
    native.0.session.check_health()?;
    let id = native
        .0
        .client
        .get_me()
        .await
        .context("Could not verify the authorized account identity")?
        .id()
        .bare_id_unchecked();
    native.shutdown().await?;
    let result = transaction.commit(id)?;
    eprintln!("Account authorized; the native session was saved privately");
    Ok(result)
}
fn prompt(message: &str) -> Result<String> {
    eprint!("{message}");
    std::io::stderr().flush()?;
    let mut value = String::new();
    std::io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_owned())
}
