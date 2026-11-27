//! Telegram Bot API transport and owner approval requests.
mod request;

use super::{bot_menu::prompt_text, chat_target, request_fingerprint};
use crate::state::State;
use anyhow::{Context, Result, ensure};
use futures_util::future::join_all;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{OnceCell, oneshot},
    time::Instant,
};
use zeroize::Zeroizing;

pub(super) const DEFAULT_TIMEOUT: u64 = 300;
const API: &str = "https://api.telegram.org";
const POLL_SECONDS: u64 = 20;
const DELIVERY_SECONDS: u64 = 4;

#[derive(Debug)]
struct RejectedRequest;

impl std::fmt::Display for RejectedRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Telegram rejected the approval bot request")
    }
}

impl std::error::Error for RejectedRequest {}

pub(super) struct Config {
    token: Zeroizing<String>,
    owners: BTreeSet<i64>,
    pub(super) timeout: Duration,
}

impl Config {
    pub(super) fn from_environment() -> Option<Self> {
        Self::parse(
            std::env::var("TELEGRAM_APPROVAL_BOT_TOKEN").ok()?.trim(),
            std::env::var("TELEGRAM_APPROVAL_OWNER_IDS").ok().as_deref(),
            std::env::var("TELEGRAM_APPROVAL_OWNER_ID").ok().as_deref(),
            std::env::var("TELEGRAM_APPROVAL_TIMEOUT_SECONDS")
                .ok()
                .as_deref(),
        )
    }

    pub(super) fn parse(
        token: &str,
        owners: Option<&str>,
        owner: Option<&str>,
        timeout: Option<&str>,
    ) -> Option<Self> {
        let (id, secret) = token.split_once(':')?;
        token_id(token)?;
        if !id.bytes().all(|byte| byte.is_ascii_digit())
            || secret.is_empty()
            || !secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return None;
        }
        let owners: BTreeSet<_> = owners
            .filter(|value| !value.trim().is_empty())
            .or(owner)?
            .split(',')
            .map(|value| value.trim().parse::<i64>().ok().filter(|id| *id > 0))
            .collect::<Option<_>>()?;
        let timeout = timeout
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_TIMEOUT)
            .clamp(5, 3600);
        Some(Self {
            token: Zeroizing::new(token.to_owned()),
            owners,
            timeout: Duration::from_secs(timeout),
        })
    }

    pub(super) fn status(&self) -> Value {
        json!(format!("configured ({} owner accounts)", self.owners.len()))
    }
}

pub(super) fn token_id(token: &str) -> Option<i64> {
    token
        .trim()
        .split_once(':')?
        .0
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

pub(crate) fn effective_timeout(configured: Option<Duration>) -> Duration {
    configured.unwrap_or(Duration::from_secs(DEFAULT_TIMEOUT))
}

pub(crate) fn status(
    state: &State,
    bot: Option<&ApprovalBot>,
    account: Option<&str>,
    remembered: BTreeMap<String, usize>,
    observed: BTreeMap<String, super::channels::ChannelAvailability>,
) -> Value {
    let config = bot.is_none().then(Config::from_environment).flatten();
    let config = bot.map(|bot| &bot.config).or(config.as_ref());
    super::status_value(state, account, remembered, observed, config)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Approved,
    Denied,
    TimedOut,
    Unavailable,
}

impl From<Outcome> for super::channels::ChannelOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Approved => Self::ApprovedOnce,
            Outcome::Denied => Self::Denied,
            Outcome::TimedOut => Self::TimedOut,
            Outcome::Unavailable => Self::Unsupported,
        }
    }
}

struct Pending {
    fingerprint: String,
    deadline: Instant,
    always: bool,
    reply: Option<oneshot::Sender<Outcome>>,
    messages: HashMap<i64, i64>,
}

