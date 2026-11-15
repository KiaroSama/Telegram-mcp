//! Telegram's higher-level features, implemented on the same grammers connection.
mod business;
mod checklists;
mod common;
mod effects;
mod emoji;
mod emoji_assets;
mod folder_plan;
mod folder_update;
mod folders;
mod polls;
mod saved;
mod stats;
mod sticker_refs;
mod stories;
mod topics;

use crate::backend::Telegram;
use anyhow::Result;
use serde_json::Value;

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    macro_rules! domain {
        ($($module:ident),+) => {$(if let Some(result) = $module::call(api, name, args).await { return Some(result); })+};
    }
    domain!(
        topics,
        folder_plan,
        folders,
        emoji,
        saved,
        polls,
        checklists,
        stories,
        business,
        stats
    );
    None
}

/// All handlers implemented by this domain.
pub const TOOL_NAMES: &[&str] = &[
    "get_folder_limits",
    "get_folder_snapshot",
    "update_folder",
    "get_custom_emoji_packs",
    "get_custom_emoji_pack",
    "preview_custom_emoji",
    "vote_poll",
    "list_topics",
    "enable_forum_topics",
    "create_forum_topic",
    "edit_forum_topic",
    "delete_forum_topic",
    "pin_forum_topic",
    "reorder_pinned_topics",
    "disable_forum_topics",
    "upgrade_to_supergroup",
    "list_forum_topic_icons",
    "list_folders",
    "get_folder",
    "create_folder",
    "add_chat_to_folder",
    "remove_chat_from_folder",
    "delete_folder",
    "reorder_folders",
    "get_chatlist_invite",
    "list_message_effects",
    "get_message_effect",
    "open_mini_app",
    "add_quick_reply",
    "read_quick_reply",
    "edit_quick_reply",
    "rename_quick_reply",
    "delete_quick_reply",
    "get_poll_results",
    "vote_in_poll",
    "get_poll_voters",
    "close_poll",
    "create_poll",
    "add_poll_option",
    "delete_poll_option",
    "list_unread_poll_votes",
    "mark_poll_votes_read",
    "get_poll_statistics",
    "reply_to_part",
    "get_part_link",
    "list_peer_stories",
    "get_stories",
    "react_to_story",
    "post_story",
    "delete_story",
    "preview_story",
    "list_saved_dialogs",
    "get_saved_history",
    "list_saved_tags",
    "name_saved_tag",
    "list_quick_replies",
    "send_quick_reply",
    "inspect_sticker_set",
    "suggest_sticker_set_name",
    "add_sticker_to_set",
    "remove_sticker_from_set",
    "move_sticker_in_set",
    "install_sticker_set",
    "uninstall_sticker_set",
    "get_sticker_sets",
    "list_saved_gifs",
    "save_gif",
    "unsave_gif",
    "get_gif_search",
    "send_gif",
    "get_stars_receipt",
    "send_checklist",
    "add_checklist_tasks",
    "set_checklist_tasks_done",
    "remove_checklist_tasks",
    "get_checklist",
    "analyze_chat_statistics",
    "get_channel_statistics",
    "reorder_pinned_chats",
    "get_fact_check",
    "set_fact_check",
    "delete_fact_check",
    "get_business_settings",
    "set_business_hours",
    "set_business_away_message",
    "set_business_greeting",
    "set_business_intro",
    "set_business_location",
    "list_business_chat_links",
    "create_business_chat_link",
    "edit_business_chat_link",
    "delete_business_chat_link",
    "resolve_business_chat_link",
    "translate",
];
