//! Per-operation private peer references shared by approval, policy and dispatch.
use super::*;
use std::path::Path;

#[derive(Clone)]
struct BoundApi {
    inner: Arc<dyn Telegram>,
    peers: Vec<(Value, String, Value)>,
    operation_guard: Option<Arc<dyn Send + Sync>>,
}

pub(super) fn with_operation_guard(
    inner: Arc<dyn Telegram>,
    operation_guard: Arc<dyn Send + Sync>,
) -> Arc<dyn Telegram> {
    Arc::new(BoundApi {
        inner,
        peers: vec![],
        operation_guard: Some(operation_guard),
    })
}

pub(super) const SECRET_CHAT_PREFIX: &str = "secret/";

pub(super) fn secret_cloud_field(name: &str) -> Option<&'static str> {
    match name {
        "create_secret_chat" => Some("user_id"),
        "copy_into_secret_chat" => Some("from_chat_id"),
        "export_secret_chat" => Some("peer_user"),
        _ => None,
    }
}

pub(super) fn secret_chat_fields(name: &str) -> &'static [&'static str] {
    match name {
        "clear_secret_history" => &["chat_id", "confirm_chat_id"],
        "copy_into_secret_chat" => &["to_chat_id"],
        "forward_secret_media" => &["from_chat_id", "to_chat_id"],
        "delete_secret_message"
        | "mark_secret_read"
        | "read_secret_messages"
        | "save_secret_media"
        | "search_secret_messages"
        | "secret_chat_status"
        | "send_secret_media"
        | "send_secret_message"
        | "send_secret_typing"
        | "send_timed_secret_media"
        | "send_timed_secret_message"
        | "set_secret_chat_timer" => &["chat_id"],
        "acknowledge_secret_delivery"
        | "close_secret_chat"
        | "delete_saved_secret_messages"
        | "delete_secret_chat"
        | "delete_secret_chat_both_sides"
        | "export_secret_chat"
        | "forget_secret_chat"
        | "get_secret_pause_status"
        | "list_secret_deliveries"
        | "pause_secret_chat"
        | "receive_secret_media"
        | "resume_secret_chat" => &["secret_chat_id"],
        _ => &[],
    }
}

pub(super) fn wire_secret_chat_id(value: &Value) -> Option<i32> {
    crate::secret::wire_chat_id(value.as_i64()?)
}