pub(crate) struct ApprovalBot {
    config: Config,
    state: Arc<State>,
    api: reqwest::Client,
    endpoint: String,
    identity: OnceCell<(i64, String)>,
    pending: Mutex<HashMap<String, Pending>>,
    menus: Mutex<super::bot_bypass::OwnerMenus>,
    accounts: Mutex<BTreeMap<String, super::channels::ChannelAvailability>>,
}

impl ApprovalBot {
    pub(super) fn from_environment(state: Arc<State>) -> Option<Arc<Self>> {
        Self::new(Config::from_environment()?, state, API, true).ok()
    }

    fn new(
        config: Config,
        state: Arc<State>,
        endpoint: &str,
        https_only: bool,
    ) -> Result<Arc<Self>> {
        let api = reqwest::Client::builder()
            .https_only(https_only)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(POLL_SECONDS + 5))
            .build()
            .map_err(|_| anyhow::anyhow!("Could not initialize the approval bot transport"))?;
        Ok(Arc::new(Self {
            config,
            state,
            api,
            endpoint: endpoint.to_owned(),
            identity: OnceCell::new(),
            pending: Mutex::new(HashMap::new()),
            menus: Mutex::default(),
            accounts: Mutex::default(),
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_loopback(
        state: Arc<State>,
        address: std::net::SocketAddr,
        timeout: Duration,
    ) -> Arc<Self> {
        assert!(address.ip().is_loopback());
        let mut config = Config::parse("7:fixture-only", Some("101,202"), None, None).unwrap();
        config.timeout = timeout;
        Self::new(config, state, &format!("http://{address}"), false).unwrap()
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.config.timeout
    }

    pub(super) fn peer_keys(&self) -> impl Iterator<Item = String> + '_ {
        token_id(&self.config.token)
            .into_iter()
            .map(|id| id.to_string())
            .chain(
                self.identity
                    .get()
                    .into_iter()
                    .flat_map(|(id, name)| [id.to_string(), name.clone()]),
            )
    }

    pub(super) fn has_waiter(&self, fingerprint: &str) -> bool {
        self.pending.lock().map_or(true, |pending| {
            pending
                .values()
                .any(|request| request.fingerprint == fingerprint)
        })
    }

    pub(super) fn codes(&self) -> Result<Vec<String>> {
        Ok(self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Approval bot request lock is unavailable"))?
            .keys()
            .cloned()
            .collect())
    }

    pub(crate) fn start(self: &Arc<Self>) -> tokio_util::sync::DropGuard {
        let bot = Arc::clone(self);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let worker = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! { () = worker.cancelled() => {}, () = bot.poll() => {} }
        });
        shutdown.drop_guard()
    }

    pub(super) async fn api(&self, method: &'static str, body: Value) -> Result<Value> {
        // reqwest errors and Telegram response descriptions may contain the token-bearing URL.
        let failure = || anyhow::anyhow!("Approval bot request failed ({method})");
        let response = self
            .api
            .post(format!(
                "{}/bot{}/{method}",
                self.endpoint,
                self.config.token.as_str()
            ))
            .json(&body)
            .send()
            .await
            .map_err(|_| failure())?;
        ensure!(response.status().is_success(), failure());
        let value: Value = response.json().await.map_err(|_| failure())?;
        if value["ok"] == false {
            return Err(RejectedRequest.into());
        }
        ensure!(value["ok"] == true, failure());
        Ok(value["result"].clone())
    }

    async fn identify(&self) -> Result<()> {
        self.identity
            .get_or_try_init(|| async {
                let me = self.api("getMe", json!({})).await?;
                let id = me["id"]
                    .as_i64()
                    .filter(|id| Some(*id) == token_id(&self.config.token))
                    .context("Approval bot identity did not match its configuration")?;
                let username = me["username"]
                    .as_str()
                    .filter(|name| {
                        !name.is_empty()
                            && name.len() <= 32
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    })
                    .context("Approval bot identity has no valid username")?;
                ensure!(me["is_bot"] == true, "Approval bot identity is not a bot");
                Ok::<_, anyhow::Error>((id, username.to_ascii_lowercase()))
            })
            .await?;
        Ok(())
    }

