use super::channels::{ApprovalChannel, ChannelOutcome, PendingRequest, ReplyBinding, valid_code};
use crate::state::State;
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

const FILE: &str = "sealed.json";
pub const HIDDEN_REQUEST: &str = "[approval request - hidden]";
pub const HIDDEN_CODE: &str = "[hidden]";

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Seals {
    codes: BTreeSet<String>,
    accounts: BTreeMap<String, AccountSeals>,
    #[serde(default)]
    redaction_material: BTreeSet<String>,
    #[serde(default)]
    pending_request: BTreeMap<String, PendingRequest>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AccountSeals {
    #[serde(default)]
    self_peer: Option<i64>,
    #[serde(default, alias = "selves", skip_serializing_if = "BTreeSet::is_empty")]
    legacy_peers: BTreeSet<String>,
    #[serde(alias = "messages")]
    message_ids: BTreeSet<i32>,
}

#[derive(Default)]
struct LiveRequests {
    ids: BTreeSet<String>,
    uncertain: bool,
}

pub struct SealedStore {
    state: Arc<State>,
    live: Mutex<LiveRequests>,
}

impl SealedStore {
    /// Keep one instance for the service. Restart keeps historical seals, not live approvals.
    pub fn new(state: Arc<State>) -> Self {
        Self {
            state,
            live: Mutex::default(),
        }
    }

    fn load_unlocked(&self) -> Result<Seals> {
        let empty = json!({"codes":[],"accounts":{}});
        // Read the former Python store only until the first native write migrates it.
        let raw = if self.state.root().join(FILE).try_exists()? {
            self.state.read_json_unlocked(FILE, empty)?
        } else {
            self.state
                .read_json_unlocked("approval-messages.json", empty)?
        };
        let mut seals: Seals =
            serde_json::from_value(raw).context("Sealed approval store is invalid")?;
        ensure!(
            seals.codes.iter().all(|code| valid_code(code)),
            "Invalid sealed approval code"
        );
        seals.codes = std::mem::take(&mut seals.codes)
            .into_iter()
            .map(|code| code.to_ascii_uppercase())
            .collect();
        let mut accounts = BTreeMap::new();
        for (label, mut entry) in std::mem::take(&mut seals.accounts) {
            // The Python default account used the empty string as its store key.
            let label = if label.is_empty() {
                "default".to_owned()
            } else {
                label.to_ascii_lowercase()
            };
            crate::session::validate_label(&label)?;
            ensure!(
                entry.self_peer.is_none_or(|peer| peer > 0)
                    && entry.message_ids.iter().all(|&id| id > 0),
                "Invalid sealed message identity"
            );
            entry.legacy_peers = entry
                .legacy_peers
                .iter()
                .map(|peer| peer_key(peer))
                .collect();
            if entry.self_peer.is_none() {
                let mut numeric = entry
                    .legacy_peers
                    .iter()
                    .filter_map(|peer| peer.parse::<i64>().ok().filter(|&id| id > 0));
                entry.self_peer = numeric.next().filter(|_| numeric.next().is_none());
            }
            ensure!(
                accounts.insert(label, entry).is_none(),
                "Ambiguous sealed account"
            );
        }
        seals.accounts = accounts;
        ensure!(
            seals.redaction_material.iter().all(|body| !body.is_empty())
                && seals.pending_request.iter().all(|(id, pending)| {
                    id == pending.request_id()
                        && pending.valid()
                        && seals.codes.contains(&pending.code().to_ascii_uppercase())
                        && (pending.channel() != ApprovalChannel::SavedMessagesChannel
                            || pending.unbound()
                            || seals
                                .accounts
                                .get(pending.account())
                                .is_some_and(|entry| entry.covers(pending)))
                }),
            "Invalid sealed approval binding"
        );
        Ok(seals)
    }

    fn modify<T>(
        &self,
        change: impl FnOnce(&mut Seals, &mut LiveRequests) -> Result<T>,
    ) -> Result<T> {
        let mut live = self
            .live
            .lock()
            .map_err(|_| anyhow::anyhow!("Sealed approval lock is unavailable"))?;
        ensure!(!live.uncertain, "Sealed approval storage is uncertain");
        let _lock = self.state.state_lock()?;
        let mut seals = self.load_unlocked()?;
        let result = change(&mut seals, &mut live)?;
        if let Err(error) = self
            .state
            .write_json_unlocked(FILE, &serde_json::to_value(seals)?)
        {
            live.uncertain = true;
            live.ids.clear();
            return Err(error);
        }
        Ok(result)
    }

    /// Save the code, body and pending binding before showing any prompt.
    pub fn begin(&self, pending: PendingRequest, bodies: &[&str]) -> Result<()> {
        self.modify(|seals, live| {
            ensure!(
                pending.valid() && !pending.is_closed(),
                "Invalid approval request binding"
            );
            ensure!(
                !pending.channel().uses_message() || pending.unbound(),
                "Approval was already delivered"
            );
            ensure!(
                (pending.channel() != ApprovalChannel::SavedMessagesChannel || !bodies.is_empty())
                    && bodies
                        .iter()
                        .all(|body| !body.is_empty() && body.len() <= 32_768),
                "Invalid sealed approval body"
            );
            let request_id = pending.request_id().to_owned();
            ensure!(
                !seals.pending_request.contains_key(&request_id),
                "Approval request already exists"
            );
            ensure!(
                seals.codes.insert(pending.code().to_owned()),
                "Approval code has already been issued"
            );
            seals
                .redaction_material
                .extend(bodies.iter().map(|body| (*body).to_owned()));
            seals.pending_request.insert(request_id.clone(), pending);
            live.ids.insert(request_id);
            Ok(())
        })
    }

    pub fn bind_message(&self, delivered: ReplyBinding<'_>, now: u64) -> Result<()> {
        self.modify(|seals, live| {
            ensure!(
                live.ids.contains(delivered.request_id),
                "Approval request is no longer live"
            );
            let pending = seals
                .pending_request
                .get_mut(delivered.request_id)
                .context("Approval request is no longer open")?;
            let message = pending.bind_message(delivered, now)?;
            if pending.channel() == ApprovalChannel::SavedMessagesChannel {
                let entry = seals
                    .accounts
                    .entry(pending.account().to_owned())
                    .or_default();
                ensure!(
                    entry.self_peer.is_none() || entry.self_peer == pending.self_peer(),
                    "Saved Messages peer does not match its seal record"
                );
                entry.self_peer = pending.self_peer();
                entry.message_ids.insert(message);
            }
            Ok(())
        })
    }

    /// A result is actionable only after its exact live binding is closed on disk.
    pub fn consume(
        &self,
        reply: ReplyBinding<'_>,
        outcome: ChannelOutcome,
        now: u64,
    ) -> Result<bool> {
        ensure!(outcome.is_owner_reply(), "Invalid owner approval response");
        {
            let live = self
                .live
                .lock()
                .map_err(|_| anyhow::anyhow!("Sealed approval lock is unavailable"))?;
            ensure!(!live.uncertain, "Sealed approval storage is uncertain");
            if !live.ids.contains(reply.request_id) {
                return Ok(false);
            }
        }
        self.modify(|seals, live| {
            if !live.ids.contains(reply.request_id)
                || !seals
                    .pending_request
                    .get(reply.request_id)
                    .is_some_and(|pending| pending.matches_reply(reply, now))
            {
                return Ok(false);
            }
            if let Some(pending) = seals.pending_request.get_mut(reply.request_id) {
                pending.close();
            }
            live.ids.remove(reply.request_id);
            Ok(true)
        })
    }

    /// Call with `Unsupported` only when the transport guarantees it did not dispatch.
    pub fn close(&self, request_id: &str, outcome: ChannelOutcome) -> Result<()> {
        self.modify(|seals, live| {
            if !live.ids.remove(request_id) {
                return Ok(());
            }
            if let Some(pending) = seals.pending_request.get_mut(request_id) {
                if pending.unbound() && !outcome.may_fall_through() {
                    pending.close();
                } else {
                    seals.pending_request.remove(request_id);
                }
            }
            Ok(())
        })
    }

    fn readable(&self) -> Result<Seals> {
        let live = self
            .live
            .lock()
            .map_err(|_| anyhow::anyhow!("Sealed approval lock is unavailable"))?;
        ensure!(!live.uncertain, "Sealed approval storage is uncertain");
        let _lock = self.state.state_lock()?;
        self.load_unlocked()
    }

    #[cfg(test)]
    pub fn codes(&self) -> Result<BTreeSet<String>> {
        Ok(self.readable()?.codes)
    }

    #[cfg(test)]
    /// Pass a canonical account and a verified peer (numeric ID, username or self alias).
    pub fn is_sealed_message(&self, account: &str, peer: &str, message_id: i32) -> bool {
        self.readable()
            .map_or(true, |seals| seals.has_message(account, peer, message_id))
    }

    pub fn is_sealed_target(&self, account: &str, arguments: &Value) -> bool {
        self.readable().map_or(true, |seals| {
            let mut peers = BTreeSet::new();
            let mut ids = BTreeSet::new();
            collect_targets(arguments, false, false, &mut peers, &mut ids);
            peers.iter().any(|peer| {
                seals.unbound_self(account, peer)
                    || ids.iter().any(|id| seals.has_message(account, peer, *id))
            }) || contains_secret(arguments, &seals)
        })
    }

    pub(super) fn seal_prompt(&self, code: &str, text: &str) -> Result<()> {
        ensure!(valid_code(code), "Invalid sealed approval code");
        self.modify(|seals, _| {
            ensure!(
                seals.codes.insert(code.to_ascii_uppercase()),
                "Approval code has already been issued"
            );
            seals.redaction_material.insert(text.to_owned());
            Ok(())
        })
    }

    pub(super) fn redact_result(&self, account: &str, value: &Value) -> Value {
        self.readable().map_or_else(
            |_| json!(HIDDEN_REQUEST),
            |seals| seals.redact_result(account, value, None),
        )
    }

    pub fn redact_text(&self, text: &str) -> String {
        self.readable().map_or_else(
            |_| HIDDEN_REQUEST.to_owned(),
            |seals| seals.redact_text(text),
        )
    }

    pub fn redact_value(&self, value: &Value) -> Value {
        self.readable()
            .map_or_else(|_| json!(HIDDEN_REQUEST), |seals| seals.redact_value(value))
    }
}

impl AccountSeals {
    fn covers(&self, pending: &PendingRequest) -> bool {
        self.self_peer == pending.self_peer() && pending.message_ids().is_subset(&self.message_ids)
    }

    fn matches_peer(&self, peer: &str) -> bool {
        self_alias(peer)
            || self.self_peer.is_some_and(|id| peer == id.to_string())
            || self.legacy_peers.contains(peer)
    }
}

fn peer_key(peer: &str) -> String {
    peer.trim().trim_start_matches('@').to_lowercase()
}

fn message_peer(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| {
        value.as_i64().map(|id| id.to_string()).or_else(|| {
            ["user_id", "channel_id", "chat_id", "id", "username"]
                .iter()
                .find_map(|key| value.get(*key).and_then(message_peer))
        })
    })
}

