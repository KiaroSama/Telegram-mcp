use crate::backend::Telegram;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub fn req<'a>(a: &'a Value, key: &str) -> Result<&'a Value> {
    a.get(key)
        .filter(|v| !v.is_null())
        .with_context(|| format!("{key} is required"))
}
pub fn text<'a>(a: &'a Value, key: &str) -> Result<&'a str> {
    req(a, key)?
        .as_str()
        .with_context(|| format!("{key} must be a string"))
}
pub fn string(a: &Value, key: &str, default: &str) -> Result<String> {
    match a.get(key).filter(|v| !v.is_null()) {
        None => Ok(default.to_owned()),
        Some(v) => Ok(v
            .as_str()
            .with_context(|| format!("{key} must be a string"))?
            .to_owned()),
    }
}
pub fn int_value(value: &Value, label: &str) -> Result<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .with_context(|| format!("{label} must be an integer"))
}
pub fn int(a: &Value, key: &str, default: i64) -> Result<i64> {
    a.get(key)
        .filter(|v| !v.is_null())
        .map_or(Ok(default), |v| int_value(v, key))
}
pub fn boolean(a: &Value, key: &str, default: bool) -> Result<bool> {
    a.get(key)
        .filter(|v| !v.is_null())
        .map_or(Ok(default), |v| {
            v.as_bool()
                .with_context(|| format!("{key} must be a boolean"))
        })
}
pub fn limit(a: &Value, key: &str, default: i64, maximum: i64) -> Result<i64> {
    let value = int(a, key, default)?;
    ensure!(value > 0, "{key} must be positive");
    Ok(value.min(maximum))
}
pub fn array<'a>(a: &'a Value, key: &str) -> Result<&'a [Value]> {
    Ok(req(a, key)?
        .as_array()
        .with_context(|| format!("{key} must be an array"))?)
}
pub fn kind(v: &Value) -> &str {
    v.get("_").and_then(Value::as_str).unwrap_or("")
}
pub fn entity_id(v: &Value) -> Result<Value> {
    ["user_id", "channel_id", "chat_id", "id"]
        .iter()
        .find_map(|k| v.get(*k))
        .cloned()
        .context("Telegram returned a peer without an id")
}
pub fn user(peer: &Value) -> Result<Value> {
    match kind(peer) {
        "inputPeerSelf" | "inputUserSelf" => Ok(json!({"_":"inputUserSelf"})),
        "inputPeerUser" | "inputUser" => Ok(
            json!({"_":"inputUser","user_id":req(peer,"user_id")?,"access_hash":req(peer,"access_hash")?}),
        ),
        _ => bail!("The target must be a user, not a group or channel"),
    }
}
pub fn channel(peer: &Value) -> Result<Value> {
    ensure!(
        matches!(kind(peer), "inputPeerChannel" | "inputChannel"),
        "This operation requires a channel or supergroup"
    );
    Ok(
        json!({"_":"inputChannel","channel_id":req(peer,"channel_id")?,"access_hash":req(peer,"access_hash")?}),
    )
}
pub fn is_channel(peer: &Value) -> bool {
    matches!(kind(peer), "inputPeerChannel" | "inputChannel")
}
pub async fn peer(api: &dyn Telegram, a: &Value, key: &str) -> Result<Value> {
    api.resolve_peer(req(a, key)?).await
}
pub async fn users(api: &dyn Telegram, values: &[Value]) -> Result<Vec<Value>> {
    let mut result = Vec::with_capacity(values.len());
    for v in values {
        result.push(user(&api.resolve_peer(v).await?)?);
    }
    Ok(result)
}
pub async fn full(api: &dyn Telegram, p: &Value) -> Result<Value> {
    if is_channel(p) {
        api.invoke("channels.getFullChannel", json!({"channel":channel(p)?}))
            .await
    } else if kind(p) == "inputPeerChat" {
        api.invoke("messages.getFullChat", json!({"chat_id":entity_id(p)?}))
            .await
    } else {
        api.invoke("users.getFullUser", json!({"id":user(p)?}))
            .await
    }
}
pub async fn chat_info(api: &dyn Telegram, p: &Value) -> Result<Value> {
    let r = full(api, p).await?;
    let id = entity_id(p)?;
    r.get("chats")
        .and_then(Value::as_array)
        .and_then(|xs| xs.iter().find(|e| e["id"] == id))
        .cloned()
        .context("Telegram returned no chat metadata")
}
pub fn now() -> Result<i64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        .try_into()?)
}
pub fn copy_present(a: &Value, target: &mut Value, keys: &[&str]) {
    for key in keys {
        if let Some(v) = a.get(*key).filter(|v| !v.is_null()) {
            target[*key] = v.clone();
        }
    }
}
pub fn display_name(v: &Value) -> String {
    if let Some(title) = v
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return title.to_owned();
    }
    let name = ["first_name", "last_name"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        if v.get("deleted").and_then(Value::as_bool) == Some(true) {
            "Deleted Account".to_owned()
        } else {
            v.get("username")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_owned()
        }
    } else {
        name
    }
}
pub fn input_photo(photo: &Value) -> Result<Value> {
    Ok(
        json!({"_":"inputPhoto","id":req(photo,"id")?,"access_hash":req(photo,"access_hash")?,"file_reference":req(photo,"file_reference")?}),
    )
}
pub fn input_document(doc: &Value) -> Result<Value> {
    Ok(
        json!({"_":"inputDocument","id":req(doc,"id")?,"access_hash":req(doc,"access_hash")?,"file_reference":req(doc,"file_reference")?}),
    )
}
pub fn username(value: &str, allow_empty: bool) -> Result<String> {
    let raw = value.trim();
    let value = [
        "https://t.me/",
        "http://t.me/",
        "t.me/",
        "https://telegram.me/",
        "http://telegram.me/",
        "telegram.me/",
        "@",
    ]
    .iter()
    .find_map(|p| raw.strip_prefix(p))
    .unwrap_or(raw)
    .trim();
    ensure!(
        (allow_empty && value.is_empty())
            || (value.len() >= 5
                && value.len() <= 32
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && !value.as_bytes()[0].is_ascii_digit()),
        "Username must contain 5-32 ASCII letters, digits or underscores and cannot start with a digit"
    );
    Ok(value.to_owned())
}