    async fn poll(&self) {
        let mut offset: Option<i64> = None;
        let mut commands_registered = false;
        loop {
            if self.identify().await.is_err() {
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            if !commands_registered {
                commands_registered = self
                    .api("setMyCommands", super::bot_menu::commands())
                    .await
                    .is_ok();
            }
            let mut query =
                json!({"timeout":POLL_SECONDS,"allowed_updates":["message","callback_query"]});
            if let Some(offset) = offset {
                query["offset"] = json!(offset);
            }
            let updates = self.api("getUpdates", query).await;
            match updates.as_ref().ok().and_then(Value::as_array) {
                Some(updates) => {
                    for update in updates {
                        if let Some(next) = update["update_id"]
                            .as_i64()
                            .and_then(|id| id.checked_add(1))
                        {
                            if offset.is_some_and(|old| next <= old) {
                                continue;
                            }
                            offset = Some(next);
                            let _ =
                                tokio::time::timeout(Duration::from_secs(2), self.update(update))
                                    .await;
                        }
                    }
                }
                None => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
    }

    async fn update(&self, update: &Value) {
        if let Some(callback) = update.get("callback_query") {
            let from = callback["from"]["id"].as_i64();
            let Some(owner) = from.filter(|owner| self.config.owners.contains(owner)) else {
                return;
            };
            let message = &callback["message"];
            if message["chat"]["id"].as_i64() != Some(owner) || message["chat"]["type"] != "private"
            {
                return;
            }
            let (Some(data), Some(id)) = (callback["data"].as_str(), callback["id"].as_str())
            else {
                return;
            };
            let Some(message_id) = message["message_id"].as_i64().filter(|id| *id > 0) else {
                return;
            };
            if !super::bot_bypass::valid_callback(data) {
                return;
            }
            let notice = if let Some((code, action)) = data
                .strip_prefix("ap:")
                .and_then(|body| body.split_once(':'))
            {
                match self.decide(code, action, owner, message_id) {
                    Ok(true) => "Decision recorded".to_owned(),
                    Ok(false) => "This request is no longer pending".to_owned(),
                    Err(_) => "Could not save decision; no approval was issued".to_owned(),
                }
            } else if data.starts_with("mn:") {
                let current = self
                    .menus
                    .lock()
                    .is_ok_and(|mut menus| menus.consume(owner, message_id, data));
                if !current {
                    return;
                }
                if let Some(key) = data.strip_prefix("mn:bp:") {
                    let Some((_, preset)) = super::bypass::BypassPreset::MENU
                        .iter()
                        .find(|(selector, _)| *selector == key)
                    else {
                        return;
                    };
                    super::bot_bypass::change(&self.state, *preset, owner)
                } else if data == "mn:ac" {
                    let body = self.account_menu();
                    self.send_menu(owner, body).await;
                    "Account list refreshed".to_owned()
                } else {
                    super::bot_menu::callback(&self.state, data)
                        .unwrap_or("Could not update approvals")
                        .to_owned()
                }
            } else {
                return;
            };
            let _ = self
                .api(
                    "answerCallbackQuery",
                    json!({"callback_query_id":id,"text":notice}),
                )
                .await;
        } else if let Some(message) = update.get("message") {
            let from = message["from"]["id"].as_i64();
            let Some(owner) = from.filter(|owner| self.config.owners.contains(owner)) else {
                return;
            };
            if message["chat"]["id"].as_i64() != Some(owner) || message["chat"]["type"] != "private"
            {
                return;
            }
            let Some(command) = message["text"].as_str() else {
                return;
            };
            let username = self
                .identity
                .get()
                .map(|(_, name)| name.as_str())
                .unwrap_or("");
            let observed = self.observed_accounts();
            if let Some(body) = super::bot_menu::message(
                &self.state,
                command,
                username,
                &self.config.token,
                owner,
                &observed,
            ) {
                self.send_menu(owner, body).await;
            }
        }
    }

    pub(super) fn observed_accounts(
        &self,
    ) -> BTreeMap<String, super::channels::ChannelAvailability> {
        self.accounts
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default()
    }

    pub(super) fn observe_account(
        &self,
        account: &str,
        availability: super::channels::ChannelAvailability,
    ) {
        if let Ok(mut rows) = self.accounts.lock() {
            rows.insert(account.to_owned(), availability);
        }
    }

    fn account_menu(&self) -> Value {
        super::bot_bypass::accounts(&self.observed_accounts())
    }

    async fn send_menu(&self, owner: i64, mut body: Value) {
        body["chat_id"] = json!(owner);
        if let Ok(result) = self.api("sendMessage", body.clone()).await
            && let Some(id) = result["message_id"].as_i64().filter(|id| *id > 0)
            && let Ok(mut menus) = self.menus.lock()
        {
            menus.record(owner, id, &body);
        }
    }

    fn decide(&self, code: &str, action: &str, owner: i64, message_id: i64) -> Result<bool> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Approval bot request lock is unavailable"))?;
        let Some(request) = pending.get_mut(code).filter(|request| {
            request.reply.is_some()
                && request.deadline > Instant::now()
                && request.messages.get(&owner) == Some(&message_id)
        }) else {
            return Ok(false);
        };
        let outcome = match action {
            "once" => {
                super::terminal::accept_bot(&self.state, &request.fingerprint, false)?;
                Outcome::Approved
            }
            "always" if request.always => {
                super::terminal::accept_bot(&self.state, &request.fingerprint, true)?;
                Outcome::Approved
            }
            "deny" => {
                self.remove_pending(&request.fingerprint)?;
                Outcome::Denied
            }
            _ => return Ok(false),
        };
        if let Some(reply) = request.reply.take() {
            let _ = reply.send(outcome);
        }
        Ok(true)
    }

    fn remove_pending(&self, fingerprint: &str) -> Result<()> {
        self.state
            .update_json("approval-pending.json", json!({}), |rows| {
                rows.as_object_mut()
                    .context("Approval request record is invalid")?
                    .remove(fingerprint);
                Ok(())
            })
    }

    pub(crate) fn begin(
        self: &Arc<Self>,
        fingerprint: &str,
        tool: &str,
        args: &Value,
    ) -> Result<Option<Request>> {
        ensure!(
            request_fingerprint(tool, args)? == fingerprint,
            "Approval request identity changed"
        );
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Approval bot request lock is unavailable"))?;
        if pending
            .values()
            .any(|request| request.fingerprint == fingerprint)
        {
            return Ok(None);
        }
        let code = loop {
            let code = format!("{:016x}", rand::random::<u64>());
            if !pending.contains_key(&code) {
                break code;
            }
        };
        let (reply, outcome) = oneshot::channel();
        let deadline = Instant::now() + self.config.timeout;
        pending.insert(
            code.clone(),
            Pending {
                fingerprint: fingerprint.to_owned(),
                deadline,
                always: chat_target(args).is_some(),
                reply: Some(reply),
                messages: HashMap::new(),
            },
        );
        Ok(Some(Request {
            bot: Arc::clone(self),
            code,
            deadline,
            outcome,
        }))
    }
}

pub(crate) struct Request {
    bot: Arc<ApprovalBot>,
    code: String,
    deadline: Instant,
    outcome: oneshot::Receiver<Outcome>,
}

fn prompt_buttons(code: &str, always: bool) -> Vec<Value> {
    let mut buttons =
        vec![json!({"text":"Approve once","callback_data":format!("ap:{code}:once")})];
    if always {
        buttons.push(json!({"text":"Always approve","callback_data":format!("ap:{code}:always")}));
    }
    buttons.push(json!({"text":"Deny","callback_data":format!("ap:{code}:deny")}));
    buttons
}