fn self_alias(peer: &str) -> bool {
    ["me", "self", "saved", "savedmessages", "saved messages"].contains(&peer)
}

impl Seals {
    fn unbound_self(&self, account: &str, peer: &str) -> bool {
        let peer = peer_key(peer);
        self.pending_request.values().any(|pending| {
            pending.channel() == ApprovalChannel::SavedMessagesChannel
                && pending.unbound()
                && pending.account().eq_ignore_ascii_case(account)
                && (self_alias(&peer)
                    || pending.self_peer().is_some_and(|id| peer == id.to_string())
                    || self
                        .accounts
                        .get(pending.account())
                        .is_some_and(|known| known.matches_peer(&peer)))
        })
    }

    fn has_message(&self, account: &str, peer: &str, message_id: i32) -> bool {
        let peer = peer_key(peer);
        self.unbound_self(account, &peer)
            || self
                .accounts
                .get(&account.to_ascii_lowercase())
                .is_some_and(|entry| {
                    entry.matches_peer(&peer) && entry.message_ids.contains(&message_id)
                })
    }

    fn redact_text(&self, text: &str) -> String {
        static LEGACY_BODY: OnceLock<Option<Regex>> = OnceLock::new();
        static LEGACY_HEAD: OnceLock<Option<Regex>> = OnceLock::new();
        let mut text = text.to_owned();
        let mut bodies: Vec<_> = self.redaction_material.iter().collect();
        bodies.sort_by_key(|body| std::cmp::Reverse(body.len()));
        for body in bodies {
            text = text.replace(body.as_str(), HIDDEN_REQUEST);
            if let Ok(escaped) = serde_json::to_string(body) {
                text = text.replace(&escaped[1..escaped.len() - 1], HIDDEN_REQUEST);
            }
        }
        if let Some(pattern) = LEGACY_BODY.get_or_init(|| {
            Regex::new(r"(?s)Approval [A-Za-z0-9]{4}: .*?from another device\.").ok()
        }) {
            text = pattern.replace_all(&text, HIDDEN_REQUEST).into_owned();
        }
        if let Some(pattern) =
            LEGACY_HEAD.get_or_init(|| Regex::new(r"Approval [A-Za-z0-9]{4}:").ok())
        {
            text = pattern
                .replace_all(&text, "Approval [hidden]:")
                .into_owned();
        }
        let mut output = String::with_capacity(text.len());
        let mut start = 0;
        for (index, ch) in text.char_indices() {
            if !ch.is_ascii_alphanumeric() {
                self.push_token(&mut output, &text[start..index]);
                output.push(ch);
                start = index + ch.len_utf8();
            }
        }
        self.push_token(&mut output, &text[start..]);
        output
    }

