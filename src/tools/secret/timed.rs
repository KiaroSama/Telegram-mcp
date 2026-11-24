use super::{
    chats::{chat_id, nonempty_text, optional_text, required_integer, with_account},
    messaging::{parse_mode, validate_media_kind},
};
use crate::backend::Telegram;
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub const TOOL_NAMES: &[&str] = &["send_timed_secret_message", "send_timed_secret_media"];

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
    let chat_id = chat_id(args, "chat_id")?;
    let seconds = required_integer(args, "seconds")?;
    ensure!(
        (1..=604_800).contains(&seconds),
        "seconds must be between 1 and 604800"
    );
    let params = match name {
        "send_timed_secret_message" => json!({
            "chat_id": chat_id,
            "message": nonempty_text(args, "message")?,
            "seconds": seconds,
            "parse_mode": parse_mode(args)?,
        }),
        "send_timed_secret_media" => {
            let file_path = nonempty_text(args, "file_path")?;
            let kind = optional_text(args, "kind")?.filter(|kind| !kind.is_empty());
            let caption = optional_text(args, "caption")?.unwrap_or_default();
            validate_media_kind(file_path, kind, caption)?;
            json!({"chat_id": chat_id, "file_path": file_path, "seconds": seconds, "kind": kind, "caption": caption})
        }
        _ => bail!("Unknown timed secret-chat tool"),
    };
    with_account(params, args)
}

#[cfg(test)]
mod tests {
    use super::payload;
    use serde_json::json;

    #[test]
    fn validates_timed_send_using_legacy_seconds_parameter() {
        assert!(
            payload(
                "send_timed_secret_message",
                &json!({
                    "chat_id":7,"message":"test","ttl_seconds":1
                })
            )
            .is_err()
        );
        for seconds in [0, 604_801] {
            assert!(
                payload(
                    "send_timed_secret_message",
                    &json!({
                        "chat_id":7,"message":"test","seconds":seconds
                    })
                )
                .is_err()
            );
        }
        assert_eq!(
            payload(
                "send_timed_secret_message",
                &json!({
                    "chat_id":7,"message":"test","seconds":604800
                })
            )
            .ok(),
            Some(json!({"chat_id":7,"message":"test","seconds":604800,"parse_mode":null}))
        );
    }
}
