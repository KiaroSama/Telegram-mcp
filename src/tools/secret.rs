pub mod actions;
pub mod autosave;
pub mod chats;
pub mod custody;
pub mod messaging;
pub mod timed;

use crate::{args::required, backend::Telegram};
use anyhow::{Context, Result};
use serde_json::Value;

pub const TOOL_NAMES: &[&str] = &[
    "accept_secret_chat",
    "acknowledge_secret_delivery",
    "check_secret_custody",
    "clear_secret_history",
    "close_secret_chat",
    "configure_secret_backend",
    "copy_into_secret_chat",
    "create_secret_chat",
    "delete_saved_secret_messages",
    "delete_secret_chat",
    "delete_secret_chat_both_sides",
    "delete_secret_message",
    "export_secret_chat",
    "forget_secret_chat",
    "forward_secret_media",
    "get_secret_backend_status",
    "get_secret_pause_status",
    "list_secret_chats",
    "list_secret_deliveries",
    "mark_secret_read",
    "pause_secret_chat",
    "read_secret_messages",
    "receive_secret_media",
    "resume_secret_chat",
    "save_secret_media",
    "search_secret_messages",
    "secret_chat_status",
    "send_secret_media",
    "send_secret_message",
    "send_secret_typing",
    "send_timed_secret_media",
    "send_timed_secret_message",
    "set_secret_chat_timer",
    "start_secret_auto_save",
    "stop_secret_auto_save",
];

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if actions::TOOL_NAMES.contains(&name) {
        actions::call(api, name, args).await
    } else if autosave::TOOL_NAMES.contains(&name) {
        autosave::call(api, name, args).await
    } else if chats::TOOL_NAMES.contains(&name) {
        chats::call(api, name, args).await
    } else if custody::TOOL_NAMES.contains(&name) {
        custody::call(api, name, args).await
    } else if messaging::TOOL_NAMES.contains(&name) {
        messaging::call(api, name, args).await
    } else if timed::TOOL_NAMES.contains(&name) {
        timed::call(api, name, args).await
    } else {
        None
    }
}

fn secret_id(args: &Value, key: &str) -> Result<i64> {
    let value = required(args, key)?
        .as_i64()
        .with_context(|| format!("{key} must be an integer"))?;
    crate::secret::wire_chat_id(value)
        .map(i64::from)
        .with_context(|| format!("{key} must identify a non-zero signed 32-bit secret chat"))
}
