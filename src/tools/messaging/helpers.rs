use crate::{args, backend::Telegram};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};

pub fn id(a: &Value, key: &str) -> Result<i64> {
    let value = args::integer(a, key, 0)?;
    ensure!(
        value > 0 && value <= i32::MAX as i64,
        "{key} must be a positive 32-bit message id"
    );
    Ok(value)
}
pub fn ids(a: &Value, key: &str) -> Result<Vec<i64>> {
    let raw = args::required(a, key)?;
    let raw = raw.as_array().cloned().unwrap_or_else(|| vec![raw.clone()]);
    ensure!(
        !raw.is_empty() && raw.len() <= 100,
        "{key} must contain 1 to 100 message ids"
    );
    raw.into_iter()
        .map(|value| id(&json!({key:value}), key))
        .collect()
}
pub fn channel(peer: &Value) -> Option<Value> {
    (peer["_"].as_str() == Some("inputPeerChannel")).then(|| json!({"_":"inputChannel","channel_id":peer["channel_id"],"access_hash":peer["access_hash"]}))
}
pub fn input_user(peer: &Value) -> Result<Value> {
    match peer["_"].as_str() {
        Some("inputPeerSelf") => Ok(json!({"_":"inputUserSelf"})),
        Some("inputPeerUser") => {
            Ok(json!({"_":"inputUser","user_id":peer["user_id"],"access_hash":peer["access_hash"]}))
        }
        _ => bail!("This operation requires a user, not a group or channel"),
    }
}
pub fn result_messages(value: &Value) -> Vec<Value> {
    value["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m["_"].as_str() != Some("messageEmpty"))
        .collect()
}
pub fn history_request(peer: Value, limit: i64, offset_id: i64, add_offset: i64) -> Value {
    json!({"peer":peer,"limit":limit,"offset_id":offset_id,"offset_date":0,"add_offset":add_offset,"max_id":0,"min_id":0,"hash":"0"})
}
pub async fn fetch(api: &dyn Telegram, peer: &Value, message_id: i64) -> Result<Value> {
    let mut request = json!({"id":[{"_":"inputMessageID","id":message_id}]});
    let method = if let Some(channel) = channel(peer) {
        request["channel"] = channel;
        "channels.getMessages"
    } else {
        "messages.getMessages"
    };
    let expected = requested_peer(api, peer).await?;
    let response = api.invoke(method, request).await?;
    result_messages(&response)
        .into_iter()
        .find(|m| m["id"].as_i64() == Some(message_id) && same_peer(&m["peer_id"], &expected))
        .ok_or_else(|| anyhow!("Message {message_id} was not found in the requested chat"))
}
pub async fn requested_peer(api: &dyn Telegram, peer: &Value) -> Result<Value> {
    let (constructor, key) = match peer["_"].as_str() {
        Some("inputPeerUser") => ("peerUser", "user_id"),
        Some("inputPeerChat") => ("peerChat", "chat_id"),
        Some("inputPeerChannel") => ("peerChannel", "channel_id"),
        Some("inputPeerSelf") => {
            let users = api
                .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
                .await?;
            let id = users[0]
                .get("id")
                .filter(|id| {
                    id.as_i64().is_some_and(|id| id > 0)
                        || id
                            .as_str()
                            .is_some_and(|id| id.parse::<i64>().is_ok_and(|id| id > 0))
                })
                .ok_or_else(|| anyhow!("Could not resolve the current account's peer"))?;
            return Ok(json!({"_":"peerUser","user_id":id}));
        }
        _ => bail!("Could not bind the message to the requested chat"),
    };
    ensure!(!peer[key].is_null(), "The requested peer has no id");
    Ok(json!({"_":constructor,key:peer[key]}))
}
pub fn same_peer(actual: &Value, expected: &Value) -> bool {
    actual["_"] == expected["_"]
        && ["user_id", "chat_id", "channel_id"].iter().any(|key| {
            actual.get(*key).filter(|v| !v.is_null()).is_some_and(|id| {
                id.to_string().trim_matches('"') == expected[*key].to_string().trim_matches('"')
            })
        })
}
pub fn copy_optional(target: &mut Value, source: &Value, pairs: &[(&str, &str)]) {
    for (from, to) in pairs {
        if let Some(value) = source.get(*from).filter(|v| !v.is_null()) {
            target[*to] = value.clone();
        }
    }
}
pub fn topic_root(a: &Value) -> Result<Option<i64>> {
    // General has no topic root; its sends and replies use the ordinary chat route.
    Ok(a.get("topic_id")
        .filter(|v| !v.is_null())
        .map(|_| id(a, "topic_id"))
        .transpose()?
        .filter(|topic| *topic != 1))
}
pub fn reply(a: &Value, fallback: Option<i64>) -> Result<Option<Value>> {
    let topic = topic_root(a)?;
    let reply = a
        .get("reply_to_message_id")
        .filter(|v| !v.is_null())
        .map(|_| id(a, "reply_to_message_id"))
        .transpose()?
        .or(fallback);
    Ok(match (reply, topic) {
        (None, None) => None,
        (Some(message), topic) => {
            let mut r = json!({"_":"inputReplyToMessage","reply_to_msg_id":message});
            if let Some(topic) = topic {
                r["top_msg_id"] = json!(topic);
            }
            Some(r)
        }
        (None, Some(topic)) => {
            Some(json!({"_":"inputReplyToMessage","reply_to_msg_id":topic,"top_msg_id":topic}))
        }
    })
}
pub fn entities(text: &str, raw: &Value) -> Result<Vec<Value>> {
    let input = raw
        .as_array()
        .ok_or_else(|| anyhow!("entities must be an array"))?;
    ensure!(input.len() <= 1000, "Too many formatting entities");
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::with_capacity(input.len());
    for entity in input {
        ensure!(
            !entity["offset_is_raw"].as_bool().unwrap_or(false),
            "Raw entity offsets cannot be sent against displayed text"
        );
        ensure!(
            !entity["url_altered"].as_bool().unwrap_or(false)
                && !entity["language_altered"].as_bool().unwrap_or(false),
            "Altered entity values must be supplied explicitly before sending"
        );
        let offset = args::integer(entity, "offset", -1)?;
        let length = args::integer(entity, "length", -1)?;
        ensure!(
            offset >= 0
                && length > 0
                && offset
                    .checked_add(length)
                    .is_some_and(|end| end <= units.len() as i64),
            "Entity offsets must fit the message's UTF-16 text"
        );
        for boundary in [offset as usize, (offset + length) as usize] {
            ensure!(
                boundary == 0
                    || boundary == units.len()
                    || !(0xD800..=0xDBFF).contains(&units[boundary - 1])
                    || !(0xDC00..=0xDFFF).contains(&units[boundary]),
                "Entity boundary splits a UTF-16 surrogate pair"
            );
        }
        let mut e = entity.clone();
        if e.get("_").is_none() {
            let kind = match entity["type"].as_str().unwrap_or("") {
                "bold" => "messageEntityBold",
                "italic" => "messageEntityItalic",
                "underline" => "messageEntityUnderline",
                "strike" | "strikethrough" => "messageEntityStrike",
                "spoiler" => "messageEntitySpoiler",
                "code" => "messageEntityCode",
                "pre" => "messageEntityPre",
                "text_url" => "messageEntityTextUrl",
                "custom_emoji" => "messageEntityCustomEmoji",
                "blockquote" => "messageEntityBlockquote",
                "mention" => "messageEntityMention",
                "url" => "messageEntityUrl",
                "email" => "messageEntityEmail",
                "phone" => "messageEntityPhone",
                "hashtag" => "messageEntityHashtag",
                "bot_command" => "messageEntityBotCommand",
                "cashtag" => "messageEntityCashtag",
                "bank_card" => "messageEntityBankCard",
                "formatted_date" => "messageEntityFormattedDate",
                "diff_insert" => "messageEntityDiffInsert",
                "diff_replace" => "messageEntityDiffReplace",
                "diff_delete" => "messageEntityDiffDelete",
                "ton_address" => "messageEntityTonAddress",
                "mention_name" => "inputMessageEntityMentionName",
                _ => bail!("Unsupported formatting entity type"),
            };
            e["_"] = json!(kind);
            e.as_object_mut().unwrap().remove("type");
            if kind == "messageEntityPre" && e.get("language").is_none() {
                e["language"] = json!("");
            }
            if let Some(emoji) = e.get("custom_emoji_id").cloned() {
                e["document_id"] = emoji;
                e.as_object_mut().unwrap().remove("custom_emoji_id");
            }
        }
        for key in ["text", "offset_is_raw", "url_altered", "language_altered"] {
            e.as_object_mut().unwrap().remove(key);
        }
        out.push(e);
    }
    Ok(out)
}
pub fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn schedule(a: &Value, key: &str) -> Result<Option<i64>> {
    let Some(value) = a.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let seconds = if let Some(n) = value.as_i64() {
        n
    } else if let Some(s) = value.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            n
        } else {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.timestamp())
                .or_else(|_| {
                    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                        .map(|d| d.and_utc().timestamp())
                })
                .map_err(|_| anyhow!("{key} must be a Unix timestamp or ISO-8601 date"))?
        }
    } else {
        bail!("{key} must be a Unix timestamp or ISO-8601 date")
    };
    ensure!(
        seconds > unix_time() && seconds <= i32::MAX as i64,
        "Scheduled time must be in the future and within Telegram's date range"
    );
    Ok(Some(seconds))
}
pub fn repeat(a: &Value) -> Result<Option<i64>> {
    match a
        .get("repeat")
        .filter(|v| !v.is_null())
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "none" | "off" => Ok(None),
        "daily" => Ok(Some(86400)),
        "weekly" => Ok(Some(604800)),
        _ => bail!("repeat must be daily, weekly, off, or omitted"),
    }
}
pub fn media_filter(a: &Value) -> Result<Value> {
    let kind = match a["media_type"].as_str().unwrap_or("") {
        "" => "inputMessagesFilterEmpty",
        "photos" => "inputMessagesFilterPhotos",
        "videos" => "inputMessagesFilterVideo",
        "media" => "inputMessagesFilterPhotoVideo",
        "links" => "inputMessagesFilterUrl",
        "files" => "inputMessagesFilterDocument",
        "music" => "inputMessagesFilterMusic",
        "voice" => "inputMessagesFilterVoice",
        "round_voice" => "inputMessagesFilterRoundVoice",
        "round_video" => "inputMessagesFilterRoundVideo",
        "gifs" => "inputMessagesFilterGif",
        "geo" => "inputMessagesFilterGeo",
        "contacts" => "inputMessagesFilterContacts",
        "phone_calls" => "inputMessagesFilterPhoneCalls",
        "chat_photos" => "inputMessagesFilterChatPhotos",
        "pinned" => "inputMessagesFilterPinned",
        "mentions" => "inputMessagesFilterMyMentions",
        _ => bail!("Unsupported media_type"),
    };
    Ok(json!({"_":kind}))
}
pub fn date(a: &Value, key: &str, end_of_day: bool) -> Result<i64> {
    let Some(s) = a.get(key).filter(|v| !v.is_null()).and_then(Value::as_str) else {
        return Ok(0);
    };
    if s.is_empty() {
        return Ok(0);
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| anyhow!("{key} must use YYYY-MM-DD"))?;
    Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() + if end_of_day { 86400 } else { 0 })
}

pub async fn build_entities(api: &dyn Telegram, text: &str, raw: &Value) -> Result<Vec<Value>> {
    let mut output = entities(text, raw)?;
    for entity in &mut output {
        if matches!(
            entity["_"].as_str(),
            Some("inputMessageEntityMentionName" | "messageEntityMentionName")
        ) {
            entity["_"] = json!("inputMessageEntityMentionName");
            let target = entity
                .get("user_id")
                .filter(|v| !v.is_null())
                .ok_or_else(|| anyhow!("mention_name requires user_id"))?;
            entity["user_id"] = input_user(&api.resolve_peer(target).await?)?;
        }
    }
    Ok(output)
}
