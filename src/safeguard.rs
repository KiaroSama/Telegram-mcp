//! Approval decisions stay outside domain handlers, before any side effect.

pub(crate) mod approval_details;
pub(crate) mod bot;
mod bot_bypass;
mod bot_menu;
pub(crate) mod bypass;
pub(crate) mod channels;
pub(crate) mod folders;
mod policy;
pub(crate) mod sealed;
mod status;
use status::status_value;
pub mod tables;
mod taint;
mod terminal;

use crate::state::{State, normalize_key};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
mod fingerprint;
pub use fingerprint::request_fingerprint;
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
    seals: sealed::SealedStore,
    details: approval_details::ApprovalDetails,
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
            seals: sealed::SealedStore::new(Arc::clone(&state)),
            details: approval_details::ApprovalDetails::default(),
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
        let account = args["account"].as_str().unwrap_or("default");
        if policy::touches_approval_channel(args, &peers, &codes)
            || self.seals.is_sealed_target(account, args)
        {
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
        self.check_with_waiter_detailed(tool, args, read_only, destructive, redeem)
            .map(|(decision, _)| decision)
    }

    pub(crate) fn check_with_waiter_detailed(
        &self,
        tool: &str,
        args: &Value,
        read_only: bool,
        destructive: bool,
        redeem: bool,
    ) -> Result<(Decision, bool)> {
        let account = args["account"].as_str().unwrap_or("default").to_lowercase();
        let chat = chat_target(args).unwrap_or(&Value::Null);
        if self.check_approval_channel(args).is_err() {
            return Ok((
                Decision::Refuse {
                    reason: "touches_approval_channel".to_owned(),
                },
                false,
            ));
        }
        let category = categorize(tool, read_only, destructive);
        let ghost = self.state.ghost_mode(Some(&account), Some(chat))["effective"]
            .as_bool()
            .unwrap_or(true);
        let mut facts = Facts {
            ghost_on: ghost,
            ..Default::default()
        };
        facts.protected_paths.push(self.state.root().to_path_buf());
        for base in [
            std::env::current_dir()?,
            self.state.installation_root().to_path_buf(),
        ] {
            facts.protected_paths.extend([
                base.join("src/safeguard"),
                base.join("telegram_mcp/safeguard"),
            ]);
        }
        let paths = folders::tool_paths(tool, args);
        if !paths.is_empty() {
            let store = folders::FolderGrants::new(&self.state, self.state.installation_root())?;
            facts.protected_folder = paths
                .iter()
                .any(|(path, _)| store.resolve_for_call(path).is_err());
        }
        let refusal = decide(tool, read_only, destructive, args, &facts);
        if refusal.outcome == "refuse" {
            return Ok((
                Decision::Refuse {
                    reason: refusal.reasons.join(", "),
                },
                false,
            ));
        }
        for (path, scope) in paths {
            match self.authorize_folder("", &account, path, scope, None) {
                Ok(Some(_)) => {}
                Ok(None) => facts
                    .outside_folders
                    .push("owner authorization required".to_owned()),
                Err(_) => {
                    return Ok((
                        Decision::Refuse {
                            reason: "folder_authorization_unavailable".to_owned(),
                        },
                        false,
                    ));
                }
            }
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
            return Ok((
                Decision::Refuse {
                    reason: decision.reasons.join(", "),
                },
                false,
            ));
        }
        let now = unix_time();
        let bypass = bypass::BypassState::read(&self.state, now).is_active(now);
        let already_allowed = decision.outcome == "run"
            || bypass
            || (self.state.approval_is_granted(&account, tool, chat)
                && !decision
                    .reasons
                    .iter()
                    .any(|reason| reason == "outside_project_folder"));
        if already_allowed && !redeem {
            return Ok((Decision::Allow, false));
        }
        let fingerprint = request_fingerprint(tool, args)?;
        if !redeem
            && self
                .bot
                .as_ref()
                .is_some_and(|bot| bot.has_waiter(&fingerprint))
        {
            return Ok((
                Decision::NeedsApproval {
                    fingerprint,
                    reasons: decision.reasons,
                },
                false,
            ));
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
            return Ok((Decision::Allow, consumed));
        }
        self.state.update_json("approval-pending.json",json!({}),|pending| {
            let map=pending.as_object_mut().context("Approval request record is invalid")?;
            map.retain(|_,value|value["expires"].as_u64().is_some_and(|expiry|expiry>=now));
            if map.len() >= 128 && !map.contains_key(&fingerprint) { bail!("Too many pending approval requests; resolve them in the owner terminal"); }
            let timeout = self.bot.as_ref().map_or(bot::DEFAULT_TIMEOUT, |bot| bot.timeout().as_secs());
            map.insert(fingerprint.clone(),json!({"tool":tool,"arguments":args,"reasons":decision.reasons,"expires":now+timeout}));
            Ok(())
        })?;
        Ok((
            Decision::NeedsApproval {
                fingerprint,
                reasons: decision.reasons,
            },
            false,
        ))
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
        let value = self.redact_tool_result(account, value);
        if let Ok(mut memory) = self.taint.lock() {
            memory.note_value(account, chat, &value);
        }
    }

    pub(crate) fn redact_tool_result(&self, account: &str, value: &Value) -> Value {
        self.seals.redact_result(account, value)
    }

    pub(crate) fn status_for_account(&self, account: Option<&str>) -> Value {
        let remembered = self
            .taint
            .lock()
            .map(|memory| memory.remembered())
            .unwrap_or_default();
        let observed = self
            .bot
            .as_ref()
            .map(|bot| bot.observed_accounts())
            .unwrap_or_default();
        bot::status(
            &self.state,
            self.bot.as_deref(),
            account,
            remembered,
            observed,
        )
    }

    #[cfg(test)]
    pub(crate) async fn request_owner_approval(
        &self,
        routes: &channels::ApprovalRoutes<'_>,
        fingerprint: &str,
        tool: &str,
        args: &Value,
        reasons: &[String],
        before: Option<&Value>,
    ) -> Result<channels::ChannelOutcome> {
        self.request_owner_approval_with_id(
            routes,
            fingerprint,
            fingerprint,
            tool,
            args,
            reasons,
            before,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn request_owner_approval_with_id(
        &self,
        routes: &channels::ApprovalRoutes<'_>,
        request_id: &str,
        fingerprint: &str,
        tool: &str,
        args: &Value,
        reasons: &[String],
        before: Option<&Value>,
    ) -> Result<channels::ChannelOutcome> {
        use channels::{ApprovalPrompt, ChannelOutcome};
        let account = args["account"].as_str().unwrap_or("default");
        use channels::{ApprovalChannel, ChannelAvailability};
        let mut capabilities = routes.capabilities(account);
        capabilities.bot = if self.bot.is_some() {
            ChannelAvailability::Available
        } else {
            ChannelAvailability::Unavailable
        };
        capabilities.owner_terminal = ChannelAvailability::Available;
        if capabilities.saved_messages != ChannelAvailability::Unknown {
            self.observe_approval_account(account, capabilities.saved_messages);
        }
        let allow_always = chat_target(args).is_some()
            && !reasons
                .iter()
                .any(|reason| reason == "outside_project_folder");
        let target =
            chat_target(args).map_or_else(|| "No specific chat".to_owned(), Value::to_string);
        let entries = self.details.detail_for(
            tool,
            approval_details::DetailInput {
                target: &target,
                before,
                arguments: args,
            },
            &self.seals,
        );
        let lines = approval_details::prompt_lines(&entries);
        let text = format!(
            "Telegram MCP approval\nAccount: {account}\nTool: {tool}\nTarget: {}\nReasons: {}\n{lines}",
            self.seals.redact_text(&target),
            reasons.join(", ")
        );
        let timeout = self
            .bot
            .as_ref()
            .map_or(Duration::from_secs(bot::DEFAULT_TIMEOUT), |bot| {
                bot.timeout()
            });
        let prompt = ApprovalPrompt {
            request_id,
            account,
            text: &text,
            allow_always,
            deadline: tokio::time::Instant::now() + timeout,
        };
        let mut outcome = ChannelOutcome::Unsupported;
        if capabilities.availability(ApprovalChannel::DialogChannel)
            == ChannelAvailability::Available
            && let Some(dialog) = routes.dialog
        {
            outcome = channels::dialog_request(dialog, &prompt, &self.seals).await?;
        }
        if outcome.may_fall_through()
            && capabilities.availability(ApprovalChannel::BotChannel)
                == ChannelAvailability::Available
            && let Some(bot) = &self.bot
        {
            let Some(mut request) = bot.begin(fingerprint, tool, args)? else {
                // An existing prompt owns this ticket; never ask the next channel.
                return Ok(ChannelOutcome::Unsupported);
            };
            outcome = request
                .wait_detailed(tool, args, fingerprint, reasons, &lines, allow_always)
                .await?;
            if !outcome.may_fall_through() {
                return Ok(outcome);
            }
        }
        if outcome.may_fall_through()
            && capabilities.availability(ApprovalChannel::SavedMessagesChannel)
                == ChannelAvailability::Available
            && let Some(transport) = routes.saved_messages
        {
            outcome = channels::saved_messages_request(transport, &prompt, &self.seals).await?;
        }
        match outcome {
            ChannelOutcome::ApprovedOnce | ChannelOutcome::ApprovedAlways => terminal::accept_bot(
                &self.state,
                fingerprint,
                outcome == ChannelOutcome::ApprovedAlways,
            )?,
            ChannelOutcome::Denied
            | ChannelOutcome::Cancelled
            | ChannelOutcome::TimedOut
            | ChannelOutcome::AmbiguousDelivery => {
                terminal::remove_pending(&self.state, fingerprint)?
            }
            ChannelOutcome::Unsupported
                if capabilities.availability(ApprovalChannel::OwnerTerminal)
                    == ChannelAvailability::Available =>
            {
                return terminal::outcome(&self.state, fingerprint, tool, args);
            }
            ChannelOutcome::Unsupported => {}
        }
        Ok(outcome)
    }

    pub(crate) fn observe_approval_account(
        &self,
        account: &str,
        availability: channels::ChannelAvailability,
    ) {
        if let Some(bot) = &self.bot {
            bot.observe_account(account, availability);
        }
    }

    pub(crate) fn authorize_folder(
        &self,
        request_id: &str,
        account: &str,
        path: &std::path::Path,
        scope: folders::FolderScope,
        once: Option<&folders::TemporaryFolderGrant>,
    ) -> Result<Option<std::path::PathBuf>> {
        let store = folders::FolderGrants::new(&self.state, self.state.installation_root())?;
        let resolved = store.resolve_for_call(path)?;
        if self
            .state
            .free_folders()
            .iter()
            .any(|root| folders::within(&resolved, root))
        {
            return Ok(Some(resolved));
        }
        let granted = store.authorize(request_id, account, &resolved, scope, once)?;
        if granted.is_some() || request_id.is_empty() || once.is_some() {
            return Ok(granted);
        }
        let one_call = self.temporary_folder_access(
            request_id,
            account,
            &[&resolved],
            scope,
            channels::ChannelOutcome::Unsupported,
        )?;
        store.authorize(request_id, account, &resolved, scope, one_call.as_ref())
    }

    pub(crate) fn temporary_folder_access(
        &self,
        request_id: &str,
        account: &str,
        paths: &[&std::path::Path],
        scope: folders::FolderScope,
        outcome: channels::ChannelOutcome,
    ) -> Result<Option<folders::TemporaryFolderGrant>> {
        let store = folders::FolderGrants::new(&self.state, self.state.installation_root())?;
        if outcome == channels::ChannelOutcome::ApprovedAlways {
            let [path] = paths else {
                bail!("Permanent permission requires one canonical folder");
            };
            let resolved = store.resolve_for_call(path)?;
            let parent = resolved
                .parent()
                .context("An approved file needs a parent folder")?;
            store.grant_always(account, parent, scope)?;
            return Ok(None);
        }
        let now = unix_time();
        if outcome == channels::ChannelOutcome::ApprovedOnce
            || (outcome.may_fall_through()
                && bypass::BypassState::read(&self.state, now).is_active(now))
        {
            return store.once(request_id, account, paths, scope).map(Some);
        }
        Ok(None)
    }

    pub(crate) fn cancel_pending_request(&self, request_id: &str, fingerprint: Option<&str>) {
        let _ = self
            .seals
            .close(request_id, channels::ChannelOutcome::Cancelled);
        let pending_key = fingerprint.unwrap_or(request_id);
        let _ = terminal::remove_pending(&self.state, pending_key);
        if let Some(fp) = fingerprint {
            if fp != request_id {
                let _ = terminal::remove_pending(&self.state, request_id);
            }
            let _ = self
                .state
                .update_json("approval-grants.json", json!({}), |grants| {
                    if let Some(map) = grants.as_object_mut() {
                        map.remove(fp);
                    }
                    Ok(())
                });
        }
    }
}

pub(crate) fn unix_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