    fn push_token(&self, output: &mut String, token: &str) {
        if valid_code(token) && self.codes.contains(&token.to_ascii_uppercase()) {
            output.push_str(HIDDEN_CODE);
        } else {
            output.push_str(token);
        }
    }

    fn redact_result(&self, account: &str, value: &Value, peer: Option<&str>) -> Value {
        match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.redact_result(account, item, peer))
                    .collect(),
            ),
            Value::Object(map) => {
                let named_peer = ["chat_id", "peer_id", "chat", "peer", "conversation_id"]
                    .iter()
                    .find_map(|key| map.get(*key).and_then(message_peer));
                let peer = named_peer.as_deref().or(peer);
                let id = map
                    .get("message_id")
                    .or_else(|| map.get("id"))
                    .and_then(|id| id.as_i64().or_else(|| id.as_str()?.parse().ok()))
                    .and_then(|id| i32::try_from(id).ok());
                if peer.is_some_and(|peer| {
                    id.is_some_and(|id| self.has_message(account, peer, id))
                        || self.unbound_self(account, peer)
                }) {
                    return json!(HIDDEN_REQUEST);
                }
                self.redact_object(map, |item| self.redact_result(account, item, peer))
            }
            _ => self.redact_value(value),
        }
    }

    fn redact_object(
        &self,
        map: &serde_json::Map<String, Value>,
        redact: impl Fn(&Value) -> Value,
    ) -> Value {
        let mut cleaned = serde_json::Map::new();
        for (key, item) in map {
            let redacted = self.redact_text(key);
            let mut label = redacted.clone();
            let mut suffix = 1;
            while cleaned.contains_key(&label) || (label != *key && map.contains_key(&label)) {
                suffix += 1;
                label = format!("{redacted} ({suffix})");
            }
            cleaned.insert(label, redact(item));
        }
        Value::Object(cleaned)
    }

    fn redact_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => json!(self.redact_text(text)),
            Value::Number(number) if self.codes.contains(&number.to_string()) => json!(HIDDEN_CODE),
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| self.redact_value(v)).collect())
            }
            Value::Object(map) => self.redact_object(map, |item| self.redact_value(item)),
            _ => value.clone(),
        }
    }
}

fn contains_secret(value: &Value, seals: &Seals) -> bool {
    match value {
        Value::String(text) => seals.redact_text(text) != *text,
        Value::Number(number) => seals.codes.contains(&number.to_string()),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| seals.redact_text(key) != *key || contains_secret(value, seals)),
        Value::Array(items) => items.iter().any(|value| contains_secret(value, seals)),
        _ => false,
    }
}

fn collect_targets(
    value: &Value,
    peer: bool,
    id: bool,
    peers: &mut BTreeSet<String>,
    ids: &mut BTreeSet<i32>,
) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_ascii_lowercase();
                let peer = peer
                    || ["chat", "peer", "entity", "username", "user"]
                        .iter()
                        .any(|part| key.contains(part));
                let id = id
                    || ["message", "reply_to", "msg_id"]
                        .iter()
                        .any(|part| key.contains(part));
                collect_targets(value, peer, id, peers, ids);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_targets(item, peer, id, peers, ids);
            }
        }
        Value::String(_) | Value::Number(_) if peer || id => {
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            if peer {
                peers.insert(peer_key(&text));
            }
            if id && let Ok(number) = text.trim().parse() {
                ids.insert(number);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "../../tests/unit/safeguard_sealed.rs"]
mod tests;
