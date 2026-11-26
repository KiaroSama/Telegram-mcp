//! Scoped Ghost Mode handlers that resolve `chat_id` to canonical marked IDs
//! alongside raw input so `@username` and numeric ID overrides always match.
use crate::{backend::Telegram, state::State};
use anyhow::Result;
use serde_json::Value;

/// Collect canonical marked ID and normalized raw key(s) for a Ghost Mode chat target.
pub async fn chat_keys(api: &dyn Telegram, input: Option<&Value>) -> Vec<String> {
    let Some(raw) = input.filter(|value| !value.is_null()) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    if let Ok(peer) = api.resolve_peer(raw).await {
        if let Some(marked) = crate::server::access::marked_peer(&peer)
            && !keys.contains(&marked)
        {
            keys.push(marked);
        } else if let Some(id) =
            crate::state::alias_peer_id(&peer).and_then(|v| v.as_str().map(str::to_owned))
            && !keys.contains(&id)
        {
            keys.push(id);
        }
    }
    if let Some(text) = raw.as_str() {
        let stripped = crate::permalinks::strip_username_prefix(text, None).to_lowercase();
        if !stripped.is_empty() && !keys.contains(&stripped) {
            keys.push(stripped);
        }
    }
    let normalized = crate::state::normalize_key(raw);
    if !normalized.is_empty() && !keys.contains(&normalized) {
        keys.push(normalized);
    }
    keys
}

pub async fn get_ghost_mode(
    api: &dyn Telegram,
    state: &State,
    account: Option<&str>,
    args: &Value,
) -> Result<Value> {
    let keys = chat_keys(api, args.get("chat_id")).await;
    Ok(state.ghost_mode_for_keys(account, &keys))
}

pub async fn set_ghost_mode(
    api: &dyn Telegram,
    state: &State,
    account: Option<&str>,
    args: &Value,
) -> Result<Value> {
    let enabled = crate::args::boolean(args, "enabled", true)?;
    let keys = chat_keys(api, args.get("chat_id")).await;
    state.set_ghost_mode_for_keys(enabled, account, &keys)
}
