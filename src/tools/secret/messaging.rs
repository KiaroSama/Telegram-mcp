use super::chats::{
    chat_id, history_limit, nonempty_text, optional_integer, optional_text, required_integer,
    with_account,
};
use crate::{args, backend::Telegram};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub const TOOL_NAMES: &[&str] = &[
    "send_secret_message",
    "send_secret_media",
    "read_secret_messages",
    "save_secret_media",
    "forward_secret_media",
];

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    Some(match payload(name, args) {
        Ok(payload) => api.secret_call(name, payload).await,
        Err(error) => Err(error),
    })
}

fn payload(name: &str, args: &Value) -> Result<Value> {
    let params = match name {
        "send_secret_message" => json!({
            "chat_id": chat_id(args, "chat_id")?,
            "message": nonempty_text(args, "message")?,
            "parse_mode": parse_mode(args)?,
            "reply_to_message_id": optional_integer(args, "reply_to_message_id")?,
        }),
        "send_secret_media" => media_payload(args)?,
        "read_secret_messages" => json!({
            "chat_id": chat_id(args, "chat_id")?,
            "limit": history_limit(args)?,
        }),
        "save_secret_media" => json!({
            "chat_id": chat_id(args, "chat_id")?,
            "message_id": required_integer(args, "message_id")?,
            "destination": optional_path(args, "destination")?,
            "honour_sender_restriction": args::boolean(args, "honour_sender_restriction", false)?,
        }),
        "forward_secret_media" => json!({
            "from_chat_id": chat_id(args, "from_chat_id")?,
            "message_id": required_integer(args, "message_id")?,
            "to_chat_id": chat_id(args, "to_chat_id")?,
            "caption": optional_text(args, "caption")?.unwrap_or_default(),
            "reply_to_message_id": optional_integer(args, "reply_to_message_id")?,
        }),
        _ => bail!("Unknown secret-chat messaging tool"),
    };
    with_account(params, args)
}

fn media_payload(args: &Value) -> Result<Value> {
    let chat_id = chat_id(args, "chat_id")?;
    let file_path = nonempty_text(args, "file_path")?;
    let self_destruct_seconds = args::integer(args, "self_destruct_seconds", 0)?;
    ensure!(
        (0..=60).contains(&self_destruct_seconds),
        "self_destruct_seconds must be between 0 and 60"
    );
    ensure!(
        self_destruct_seconds == 0,
        "Secret chats use a chat timer. Use set_secret_chat_timer or send_timed_secret_media"
    );
    let as_voice = args::boolean(args, "as_voice", false)?;
    let kind = optional_text(args, "kind")?;
    ensure!(
        !as_voice || kind.is_none_or(|kind| kind == "voice_note"),
        "as_voice requires kind to be unset or voice_note"
    );
    let kind = if as_voice {
        Some("voice_note")
    } else {
        kind.filter(|s| !s.is_empty())
    };
    let caption = optional_text(args, "caption")?.unwrap_or_default();
    validate_media_kind(file_path, kind, caption)?;
    let waveform = match &args["waveform"] {
        Value::Null => None,
        value => {
            let samples = value
                .as_array()
                .context("waveform must be an array of integers")?;
            ensure!(
                (1..=100).contains(&samples.len())
                    && samples
                        .iter()
                        .all(|sample| sample.as_u64().is_some_and(|n| n <= 31)),
                "waveform must contain 1 to 100 integers between 0 and 31"
            );
            Some(samples)
        }
    };
    // The runtime checks the actual source, thumbnail and destination against the account's roots.
    Ok(json!({
        "chat_id": chat_id,
        "file_path": file_path,
        "kind": kind,
        "self_destruct_seconds": self_destruct_seconds,
        "as_voice": as_voice,
        "caption": caption,
        "reply_to_message_id": optional_integer(args, "reply_to_message_id")?,
        "duration": media_dimension(args, "duration")?,
        "width": media_dimension(args, "width")?,
        "height": media_dimension(args, "height")?,
        "thumbnail_path": optional_path(args, "thumbnail_path")?,
        "title": optional_text(args, "title")?,
        "performer": optional_text(args, "performer")?,
        "sticker_alt": optional_text(args, "sticker_alt")?,
        "waveform": waveform,
    }))
}

fn media_dimension(args: &Value, key: &str) -> Result<Option<i64>> {
    let value = optional_integer(args, key)?;
    ensure!(
        value.is_none_or(|n| (0..=i64::from(i32::MAX)).contains(&n)),
        "{key} must be a non-negative 32-bit integer"
    );
    Ok(value)
}

fn optional_path<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    optional_text(args, key)?
        .map(|_| nonempty_text(args, key))
        .transpose()
}

pub(super) fn parse_mode(args: &Value) -> Result<Option<&str>> {
    match optional_text(args, "parse_mode")?.filter(|mode| !mode.is_empty()) {
        None => Ok(None),
        Some(mode) if mode.eq_ignore_ascii_case("markdown") || mode.eq_ignore_ascii_case("md") => {
            Ok(Some("markdown"))
        }
        Some(mode) if mode.eq_ignore_ascii_case("html") || mode.eq_ignore_ascii_case("htm") => {
            Ok(Some("html"))
        }
        Some(_) => bail!("parse_mode must be markdown or html"),
    }
}

pub(super) fn validate_media_kind(
    file_path: &str,
    kind: Option<&str>,
    caption: &str,
) -> Result<()> {
    let extension = Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let allowed: &[&str] = match extension.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "heic" | "heif" => {
            &["photo", "sticker", "document"]
        }
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => {
            &["video", "video_note", "animation", "document"]
        }
        "mp3" | "m4a" | "flac" | "wav" | "aac" => &["audio", "document"],
        "ogg" | "oga" | "opus" => &["voice_note", "audio", "document"],
        "gif" => &["animation", "video", "document"],
        "tgs" => &["sticker", "document"],
        _ => &[],
    };
    let kind = kind
        .or_else(|| allowed.first().copied())
        .unwrap_or("document");
    ensure!(
        matches!(
            kind,
            "photo"
                | "video"
                | "document"
                | "audio"
                | "animation"
                | "sticker"
                | "video_note"
                | "voice_note"
        ),
        "Unsupported secret-chat media kind"
    );
    ensure!(
        allowed.is_empty() || allowed.contains(&kind),
        "The secret-chat media kind is incompatible with file_path"
    );
    ensure!(
        caption.is_empty() || !matches!(kind, "sticker" | "video_note"),
        "Stickers and video notes cannot carry captions"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::payload;
    use serde_json::json;

    #[test]
    fn reads_thirty_messages_by_default_and_refuses_unbounded_requests() {
        let args = json!({"chat_id":7});
        assert_eq!(
            payload("read_secret_messages", &args).ok(),
            Some(json!({"chat_id":7,"limit":30}))
        );
        assert!(payload("read_secret_messages", &json!({"chat_id":7,"limit":501})).is_err());
    }

    #[test]
    fn refuses_per_message_ttl_and_conflicting_voice_kind() {
        assert!(
            payload(
                "send_secret_media",
                &json!({
                    "chat_id":7,"file_path":"message.jpg","self_destruct_seconds":1
                })
            )
            .is_err()
        );
        assert!(
            payload(
                "send_secret_media",
                &json!({
                    "chat_id":7,"file_path":"message.ogg","kind":"audio","as_voice":true
                })
            )
            .is_err()
        );
    }
}
