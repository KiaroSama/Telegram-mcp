//! Approval decisions stay outside domain handlers, before any side effect.

pub(crate) mod bot;
mod bot_menu;
mod policy;
pub mod tables;
mod taint;
mod terminal;

use crate::state::{State, normalize_key};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use policy::{Category, Facts, PolicyDecision, categorize, decide};
pub use terminal::approve_from_terminal;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Decision {
    Allow,
    NeedsApproval {
        fingerprint: String,
        reasons: Vec<String>,
    },
    Refuse {
        reason: String,
    },
}

type RecentSends = HashMap<(String, String), VecDeque<(Instant, String)>>;

pub struct Safeguard {
    state: Arc<State>,
    taint: Mutex<taint::Memory>,
    sends: Mutex<RecentSends>,
    pub(crate) bot: Option<Arc<bot::ApprovalBot>>,
}

pub(super) fn chat_target(args: &Value) -> Option<&Value> {
    args.get("chat_id")
        .or_else(|| args.get("to_chat_id"))
        .filter(|chat| !chat.is_null())
}

impl Safeguard {
    pub fn new(state: Arc<State>) -> Self {
        Self {
            bot: bot::ApprovalBot::from_environment(Arc::clone(&state)),
            state,
            taint: Mutex::new(taint::Memory::default()),
            sends: Mutex::new(HashMap::new()),
        }
    }

    fn approval_chats(&self) -> std::collections::HashSet<String> {
        let explicit = std::env::var("TELEGRAM_APPROVAL_CHAT_IDS").unwrap_or_default();
        let username = std::env::var("TELEGRAM_APPROVAL_BOT_USERNAME").unwrap_or_default();
        let token = zeroize::Zeroizing::new(
            std::env::var("TELEGRAM_APPROVAL_BOT_TOKEN").unwrap_or_default(),
        );
        explicit
            .split(',')
            .chain([username.as_str()])
            .filter(|value| !value.trim().is_empty())
            .map(|value| policy::chat_key(&json!(value)))
            .chain(bot::token_id(&token).map(|id| id.to_string()))
            .chain(self.bot.iter().flat_map(|bot| bot.peer_keys()))
            .collect()
    }

    pub(crate) fn has_approval_peers(&self) -> bool {
        !self.approval_chats().is_empty()
    }

    pub(crate) fn check_approval_channel(&self, args: &Value) -> Result<()> {
        let peers = self.approval_chats();
        let codes = self
            .bot
            .as_ref()
            .map(|bot| bot.codes())
            .transpose()?
            .unwrap_or_default();
        if policy::touches_approval_channel(args, &peers, &codes) {
            bail!("touches_approval_channel");
        }
        Ok(())
    }

    pub fn check(
        &self,
        tool: &str,
        args: &Value,
        read_only: bool,
        destructive: bool,
    ) -> Result<Decision> {
        self.check_with_waiter(tool, args, read_only, destructive, false)
    }

