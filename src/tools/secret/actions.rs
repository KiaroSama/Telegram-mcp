use super::chats::{
    chat_id, history_limit, nonempty_text, optional_integer, optional_text, required_integer,
    with_account,
};
use crate::backend::Telegram;
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};

pub const TOOL_NAMES: &[&str] = &[
    "delete_secret_message",
    "clear_secret_history",
    "mark_secret_read",
    "send_secret_typing",
    "search_secret_messages",
    "copy_into_secret_chat",
];

const TYPING_ACTIONS: &[&str] = &[
    "typing",
    "recording_voice",
    "recording_video",
    "uploading_photo",
    "uploading_video",
    "uploading_document",
    "cancel",
];

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !TOOL_NAMES.contains(&name) {
        return None;
    }
    Some(dispatch(api, name, args).await)
}

async fn dispatch(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let mut params = payload(name, args)?;
    if name == "copy_into_secret_chat" {
        params["from_chat_id"] = api.resolve_peer(&params["from_chat_id"]).await?;
    }
    api.secret_call(name, params).await
}

fn payload(name: &str, args: &Value) -> Result<Value> {
    let params = match name {
        "delete_secret_message" => json!({
            "chat_id": chat_id(args, "chat_id")?,
            "message_id": required_integer(args, "message_id")?,
        }),
        "clear_secret_history" => {
            let chat_id = chat_id(args, "chat_id")?;
            let confirm_chat_id = required_integer(args, "confirm_chat_id")?;
            ensure!(
                confirm_chat_id == chat_id,
                "confirm_chat_id must match chat_id before history can be cleared on both devices"
            );
            json!({"chat_id": chat_id, "confirm_chat_id": confirm_chat_id})
        }
        "mark_secret_read" => json!({
            "chat_id": chat_id(args, "chat_id")?,
            "message_id": optional_integer(args, "message_id")?,
        }),
        "send_secret_typing" => {
            let action = optional_text(args, "action")?
                .unwrap_or("typing")
                .trim()
                .to_ascii_lowercase();
            ensure!(
                TYPING_ACTIONS.contains(&action.as_str()),
                "action must be one of: {}",
                TYPING_ACTIONS.join(", ")
            );
            json!({"chat_id": chat_id(args, "chat_id")?, "action": action})
        }
        "search_secret_messages" => json!({
            "chat_id": chat_id(args, "chat_id")?,
            "query": nonempty_text(args, "query")?,
            "limit": history_limit(args)?,
        }),
        "copy_into_secret_chat" => {
            let message_id = required_integer(args, "message_id")?;
            ensure!(
                (1..=i64::from(i32::MAX)).contains(&message_id),
                "message_id must be a positive 32-bit cloud message ID"
            );
            let source = &args["from_chat_id"];
            ensure!(
                source.as_i64().is_some() || source.as_str().is_some_and(|s| !s.trim().is_empty()),
                "from_chat_id must be an integer or a nonempty username or alias"
            );
            json!({
                "from_chat_id":source,
                "message_id": message_id,
                "to_chat_id": chat_id(args, "to_chat_id")?,
            })
        }
        _ => bail!("Unknown secret-chat action tool"),
    };
    with_account(params, args)
}

#[cfg(test)]
mod tests {
    use super::payload;
    use serde_json::json;

    #[test]
    fn history_clear_requires_the_same_explicit_id_twice() {
        assert!(payload("clear_secret_history", &json!({"chat_id":7})).is_err());
        assert!(
            payload(
                "clear_secret_history",
                &json!({"chat_id":7,"confirm_chat_id":8})
            )
            .is_err()
        );
        let args = json!({"chat_id":7,"confirm_chat_id":7});
        assert_eq!(payload("clear_secret_history", &args).ok(), Some(args));
    }

    #[test]
    fn typing_normalizes_known_actions_and_rejects_unknown_ones() {
        assert_eq!(
            payload(
                "send_secret_typing",
                &json!({
                    "chat_id":7,"action":" UPLOADING_PHOTO "
                })
            )
            .ok(),
            Some(json!({"chat_id":7,"action":"uploading_photo"}))
        );
        assert!(
            payload(
                "send_secret_typing",
                &json!({"chat_id":7,"action":"offline"})
            )
            .is_err()
        );
    }
}
