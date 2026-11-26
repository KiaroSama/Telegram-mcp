//! Contact alias handlers honoring live `TELEGRAM_ALIASES_FILE`, rejecting
//! unconfirmed bare display names as `chat_id`, and persisting resolved names.
use crate::{backend::Telegram, state::State};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Return `true` only when `input` is a confirmed Telegram identifier
/// (non-zero integer ID, `InputPeer` object, `@username`, recognized link-domain
/// URL, phone number, or `me`/`self`/`saved`). Rejects bare display names.
pub fn is_confirmed_chat_identifier(input: &Value) -> bool {
    if let Some(id) = input.as_i64() {
        return id != 0;
    }
    if input
        .get("_")
        .and_then(Value::as_str)
        .is_some_and(|tag| tag.starts_with("inputPeer"))
    {
        return true;
    }
    let Some(raw) = input.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    if raw.parse::<i64>().is_ok_and(|id| id != 0) {
        return true;
    }
    if matches!(
        raw.to_ascii_lowercase().as_str(),
        "me" | "self" | "saved" | "saved messages"
    ) {
        return true;
    }
    if let Some(rest) = raw.strip_prefix('@') {
        return (1..=64).contains(&rest.len())
            && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    }
    if raw.starts_with('+') {
        let digits = raw.bytes().filter(u8::is_ascii_digit).count();
        return digits >= 7
            && raw
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b' ' | b'(' | b')'));
    }
    let lower = raw.to_ascii_lowercase();
    for prefix in crate::permalinks::username_prefixes(None) {
        if let Some(rest) = lower.strip_prefix(&prefix.to_ascii_lowercase()) {
            let trimmed = rest.trim_matches('/');
            return (1..=64).contains(&trimmed.len())
                && trimmed
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        }
    }
    false
}

async fn resolve_display_name(api: &dyn Telegram, peer: &Value) -> Option<String> {
    match peer["_"].as_str()? {
        "inputPeerSelf" => {
            let users = api
                .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
                .await
                .ok()?;
            let user = users.as_array()?.first()?;
            Some(crate::sanitize::full_name(
                user["first_name"].as_str(),
                user["last_name"].as_str(),
            ))
        }
        "inputPeerUser" => {
            let users = api
                .invoke(
                    "users.getUsers",
                    json!({"id":[{"_":"inputUser","user_id":peer["user_id"],"access_hash":peer["access_hash"]}]}),
                )
                .await
                .ok()?;
            let user = users.as_array()?.first()?;
            let name =
                crate::sanitize::full_name(user["first_name"].as_str(), user["last_name"].as_str());
            if !crate::sanitize::is_empty_placeholder(&name) {
                Some(name)
            } else {
                user["username"].as_str().map(|u| format!("@{u}"))
            }
        }
        "inputPeerChat" => {
            let result = api
                .invoke("messages.getChats", json!({"id":[peer["chat_id"]]}))
                .await
                .ok()?;
            let chat = result["chats"].as_array()?.first()?;
            chat["title"].as_str().map(str::to_owned)
        }
        "inputPeerChannel" => {
            let result = api
                .invoke(
                    "channels.getChannels",
                    json!({"id":[{"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]}]}),
                )
                .await
                .ok()?;
            let chat = result["chats"].as_array()?.first()?;
            chat["title"].as_str().map(str::to_owned)
        }
        _ => None,
    }
}

pub fn list_contact_aliases(state: &State, account: &str) -> Result<Value> {
    let aliases = state.aliases(account)?;
    let mut grouped = BTreeMap::<String, Value>::new();
    for (alias, record) in aliases.as_object().context("Invalid aliases record")? {
        let peer = &record["peer"];
        let id = crate::state::alias_peer_id(peer)
            .or_else(|| record.get("id").cloned())
            .unwrap_or(Value::Null);
        let key = serde_json::to_string(&id)?;
        let row = grouped.entry(key).or_insert_with(|| {
            json!({
                "id": id,
                "name": record["name"],
                "peer": peer,
                "account": account,
                "aliases": [],
                "needs_migration": record["needs_migration"]
            })
        });
        if row["name"].is_null() {
            row["name"] = record["name"].clone();
        }
        if let Some(list) = row["aliases"].as_array_mut() {
            list.push(json!(alias));
        }
    }
    Ok(json!(grouped.into_values().collect::<Vec<_>>()))
}

pub async fn set_contact_alias(
    api: &dyn Telegram,
    state: &State,
    account: &str,
    args: &Value,
) -> Result<Value> {
    let alias = crate::args::text(args, "alias")?;
    let input = crate::args::required(args, "chat_id")?;
    if !is_confirmed_chat_identifier(input) {
        bail!(
            "chat_id must be a numeric ID, @username, phone number, or confirmed peer; bare display names are not accepted"
        );
    }
    let resolved = api.resolve_peer(input).await?;
    let display_name = resolve_display_name(api, &resolved).await;
    state.alias_set_with_name(
        account,
        alias,
        resolved,
        display_name.as_deref(),
        crate::args::boolean(args, "replace", false)?,
    )
}

pub fn delete_contact_alias(state: &State, account: &str, args: &Value) -> Result<Value> {
    Ok(json!({
        "deleted": state.alias_remove(account, crate::args::text(args, "alias")?)?
    }))
}