    pub(crate) fn check_with_waiter(
        &self,
        tool: &str,
        args: &Value,
        read_only: bool,
        destructive: bool,
        redeem: bool,
    ) -> Result<Decision> {
        let account = args["account"].as_str().unwrap_or("default").to_lowercase();
        let chat = chat_target(args).unwrap_or(&Value::Null);
        let category = categorize(tool, read_only, destructive);
        let ghost = self.state.ghost_mode(Some(&account), Some(chat))["effective"]
            .as_bool()
            .unwrap_or(true);
        let mut facts = Facts {
            ghost_on: ghost,
            ..Default::default()
        };
        facts.protected_paths.push(self.state.root().to_path_buf());
        facts
            .protected_paths
            .push(std::env::current_dir()?.join("src/safeguard"));
        facts
            .protected_paths
            .push(std::env::current_dir()?.join("telegram_mcp/safeguard"));
        facts.approval_chats = self.approval_chats();
        if let Some(bot) = &self.bot {
            facts.pending_codes.extend(bot.codes()?);
        }
        if category == Category::Send {
            let sends = self
                .sends
                .lock()
                .map_err(|_| anyhow::anyhow!("Send policy lock is unavailable"))?;
            let recent = sends.get(&(account.clone(), tool.to_owned()));
            facts.bulk = recent.is_some_and(|items| {
                let mut chats: std::collections::HashSet<_> = items
                    .iter()
                    .filter(|(when, _)| when.elapsed() < Duration::from_secs(60))
                    .map(|(_, chat)| chat.as_str())
                    .collect();
                let target = normalize_key(chat);
                chats.insert(&target);
                chats.len() > 5
            });
            let known = self.state.read_json("sent-recipients.json", json!({}))?;
            let entries = known
                .as_object()
                .context("Sent-recipient store is invalid")?;
            facts.first_message = match entries.get(&account) {
                None | Some(Value::Null) => true,
                Some(Value::Array(rows)) => {
                    !rows.iter().any(|v| normalize_key(v) == normalize_key(chat))
                }
                _ => bail!("Sent-recipient record is invalid; repair private state before sending"),
            };
        }
        if !read_only {
            facts.tainted = self
                .taint
                .lock()
                .map_err(|_| anyhow::anyhow!("Untrusted-content policy lock is unavailable"))?
                .find(&account, args);
        }
        let decision = decide(tool, read_only, destructive, args, &facts);
        if decision.outcome == "refuse" {
            return Ok(Decision::Refuse {
                reason: decision.reasons.join(", "),
            });
        }
        let already_allowed = decision.outcome == "run"
            || (self.state.approval_is_granted(&account, tool, chat)
                && !decision
                    .reasons
                    .iter()
                    .any(|reason| reason == "outside_project_folder"));
        if already_allowed && !redeem {
            return Ok(Decision::Allow);
        }
        let fingerprint = request_fingerprint(tool, args)?;
        if !redeem
            && self
                .bot
                .as_ref()
                .is_some_and(|bot| bot.has_waiter(&fingerprint))
        {
            return Ok(Decision::NeedsApproval {
                fingerprint,
                reasons: decision.reasons,
            });
        }
        let now = unix_seconds();
        let consumed = self
            .state
            .update_json("approval-grants.json", json!({}), |grants| {
                let approved = grants
                    .get(&fingerprint)
                    .and_then(|value| value["expires"].as_u64())
                    .is_some_and(|expiry| expiry >= now);
                if let Some(map) = grants.as_object_mut() {
                    map.remove(&fingerprint);
                } else {
                    bail!("Approval grant record is invalid");
                }
                Ok(approved)
            })?;
        if consumed || already_allowed {
            self.state
                .update_json("approval-pending.json", json!({}), |pending| {
                    if let Some(map) = pending.as_object_mut() {
                        map.remove(&fingerprint);
                    }
                    Ok(())
                })?;
            return Ok(Decision::Allow);
        }
        self.state.update_json("approval-pending.json",json!({}),|pending| {
            let map=pending.as_object_mut().context("Approval request record is invalid")?;
            map.retain(|_,value|value["expires"].as_u64().is_some_and(|expiry|expiry>=now));
            if map.len() >= 128 && !map.contains_key(&fingerprint) { bail!("Too many pending approval requests; resolve them in the owner terminal"); }
            let timeout = self.bot.as_ref().map_or(bot::DEFAULT_TIMEOUT, |bot| bot.timeout().as_secs());
            map.insert(fingerprint.clone(),json!({"tool":tool,"arguments":args,"reasons":decision.reasons,"expires":now+timeout}));
            Ok(())
        })?;
        Ok(Decision::NeedsApproval {
            fingerprint,
            reasons: decision.reasons,
        })
    }

    pub fn note_success(&self, tool: &str, args: &Value) -> Result<()> {
        if !tables::SEND.contains(&tool) {
            return Ok(());
        }
        let account = args["account"].as_str().unwrap_or("default").to_lowercase();
        let chat = args
            .get("chat_id")
            .or_else(|| args.get("to_chat_id"))
            .context("A send tool did not provide a recipient")?;
        let key = normalize_key(chat);
        // Count the accepted remote send even if supplementary disk bookkeeping
        // fails. The account execution lock protects the next approval decision.
        {
            let mut sends = self
                .sends
                .lock()
                .map_err(|_| anyhow::anyhow!("Send policy lock is unavailable"))?;
            let recent = sends.entry((account.clone(), tool.to_owned())).or_default();
            while recent
                .front()
                .is_some_and(|(when, _)| when.elapsed() >= Duration::from_secs(60))
            {
                recent.pop_front();
            }
            recent.push_back((Instant::now(), key.clone()));
            while recent.len() > 1000 {
                recent.pop_front();
            }
        }
        self.state
            .update_json("sent-recipients.json", json!({}), |known| {
                known
                    .as_object()
                    .context("Sent-recipient store is invalid")?;
                if known[&account].is_null() {
                    known[&account] = json!([]);
                }
                let rows = known[&account]
                    .as_array_mut()
                    .context("Sent-recipient record is invalid")?;
                if !rows.iter().any(|v| normalize_key(v) == key) {
                    rows.push(json!(key));
                }
                if rows.len() > 10_000 {
                    rows.remove(0);
                }
                Ok(())
            })?;
        Ok(())
    }

    pub fn note_rendered(&self, account: &str, chat: &Value, value: &Value) {
        if let Ok(mut memory) = self.taint.lock() {
            memory.note_value(account, chat, value);
        }
    }
}

pub fn request_fingerprint(tool: &str, args: &Value) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(b"telegram-mcp-approval-v1\0");
    hash.update(tool.as_bytes());
    hash.update(b"\0");
    hash.update(serde_json::to_vec(&canonical(args))?);
    Ok(crate::lowercase_hex(&hash.finalize()))
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect();
            serde_json::to_value(sorted).expect("JSON object serializes")
        }
        Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
