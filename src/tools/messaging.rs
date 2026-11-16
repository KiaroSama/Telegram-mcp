//! Message, media and keyboard operations on the account's existing MTProto connection.
pub mod helpers;
mod inspect;
mod keyboard;
pub mod media;
mod read;
pub mod rich_quote;
mod state;
pub mod transcribe;
mod write;

use crate::backend::Telegram;
use anyhow::Result;
use serde_json::Value;

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if name == "get_send_as" {
        return state::call(api, "list_send_as", args).await;
    }
    if let Some(result) = transcribe::call(api, name, args).await {
        return Some(result);
    }
    if let Some(result) = read::call(api, name, args).await {
        return Some(result);
    }
    if let Some(result) = write::call(api, name, args).await {
        return Some(result);
    }
    if let Some(result) = state::call(api, name, args).await {
        return Some(result);
    }
    if let Some(result) = inspect::call(api, name, args).await {
        return Some(result);
    }
    media::call(api, name, args).await
}

/// Concrete routes implemented by this module (used to audit catalogue coverage).
pub const TOOL_NAMES: &[&str] = &[
    "transcribe_voice",
    "rate_voice_transcription",
    "get_send_as",
    "answer_reply_button",
    "cancel_scheduled_message",
    "clear_draft",
    "click_button",
    "copy_message",
    "delete_chat_history",
    "delete_message",
    "delete_messages_bulk",
    "delete_scheduled_message",
    "download_media",
    "download_rich_media",
    "edit_message",
    "edit_message_media",
    "edit_scheduled_message",
    "forward_message",
    "forward_messages",
    "get_custom_emoji",
    "get_drafts",
    "get_history",
    "get_media_details",
    "get_media_frames",
    "get_media_info",
    "get_media_thumbnail",
    "get_message_context",
    "get_message_reactions",
    "get_message_read_by",
    "get_message_views",
    "get_messages",
    "get_photo_sheet",
    "get_pinned_messages",
    "get_scheduled_messages",
    "inline_query",
    "inspect_buttons",
    "inspect_custom_emoji",
    "inspect_message",
    "inspect_messages",
    "list_calls",
    "list_disappearing_media",
    "list_inline_buttons",
    "list_messages",
    "list_photos",
    "list_scheduled_messages",
    "list_send_as",
    "mark_as_read",
    "open_photo",
    "pin_message",
    "press_inline_button",
    "press_reply_button",
    "read_rich_message",
    "remove_reaction",
    "replace_custom_emoji",
    "reply_to_message",
    "save_disappearing_media",
    "save_draft",
    "schedule_message",
    "search_global",
    "search_messages",
    "search_posts",
    "send_album",
    "send_disappearing_media",
    "send_file",
    "send_inline_result",
    "send_message",
    "send_reaction",
    "send_scheduled_message",
    "send_sticker",
    "send_typing",
    "send_voice",
    "set_default_send_as",
    "unpin_all_messages",
    "unpin_message",
    "upload_file",
];
