use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub fn id(v: &Value) -> Result<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| anyhow::anyhow!("Expected a whole integer ID"))
}
pub fn positive(v: &Value) -> Result<i64> {
    let n = id(v)?;
    ensure!(n > 0, "ID must be positive");
    Ok(n)
}
pub fn ids(v: &Value, max: usize) -> Result<Vec<i64>> {
    let values = match v.as_array() {
        Some(v) => v.clone(),
        None => vec![v.clone()],
    };
    ensure!(
        !values.is_empty() && values.len() <= max,
        "Expected 1..={max} IDs"
    );
    values.iter().map(positive).collect()
}
pub fn unique(values: &[i64]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    ensure!(
        values.iter().all(|v| seen.insert(*v)),
        "Duplicate IDs are not allowed"
    );
    Ok(())
}
pub fn string<'a>(v: &'a Value, key: &str, default: &'a str) -> Result<&'a str> {
    match v.get(key).filter(|x| !x.is_null()) {
        Some(x) => x
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{key} must be a string")),
        None => Ok(default),
    }
}
pub fn txt(s: &str) -> Value {
    json!({"_":"textWithEntities","text":s,"entities":[]})
}
pub fn nonempty(s: &str, max: usize, what: &str) -> Result<()> {
    ensure!(!s.trim().is_empty(), "{what} must not be empty");
    ensure!(
        s.encode_utf16().count() <= max,
        "{what} exceeds {max} UTF-16 code units"
    );
    Ok(())
}
pub fn channel(p: &Value) -> Result<Value> {
    ensure!(
        p["_"].as_str() == Some("inputPeerChannel"),
        "This operation requires a channel or supergroup"
    );
    Ok(json!({"_":"inputChannel","channel_id":p["channel_id"],"access_hash":p["access_hash"]}))
}
pub fn user(p: &Value) -> Result<Value> {
    match p["_"].as_str() {
        Some("inputPeerSelf") => Ok(json!({"_":"inputUserSelf"})),
        Some("inputPeerUser") => {
            Ok(json!({"_":"inputUser","user_id":p["user_id"],"access_hash":p["access_hash"]}))
        }
        _ => bail!("This operation requires a user or bot"),
    }
}
pub fn copy_optional(args: &Value, into: &mut Value, fields: &[&str]) {
    for key in fields {
        if let Some(value) = args.get(*key).filter(|v| !v.is_null()) {
            into[*key] = value.clone();
        }
    }
}
pub fn reaction(args: &Value, allow_empty: bool) -> Result<Value> {
    let emoji = args
        .get("emoji")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let custom = args.get("custom_emoji_id").filter(|v| !v.is_null());
    ensure!(
        emoji.is_none() || custom.is_none(),
        "Choose emoji or custom_emoji_id, not both"
    );
    if let Some(c) = custom {
        positive(c)?;
        return Ok(json!({"_":"reactionCustomEmoji","document_id":c}));
    }
    if let Some(e) = emoji {
        return Ok(json!({"_":"reactionEmoji","emoticon":e}));
    }
    ensure!(allow_empty, "Give emoji or custom_emoji_id");
    Ok(json!({"_":"reactionEmpty"}))
}
pub async fn message(api: &dyn Telegram, p: &Value, msg: i64) -> Result<Value> {
    ensure!(
        msg > 0 && msg <= i32::MAX as i64,
        "message_id must be a positive 32-bit integer"
    );
    let mut params = json!({"id":[{"_":"inputMessageID","id":msg}]});
    let method = if p["_"] == "inputPeerChannel" {
        params["channel"] = channel(p)?;
        "channels.getMessages"
    } else {
        "messages.getMessages"
    };
    let result = api.invoke(method, params).await?;
    result["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|m| id(&m["id"]).ok() == Some(msg)))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Message not found or inaccessible"))
}
pub fn document(d: &Value) -> Result<Value> {
    ensure!(
        d["_"] == "document",
        "Message does not contain a downloadable document"
    );
    Ok(
        json!({"_":"inputDocument","id":d["id"],"access_hash":d["access_hash"],"file_reference":d.get("file_reference").cloned().unwrap_or(json!([]))}),
    )
}
pub fn timestamp(v: &Value, key: &str) -> Result<i64> {
    let s = a::text(v, key)?;
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp());
    }
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .map_err(|_| anyhow::anyhow!("{key} must be an ISO 8601 date-time"))?;
    Ok(dt.and_utc().timestamp())
}
pub fn slug(raw: &str, kind: &str) -> Result<String> {
    fn valid(s: &str) -> bool {
        !s.is_empty()
            && s.len() <= 256
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }
    ensure!(raw.len() <= 2048, "Invite link is too long");
    let raw = raw.trim();
    if valid(raw) {
        return Ok(raw.into());
    }
    let full = if raw.starts_with("t.me/") || raw.starts_with("telegram.me/") {
        format!("https://{raw}")
    } else {
        raw.into()
    };
    let u = url::Url::parse(&full).map_err(|_| anyhow::anyhow!("Invalid Telegram link"))?;
    ensure!(
        u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && u.fragment().is_none(),
        "Invalid Telegram link"
    );
    let result = if u.scheme() == "tg" && u.host_str() == Some(kind) && u.path().is_empty() {
        let v: Vec<_> = u
            .query_pairs()
            .filter(|(k, _)| k == "slug")
            .map(|(_, v)| v.into_owned())
            .collect();
        ensure!(v.len() == 1, "Invalid Telegram link");
        v[0].clone()
    } else {
        ensure!(
            matches!(u.scheme(), "http" | "https")
                && matches!(u.host_str(), Some("t.me" | "telegram.me")),
            "Invalid Telegram link host"
        );
        let prefix = if kind == "addlist" {
            "/addlist/"
        } else {
            "/m/"
        };
        u.path()
            .strip_prefix(prefix)
            .unwrap_or("")
            .trim_end_matches('/')
            .into()
    };
    ensure!(valid(&result), "Invalid Telegram link slug");
    Ok(result)
}
pub async fn entities(api: &dyn Telegram, items: &Value, text: &str) -> Result<Value> {
    Ok(json!(
        crate::tools::messaging::helpers::build_entities(api, text, items).await?
    ))
}
pub fn parsed(text: &str, args: &Value) -> Result<Value> {
    let (text, entities) =
        crate::backend::parse_text(text, args.get("parse_mode").and_then(Value::as_str))?;
    Ok(json!({"_":"textWithEntities","text":text,"entities":entities}))
}
pub async fn upload_media(api: &dyn Telegram, path: &str) -> Result<Value> {
    let path = std::path::Path::new(path);
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = match extension.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "heic" | "heif" => "photo",
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => "video",
        "gif" => "animation",
        "mp3" | "m4a" | "flac" | "wav" | "aac" | "ogg" | "oga" | "opus" => "audio",
        _ => "document",
    };
    crate::tools::messaging::media::uploads::uploaded(api, path, kind, None).await
}
pub async fn attachment(api: &dyn Telegram, peer: &Value, path: &str) -> Result<Value> {
    let media = upload_media(api, path).await?;
    let saved = api
        .invoke("messages.uploadMedia", json!({"peer":peer,"media":media}))
        .await?;
    as_input_media(&saved)
}
pub fn as_input_media(media: &Value) -> Result<Value> {
    if media["_"] == "messageMediaPhoto" {
        let p = &media["photo"];
        return Ok(
            json!({"_":"inputMediaPhoto","id":{"_":"inputPhoto","id":p["id"],"access_hash":p["access_hash"],"file_reference":p["file_reference"]}}),
        );
    }
    if media["_"] == "messageMediaDocument" {
        return Ok(json!({"_":"inputMediaDocument","id":document(&media["document"])?}));
    }
    bail!("Attachment must be a photo or document")
}
pub async fn app_limits(api: &dyn Telegram) -> Value {
    use std::{
        collections::HashMap,
        sync::OnceLock,
        time::{Duration, Instant},
    };
    static CACHE: OnceLock<tokio::sync::Mutex<HashMap<String, (Instant, Value)>>> = OnceLock::new();
    if let Some(key) = api.cache_key() {
        let mut cache = CACHE
            .get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
            .lock()
            .await;
        if let Some((time, value)) = cache.get(&key)
            && time.elapsed() < Duration::from_secs(3600)
        {
            return value.clone();
        }
        let value = fetch_app_limits(api).await;
        if cache.len() >= 64 && !cache.contains_key(&key) {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), value.clone()));
        return value;
    }
    fetch_app_limits(api).await
}
async fn fetch_app_limits(api: &dyn Telegram) -> Value {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.invoke("help.getAppConfig", json!({"hash":0})),
    )
    .await;
    let mut out = json!({});
    if let Ok(Ok(config)) = result
        && let Some(entries) = config["config"]["value"].as_array()
    {
        for entry in entries {
            if let (Some(key), Some(n)) = (entry["key"].as_str(), entry["value"]["value"].as_f64())
                && (1.0..=100000.0).contains(&n)
            {
                out[key] = json!(n as i64);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "../../../tests/unit/advanced_common.rs"]
mod tests;