pub(super) async fn bind(
    name: &str,
    inner: Arc<dyn Telegram>,
    args: &Value,
) -> Result<(Arc<dyn Telegram>, Value)> {
    let mut bound = BoundApi {
        inner,
        peers: vec![],
        operation_guard: None,
    };
    let mut approval = args.clone();
    let mut originals = vec![];
    let mut selectors = Vec::new();
    if crate::tools::secret::TOOL_NAMES.contains(&name) {
        if let Some(field) = secret_cloud_field(name) {
            selectors.push((format!("/{field}"), false));
        }
        for &field in secret_chat_fields(name) {
            if let Some(value) = args.get(field).filter(|v| !v.is_null()) {
                let id = wire_secret_chat_id(value).with_context(|| {
                    format!("{field} must identify a non-zero signed 32-bit secret chat")
                })?;
                // Approval identities are namespaced; dispatch still gets the original IDs.
                approval[field] = json!(format!("{SECRET_CHAT_PREFIX}{id}"));
            }
        }
    } else {
        for field in [
            "chat",
            "chat_id",
            "to_chat_id",
            "from_chat_id",
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
            "user_ids",
            "chat_ids",
            "peers",
        ] {
            selectors.push((
                format!("/{field}"),
                matches!(field, "user_ids" | "chat_ids" | "peers"),
            ));
        }
    }
    if matches!(
        name,
        "get_full_user" | "resolve_username" | "lookup_username" | "add_contact"
    ) {
        selectors.push(("/username".into(), false));
    }
    if name == "lookup_peer" {
        selectors.push(("/query".into(), false));
    }
    if name == "add_proxies" {
        selectors.push(("/source".into(), false));
    }
    if name == "delete_revoked_invite_links" {
        selectors.push(("/admin".into(), false));
    }
    if name == "export_recent_actions" {
        selectors.push(("/admins".into(), true));
    }
    if name == "update_folder" {
        for field in ["include_chat_ids", "pinned_chat_ids", "exclude_chat_ids"] {
            selectors.push((format!("/patch/{field}"), true));
        }
    }
    if name == "set_privacy_settings" {
        selectors.extend([
            ("/allow_users".into(), true),
            ("/disallow_users".into(), true),
        ]);
        if let Some(rules) = args["rules"].as_array() {
            for (index, rule) in rules.iter().enumerate() {
                if matches!(
                    rule["rule"].as_str(),
                    Some("users_allowed" | "users_disallowed")
                ) {
                    selectors.push((format!("/rules/{index}/users"), true));
                }
            }
        }
    }
    for (pointer, list) in selectors {
        let Some(value) = args.pointer(&pointer).filter(|v| !v.is_null()) else {
            continue;
        };
        let inputs: Vec<&Value> = if list {
            value
                .as_array()
                .context("Recipient list must be an array")?
                .iter()
                .collect()
        } else {
            vec![value]
        };
        let mut identities = vec![];
        for input in inputs {
            let normalized = if name == "lookup_peer" && pointer == "/query" {
                crate::tools::identity::normalize_lookup_query(input)?
            } else if name == "add_proxies" && pointer == "/source" {
                crate::tools::local::proxies::source_selector(input)?
            } else {
                input.clone()
            };
            let peer = bound.resolve_peer(&normalized).await?;
            let id = if peer["_"] == "inputPeerSelf" {
                let own = bound
                    .inner
                    .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
                    .await?;
                let rows = own
                    .as_array()
                    .context("Could not establish the authenticated recipient identity")?;
                let row = rows
                    .first()
                    .filter(|_| rows.len() == 1)
                    .filter(|row| row["_"] == "user")
                    .context("Could not establish the authenticated recipient identity")?;
                row["id"]
                    .as_str()
                    .and_then(|id| id.parse::<i64>().ok())
                    .or_else(|| row["id"].as_i64())
                    .filter(|id| *id > 0)
                    .context("The authenticated recipient identity is invalid")?
                    .to_string()
            } else {
                access::marked_peer(&peer)
                    .context("The recipient has no stable Telegram identity")?
            };
            bound.peers.push((input.clone(), id.clone(), peer.clone()));
            if normalized != *input {
                bound.peers.push((normalized, id.clone(), peer));
            }
            identities.push(json!(id));
            originals.push(input.clone());
        }
        *approval
            .pointer_mut(&pointer)
            .context("Bound recipient field is unavailable")? = if list {
            json!(identities)
        } else {
            identities.pop().expect("A scalar recipient was resolved")
        };
    }
    if !originals.is_empty() {
        // Preserve checks for literal approval-channel names and tainted input.
        // Clients cannot inject this field: their original schema was validated.
        approval["_requested_peer_targets"] = json!(originals);
    }
    Ok((Arc::new(bound), approval))
}

#[async_trait::async_trait]
impl Telegram for BoundApi {
    fn pinned_identity(&self, peer: &Value) -> Option<String> {
        self.peers
            .iter()
            .find(|(_, _, pinned)| pinned == peer)
            .map(|(_, id, _)| id.clone())
            .or_else(|| self.inner.pinned_identity(peer))
    }
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        self.inner.invoke(method, params).await
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        if let Some((_, _, peer)) = self.peers.iter().find(|(original, id, peer)| {
            original == input
                || peer == input
                || input.as_str() == Some(id.as_str())
                || input.as_i64().is_some_and(|n| n.to_string() == *id)
        }) {
            return Ok(peer.clone());
        }
        self.inner.resolve_peer(input).await
    }
    async fn upload(&self, path: &Path) -> Result<Value> {
        self.inner.upload(path).await
    }
    async fn download(&self, location: Value, path: &Path, max_bytes: u64) -> Result<Value> {
        self.inner.download(location, path, max_bytes).await
    }
    async fn download_memory(&self, location: Value, max_bytes: u64) -> Result<Vec<u8>> {
        self.inner.download_memory(location, max_bytes).await
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        self.inner.clone_handle().map(|inner| {
            Arc::new(Self {
                inner,
                peers: self.peers.clone(),
                operation_guard: None,
            }) as Arc<dyn Telegram>
        })
    }
    fn operation_guard(&self) -> Option<Arc<dyn Send + Sync>> {
        self.operation_guard.clone()
    }
    fn cache_key(&self) -> Option<String> {
        self.inner.cache_key()
    }
    fn alias_matches(&self, query: &str) -> Result<Value> {
        self.inner.alias_matches(query)
    }
}
