//! Account identity, contacts, group administration and community tools.

mod base;
mod channels;
mod chats;
mod communities;
mod compatibility;
mod contacts;
mod groups;
mod invites;
mod notifications;
mod profile;
mod rights;

use crate::backend::Telegram;
use anyhow::Result;
use serde_json::Value;

pub fn invite_link_qr_in(state: &crate::state::State, args: &Value) -> Result<Value> {
    invites::qr_in(state, args)
}

/// One peer-selector normalization shared by the handler and access policy.
/// Only public username links are accepted here; private invites never resolve.
pub(crate) fn normalize_lookup_query(query: &Value) -> Result<Value> {
    let Some(raw) = query.as_str() else {
        return Ok(query.clone());
    };
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let prefix = [
        "https://t.me/",
        "http://t.me/",
        "t.me/",
        "https://telegram.me/",
        "http://telegram.me/",
        "telegram.me/",
    ]
    .into_iter()
    .find(|prefix| lower.starts_with(prefix));
    let Some(prefix) = prefix else {
        return Ok(query.clone());
    };
    let name = trimmed[prefix.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    anyhow::ensure!(
        !name.is_empty(),
        "The public link contains no peer username"
    );
    anyhow::ensure!(
        !name.starts_with('+') && !name.eq_ignore_ascii_case("joinchat"),
        "This is a private invite link, not a peer username; use import_chat_invite or join_chat_by_link"
    );
    Ok(Value::String(name.to_owned()))
}

pub const TOOL_NAMES: &[&str] = &[
    "add_chat_to_community",
    "add_contact",
    "approve_community_link_request",
    "approve_join_request",
    "archive_chat",
    "ban_community_member",
    "ban_user",
    "block_user",
    "check_channel_username",
    "clear_chat_history",
    "convert_to_gigagroup",
    "create_channel",
    "create_community",
    "create_group",
    "create_invite_link",
    "create_paid_invite_link",
    "deactivate_channel_usernames",
    "delete_channel",
    "delete_chat",
    "delete_chat_photo",
    "delete_community",
    "delete_community_photo",
    "delete_contact",
    "delete_profile_photo",
    "delete_revoked_invite_links",
    "demote_admin",
    "edit_admin_rights_channel",
    "edit_admin_rights_community",
    "edit_admin_rights_group",
    "edit_admin_rights",
    "edit_chat_about",
    "edit_chat_photo",
    "edit_chat_title",
    "edit_invite_link",
    "export_chat_invite",
    "export_contacts",
    "get_admins",
    "get_banned_users",
    "get_blocked_users",
    "get_bot_commands",
    "get_bot_info",
    "get_chat",
    "get_chats",
    "get_common_chats",
    "get_community_chats",
    "get_community_link_requests",
    "get_contact_chats",
    "get_contact_ids",
    "get_direct_chat_by_contact",
    "get_full_chat",
    "get_full_user",
    "get_invite_link",
    "get_invite_link_qr",
    "get_last_interaction",
    "get_me",
    "get_member_admin_status",
    "get_message_link",
    "get_participants",
    "get_privacy_settings",
    "get_recent_actions",
    "get_similar_channels",
    "get_user_photos",
    "get_user_status",
    "import_chat_invite",
    "import_contacts",
    "invite_to_group",
    "join_chat_by_link",
    "kick_user",
    "leave_chat",
    "list_authorizations",
    "list_chats",
    "list_contacts",
    "list_discussion_candidates",
    "list_invite_link_admins",
    "list_invite_links",
    "list_join_requests",
    "list_link_joins",
    "list_my_communities",
    "list_saved_sounds",
    "lookup_peer",
    "mark_chat_unread",
    "mark_topic_as_read",
    "mute_chat",
    "pin_chat",
    "promote_admin_channel",
    "promote_admin_community",
    "promote_admin_group",
    "promote_admin",
    "reject_community_link_request",
    "remove_chat_from_community",
    "remove_member_exception",
    "remove_saved_sound",
    "remove_user",
    "rename_community",
    "reorder_channel_usernames",
    "replace_chat_photo",
    "report_anti_spam_false_positive",
    "resolve_username",
    "resolve_phone",
    "revoke_invite_link",
    "search_contacts",
    "search_my_chats",
    "search_public_chats",
    "send_contact",
    "set_anti_spam",
    "set_authorization_calls",
    "set_authorization_secret_chats",
    "set_autotranslation",
    "set_boosts_to_unblock",
    "set_bot_commands",
    "set_bot_info",
    "invite_users_to_bot",
    "set_channel_color",
    "set_channel_emoji_pack",
    "set_channel_emoji_status",
    "set_channel_location",
    "set_channel_stickers",
    "set_channel_username",
    "set_chat_sound_disabled",
    "set_chat_tone",
    "set_community_photo",
    "set_community_who_can_add",
    "set_discussion_group",
    "set_default_chat_permissions",
    "set_group_permissions",
    "set_join_request",
    "set_join_to_send",
    "set_member_exception",
    "set_member_tag",
    "set_my_username",
    "set_participants_hidden",
    "set_prehistory_hidden",
    "set_privacy_settings",
    "set_profile_photo",
    "set_secret_chats_only_device",
    "set_signatures",
    "set_view_forum_as_messages",
    "subscribe_public_channel",
    "terminate_authorization",
    "toggle_channel_username",
    "toggle_slow_mode",
    "unarchive_chat",
    "unban_community_member",
    "unban_user",
    "unblock_user",
    "unmute_chat",
    "unpin_chat",
    "update_profile",
    "upgrade_group_to_supergroup",
];

pub fn names() -> Vec<&'static str> {
    [
        contacts::NAMES,
        chats::NAMES,
        groups::NAMES,
        rights::NAMES,
        invites::NAMES,
        channels::NAMES,
        communities::NAMES,
        notifications::NAMES,
        profile::NAMES,
        compatibility::NAMES,
    ]
    .concat()
}

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if compatibility::NAMES.contains(&name) {
        return Some(compatibility::run(api, name, args).await);
    }
    if contacts::NAMES.contains(&name) {
        return Some(contacts::run(api, name, args).await);
    }
    if chats::NAMES.contains(&name) {
        return Some(chats::run(api, name, args).await);
    }
    if groups::NAMES.contains(&name) {
        return Some(groups::run(api, name, args).await);
    }
    if rights::NAMES.contains(&name) {
        return Some(rights::run(api, name, args).await);
    }
    if invites::NAMES.contains(&name) {
        return Some(invites::run(api, name, args).await);
    }
    if channels::NAMES.contains(&name) {
        return Some(channels::run(api, name, args).await);
    }
    if communities::NAMES.contains(&name) {
        return Some(communities::run(api, name, args).await);
    }
    if notifications::NAMES.contains(&name) {
        return Some(notifications::run(api, name, args).await);
    }
    if profile::NAMES.contains(&name) {
        return Some(profile::run(api, name, args).await);
    }
    None
}
