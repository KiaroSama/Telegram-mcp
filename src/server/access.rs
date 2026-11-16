//! Chat and forum-topic restrictions ported from the latest upstream policy.
use crate::backend::Telegram;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
#[path = "../../tests/unit/access.rs"]
mod tests;

#[derive(Default, Clone)]
pub struct AccessPolicy {
    enabled: bool,
    chats: BTreeMap<String, Option<BTreeSet<i64>>>,
}
fn key(text: &str) -> String {
    text.trim().trim_start_matches('@').to_ascii_lowercase()
}
pub fn marked_peer(peer: &Value) -> Option<String> {
    let integer = |k: &str| {
        peer[k]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| peer[k].as_i64())
    };
    match peer["_"].as_str()? {
        "inputPeerUser" | "peerUser" => Some(integer("user_id")?.to_string()),
        "inputPeerChat" | "peerChat" => Some((-integer("chat_id")?).to_string()),
        "inputPeerChannel" | "peerChannel" => {
            Some((-1_000_000_000_000i64 - integer("channel_id")?).to_string())
        }
        _ => None,
    }
}
fn row_identity(row: &Value) -> Option<String> {
    let id = || {
        row["id"]
            .as_i64()
            .or_else(|| row["id"].as_str().and_then(|id| id.parse().ok()))
    };
    match row["_"].as_str().unwrap_or_default() {
        "user" | "userEmpty" => Some(id()?.to_string()),
        "chat" | "chatForbidden" => Some((-id()?).to_string()),
        "channel" | "channelForbidden" | "community" => {
            Some((-1_000_000_000_000i64 - id()?).to_string())
        }
        "dialog" => marked_peer(&row["peer"]),
        "message" | "messageService" => marked_peer(&row["peer_id"]),
        _ => None,
    }
}
impl AccessPolicy {
    pub fn from_environment() -> Result<Self> {
        Self::parse(std::env::var("TELEGRAM_ALLOWED_CHAT_IDS").ok().as_deref())
    }
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
            return Ok(Self::default());
        };
        let mut policy = Self {
            enabled: true,
            chats: BTreeMap::new(),
        };
        for item in raw.split(',') {
            let item = item.trim();
            if item.is_empty() {
                bail!("Empty chat allowlist entry");
            }
            let (chat, topic) = if let Some((chat, topic)) = item.rsplit_once(':') {
                let topic = topic
                    .parse::<i64>()
                    .context("Topic allowlist id must be a positive integer")?;
                if topic < 1 {
                    bail!("Topic ids must be positive");
                }
                (chat, Some(topic))
            } else {
                (item, None)
            };
            let chat = key(chat);
            if chat.is_empty() {
                bail!("Chat allowlist entry has no chat");
            }
            let entry = policy
                .chats
                .entry(chat)
                .or_insert_with(|| Some(BTreeSet::new()));
            match topic {
                None => *entry = None,
                Some(id) => {
                    if let Some(ids) = entry {
                        ids.insert(id);
                    }
                }
            }
        }
        Ok(policy)
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn allows_event(&self, chat: &str) -> bool {
        !self.enabled || self.chats.get(&key(chat)).is_some_and(Option::is_none)
    }
    pub fn allows_event_peer(&self, chat: &str, username: Option<&str>) -> bool {
        self.allows_event(chat)
            || username
                .is_some_and(|username| self.chats.get(&key(username)).is_some_and(Option::is_none))
    }
    pub fn allows_message_event(
        &self,
        chat: &str,
        username: Option<&str>,
        topic: Option<i64>,
    ) -> bool {
        if !self.enabled {
            return true;
        }
        [Some(chat), username].into_iter().flatten().any(|name| {
            self.chats.get(&key(name)).is_some_and(|scope| {
                scope
                    .as_ref()
                    .is_none_or(|topics| topic.is_some_and(|id| topics.contains(&id)))
            })
        })
    }
    pub async fn check(&self, name: &str, args: &Value, api: &dyn Telegram) -> Result<()> {
        self.scoped(name, args, api).await.map(|_| ())
    }
    /// Numeric aliases exist only in this operation's clone, and only after a
    /// trusted resolver proved an originally permitted target. Response fields
    /// and bindings from another account/request cannot widen this scope.
    pub async fn scoped(&self, name: &str, args: &Value, api: &dyn Telegram) -> Result<Self> {
        let mut scoped = self.clone();
        if !self.enabled {
            return Ok(scoped);
        }
        if name == "add_contact" && args.get("username").is_none_or(|value| value.is_null()) {
            bail!(
                "Phone-only contact import cannot prove a permitted identity while a chat allowlist is active"
            );
        }
        let has_primary = [
            "chat",
            "chat_id",
            "from_chat_id",
            "to_chat_id",
            "channel",
            "channel_id",
            "group_id",
            "community",
            "first_chat",
            "source_chat_id",
            "destination_chat_id",
        ]
        .iter()
        .any(|field| args.get(*field).is_some_and(|value| !value.is_null()));
        for field in [
            "chat",
            "chat_id",
            "from_chat_id",
            "to_chat_id",
            "channel",
            "channel_id",
            "group_id",
            "community",
            "first_chat",
            "source_chat_id",
            "destination_chat_id",
            "contact_id",
            "user_id",
            "user",
            "bot",
            "bot_username",
            "from_user",
            "peer_id",
        ] {
            if has_primary && matches!(field, "contact_id" | "user_id" | "user" | "from_user") {
                continue;
            }
            let Some(value) = args.get(field).filter(|v| !v.is_null()) else {
                continue;
            };
            let literal = value.as_str().map(key).unwrap_or_else(|| value.to_string());
            let resolved = api.resolve_peer(value).await?;
            let marked = marked_peer(&resolved).or_else(|| api.pinned_identity(&resolved));
            let entry = self
                .scope_for(&literal, marked.as_deref())
                .context("Chat is outside TELEGRAM_ALLOWED_CHAT_IDS")?;
            if let Some(topics) = &entry {
                let topic = args
                    .get("topic_id")
                    .or_else(|| args.get("top_msg_id"))
                    .and_then(|v| {
                        v.as_i64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    });
                if !topic.is_some_and(|id| topics.contains(&id)) {
                    bail!(
                        "This chat is restricted to specific forum topics; an allowed topic_id is required"
                    );
                }
                if !matches!(
                    name,
                    "get_topic_messages"
                        | "get_history"
                        | "get_messages"
                        | "list_messages"
                        | "search_messages"
                        | "send_message"
                        | "send_file"
                        | "send_photo"
                        | "send_video"
                        | "send_voice"
                        | "send_audio"
                        | "reply_to_message"
                        | "get_messages_after"
                        | "inspect_document"
                        | "transcribe_voice"
                        | "mark_topic_as_read"
                ) {
                    bail!("This operation cannot enforce a topic restriction safely");
                }
                let reply_field = if name == "reply_to_message" {
                    Some("message_id")
                } else if name == "send_message"
                    && args
                        .get("reply_to_message_id")
                        .is_some_and(|id| !id.is_null())
                {
                    Some("reply_to_message_id")
                } else {
                    None
                };
                if let Some(field) = reply_field {
                    let message_id = crate::tools::messaging::helpers::id(args, field)?;
                    let message =
                        crate::tools::messaging::helpers::fetch(api, &resolved, message_id).await?;
                    crate::documents::check_message_topic(
                        api,
                        &resolved,
                        &message,
                        topic.expect("The permitted topic was checked above"),
                    )
                    .await?;
                }
            }
            scoped.bind(marked, &entry);
        }
        for field in ["chat_ids", "peers", "user_ids"] {
            if has_primary && field == "user_ids" {
                continue;
            }
            if let Some(values) = args.get(field).and_then(Value::as_array) {
                for value in values {
                    let resolved = api.resolve_peer(value).await?;
                    let literal = value.as_str().map(key).unwrap_or_else(|| value.to_string());
                    let marked = marked_peer(&resolved).or_else(|| api.pinned_identity(&resolved));
                    let entry = self.scope_for(&literal, marked.as_deref());
                    if !entry.as_ref().is_some_and(Option::is_none) {
                        bail!("A peer list contains a chat outside its permitted whole-chat scope");
                    }
                    scoped.bind(marked, &entry.expect("Whole-chat scope was checked"));
                }
            }
        }
        if matches!(
            name,
            "resolve_username"
                | "lookup_username"
                | "lookup_peer"
                | "get_full_user"
                | "add_contact"
        ) && let Some(value) = args
            .get("username")
            .or_else(|| args.get("query"))
            .filter(|value| !value.is_null())
        {
            let normalized = if name == "lookup_peer" {
                crate::tools::identity::normalize_lookup_query(value)?
            } else {
                value.clone()
            };
            let value = &normalized;
            let literal = value.as_str().map(key).unwrap_or_else(|| value.to_string());
            let resolved = api.resolve_peer(value).await?;
            let marked = marked_peer(&resolved).or_else(|| api.pinned_identity(&resolved));
            let entry = self
                .scope_for(&literal, marked.as_deref())
                .context("Peer lookup is outside the configured chat scope")?;
            scoped.bind(marked, &entry);
        }
        if matches!(
            name,
            "search_global"
                | "search_all_messages"
                | "list_calls"
                | "get_recent_actions"
                | "export_recent_actions"
                | "get_folder_snapshot"
                | "list_folders"
                | "get_folder"
                | "update_folder"
                | "get_contact_ids"
                | "get_blocked_users"
                | "import_contacts"
                | "resolve_phone"
                | "enable_incoming_feed"
                | "incoming_feed_status"
        ) {
            bail!("This account-wide operation is unavailable while a chat allowlist is active");
        }
        if name == "ban_community_member" && args["also_from_chats"] == true {
            bail!("Linked-chat community mutations cannot enforce the configured chat allowlist");
        }
        // Listing has no primary target. Resolve configured username scopes
        // using the backend, rather than trusting a username in arbitrary rows.
        if matches!(
            name,
            "list_contacts"
                | "export_contacts"
                | "search_contacts"
                | "list_chats"
                | "search_chats"
                | "get_contact_chats"
                | "get_direct_chat_by_contact"
        ) {
            for (alias, entry) in &self.chats {
                if alias.parse::<i64>().is_ok() {
                    continue;
                }
                if let Ok(peer) = api.resolve_peer(&json!(format!("@{alias}"))).await {
                    scoped.bind(marked_peer(&peer), entry);
                }
            }
        }
        Ok(scoped)
    }
    fn scope_for(&self, literal: &str, marked: Option<&str>) -> Option<Option<BTreeSet<i64>>> {
        match (
            self.chats.get(literal),
            marked.and_then(|id| self.chats.get(id)),
        ) {
            (None, None) => None,
            (Some(None), _) | (_, Some(None)) => Some(None),
            (Some(Some(left)), Some(Some(right))) => {
                Some(Some(left.union(right).copied().collect()))
            }
            (Some(scope), None) | (None, Some(scope)) => Some(scope.clone()),
        }
    }
    fn bind(&mut self, id: Option<String>, scope: &Option<BTreeSet<i64>>) {
        let Some(id) = id else {
            return;
        };
        match self.chats.entry(id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(scope.clone());
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                match (entry.get_mut(), scope) {
                    (current, None) => *current = None,
                    (Some(current), Some(extra)) => current.extend(extra),
                    (None, Some(_)) => {}
                }
            }
        }
    }
    pub fn filter(&self, value: &mut Value) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.filter_value(value, 0);
        if let Some(object) = value.as_object_mut() {
            object.insert("access_filtered".into(), json!(true));
        }
        Ok(())
    }
    pub fn filter_for(&self, name: &str, value: &mut Value) -> Result<()> {
        // The event ring filters immutable metadata using both marked peer and
        // username/topic before projecting public fields. Re-filtering that
        // projection would discard valid username-scoped events.
        if matches!(
            name,
            "wait_for_events" | "wait_for_new_message" | "wait_for_settled_message"
        ) {
            if self.enabled {
                value["access_filtered"] = json!(true);
            }
            return Ok(());
        }
        if self.enabled
            && matches!(
                name,
                "list_contacts"
                    | "export_contacts"
                    | "search_contacts"
                    | "list_chats"
                    | "search_chats"
                    | "get_contact_chats"
                    | "get_direct_chat_by_contact"
            )
        {
            let rows = if value.is_array() {
                value.as_array_mut()
            } else {
                value.get_mut("results").and_then(Value::as_array_mut)
            };
            if let Some(rows) = rows {
                rows.retain(|row| {
                    let id = row_identity(row).unwrap_or_else(|| {
                        row["id"]
                            .as_str()
                            .map(key)
                            .unwrap_or_else(|| row["id"].to_string())
                    });
                    self.chats.contains_key(&id)
                });
            }
        }
        self.filter(value)
    }
    fn filter_value(&self, value: &mut Value, depth: usize) {
        if depth > 64 {
            *value = Value::Null;
            return;
        }
        match value {
            Value::Array(values) => {
                values.retain(|value| {
                    if let Some(peer) = value.get("peer_id")
                        && let Some(id) = marked_peer(peer)
                    {
                        return self.chats.contains_key(&id);
                    }
                    if let Some(id) = value.get("chat_id").filter(|id| !id.is_null()) {
                        return self
                            .chats
                            .contains_key(&id.as_str().map(key).unwrap_or_else(|| id.to_string()));
                    }
                    true
                });
                for value in values {
                    self.filter_value(value, depth + 1);
                }
            }
            Value::Object(object) => {
                for (name, values) in object.iter_mut() {
                    if matches!(name.as_str(), "chats" | "dialogs")
                        && let Some(rows) = values.as_array_mut()
                    {
                        rows.retain(|row| {
                            let id = row_identity(row).unwrap_or_else(|| {
                                row["id"]
                                    .as_str()
                                    .map(key)
                                    .unwrap_or_else(|| row["id"].to_string())
                            });
                            self.chats.contains_key(&id)
                        });
                    }
                    self.filter_value(values, depth + 1);
                }
            }
            _ => {}
        }
    }
}
