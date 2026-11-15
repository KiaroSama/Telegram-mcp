mod admin;

use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const NAMES: &[&str] = &[
    "set_group_permissions",
    "set_member_exception",
    "remove_member_exception",
    "demote_admin",
    "promote_admin_group",
    "promote_admin_channel",
    "promote_admin_community",
    "edit_admin_rights_group",
    "edit_admin_rights_channel",
    "edit_admin_rights_community",
    "promote_admin",
    "edit_admin_rights",
];
const ADMIN: &[&str] = &[
    "change_info",
    "post_messages",
    "edit_messages",
    "delete_messages",
    "ban_users",
    "invite_users",
    "pin_messages",
    "add_admins",
    "anonymous",
    "manage_call",
    "other",
    "manage_topics",
    "post_stories",
    "edit_stories",
    "delete_stories",
    "manage_direct_messages",
    "manage_ranks",
    "manage_linked_peers",
    "manage_welcome_messages",
];
const GROUP: &[&str] = &[
    "change_info",
    "manage_welcome_messages",
    "delete_messages",
    "ban_users",
    "invite_users",
    "manage_topics",
    "pin_messages",
    "post_stories",
    "edit_stories",
    "delete_stories",
    "manage_call",
    "manage_ranks",
    "anonymous",
    "add_admins",
];
const CHANNEL: &[&str] = &[
    "change_info",
    "manage_welcome_messages",
    "post_messages",
    "edit_messages",
    "delete_messages",
    "post_stories",
    "edit_stories",
    "delete_stories",
    "invite_users",
    "manage_call",
    "manage_direct_messages",
    "add_admins",
    "ban_users",
];
const COMMUNITY: &[&str] = &[
    "change_info",
    "manage_linked_peers",
    "ban_users",
    "add_admins",
];
const ITEMS: &[(&str, &[&str])] = &[
    ("send_messages", &["send_plain"]),
    ("photos", &["send_photos"]),
    ("video_files", &["send_videos"]),
    ("video_messages", &["send_roundvideos"]),
    ("music", &["send_audios"]),
    ("voice_messages", &["send_voices"]),
    ("files", &["send_docs"]),
    (
        "stickers_and_gifs",
        &["send_stickers", "send_gifs", "send_games", "send_inline"],
    ),
    ("embed_links", &["embed_links"]),
    ("polls", &["send_polls"]),
    ("reactions", &["send_reactions"]),
    ("add_members", &["invite_users"]),
    ("create_topics", &["manage_topics"]),
    ("pin_messages", &["pin_messages"]),
    ("edit_own_tags", &["edit_rank"]),
    ("change_group_info", &["change_info"]),
];
fn requested(a: &Value) -> Result<BTreeMap<&'static str, bool>> {
    let mut given = BTreeMap::new();
    for (key, _) in ITEMS {
        if let Some(v) = a.get(*key).filter(|v| !v.is_null()) {
            given.insert(
                *key,
                v.as_bool()
                    .with_context(|| format!("{key} must be true or false"))?,
            );
        }
    }
    if let Some(value) = a.get("send_media").filter(|v| !v.is_null()) {
        let allowed = value
            .as_bool()
            .context("send_media must be true or false")?;
        for (key, _) in &ITEMS[1..11] {
            given.entry(*key).or_insert(allowed);
        }
    }
    ensure!(
        !(given.get("send_messages") == Some(&false) && given.get("embed_links") == Some(&true)),
        "Embed links cannot be allowed when Send messages is not allowed"
    );
    Ok(given)
}
fn allowed(rights: &Value) -> BTreeMap<&'static str, bool> {
    ITEMS
        .iter()
        .map(|(key, fields)| {
            let media = ITEMS[1..11].iter().any(|(k, _)| k == key);
            let restricted = rights["view_messages"] == true
                || fields.iter().any(|f| rights[*f] == true)
                || ((*key == "send_messages" || media) && rights["send_messages"] == true)
                || (media && rights["send_media"] == true);
            (*key, !restricted)
        })
        .collect()
}
fn banned(allowed: &BTreeMap<&str, bool>, until: i64) -> Value {
    let mut result = json!({"_":"chatBannedRights","until_date":until});
    for (key, fields) in ITEMS {
        if allowed.get(key) == Some(&false) {
            for field in *fields {
                result[*field] = true.into();
            }
        }
    }
    result
}
fn slow(value: &Value) -> Result<i64> {
    let seconds = if let Some(s) = value.as_str() {
        match s.trim().to_lowercase().as_str() {
            "off" => 0,
            "5s" => 5,
            "10s" => 10,
            "30s" => 30,
            "1m" => 60,
            "5m" => 300,
            "15m" => 900,
            "1h" => 3600,
            _ => int_value(value, "slow_mode")?,
        }
    } else {
        int_value(value, "slow_mode")?
    };
    ensure!(
        [0, 5, 10, 30, 60, 300, 900, 3600].contains(&seconds),
        "slow_mode must be off, 5s, 10s, 30s, 1m, 5m, 15m or 1h"
    );
    Ok(seconds)
}
fn group_refusal(info: &Value, given: &BTreeMap<&str, bool>) -> Result<()> {
    ensure!(
        kind(info) == "chat" || kind(info) == "channel" && info["megagroup"] == true,
        "Member permissions require a group, not a broadcast channel"
    );
    ensure!(
        !given.contains_key("create_topics") || info["forum"] == true,
        "Create topics exists only in forum groups"
    );
    let locked =
        given.get("pin_messages") == Some(&true) || given.get("change_group_info") == Some(&true);
    ensure!(
        !locked || info["username"].as_str().is_none_or(str::is_empty),
        "Pin messages and Change group info cannot be granted to all members in public groups"
    );
    ensure!(
        !locked || info["has_link"] != true,
        "Pin messages and Change group info cannot be granted to all members in discussion groups"
    );
    Ok(())
}
pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name.contains("admin") {
        return admin::run(api, name, a).await;
    }
    let given = requested(a)?;
    let seconds = a
        .get("slow_mode")
        .filter(|v| !v.is_null())
        .map(slow)
        .transpose()?;
    let boosts = a
        .get("do_not_restrict_boosters")
        .filter(|v| !v.is_null())
        .map(|v| int_value(v, "do_not_restrict_boosters"))
        .transpose()?;
    ensure!(
        boosts.is_none_or(|n| (0..=5).contains(&n)),
        "do_not_restrict_boosters must be 0 to 5"
    );
    let stars = a
        .get("charge_stars_per_message")
        .filter(|v| !v.is_null())
        .map(|v| int_value(v, "charge_stars_per_message"))
        .transpose()?;
    ensure!(
        stars.is_none_or(|n| n >= 0),
        "charge_stars_per_message must be zero or greater"
    );
    let until = a
        .get("until_date")
        .filter(|v| !v.is_null())
        .map(|v| int_value(v, "until_date"))
        .transpose()?;
    ensure!(
        until.is_none_or(|n| n >= 0),
        "until_date must be zero or a Unix timestamp"
    );
    ensure!(
        name == "remove_member_exception"
            || !given.is_empty()
            || seconds.is_some()
            || boosts.is_some()
            || stars.is_some()
            || until.is_some(),
        "Nothing to change: pass at least one permission or setting"
    );
    let p = peer(api, a, "chat_id").await?;
    let info = chat_info(api, &p).await?;
    group_refusal(&info, &given)?;
    if name == "set_group_permissions" {
        ensure!(
            is_channel(&p)
                || seconds.unwrap_or(0) == 0 && boosts.unwrap_or(0) == 0 && stars.unwrap_or(0) == 0,
            "Stars, boosters and slow mode require a supergroup"
        );
        let mut permissions = allowed(&info["default_banned_rights"]);
        permissions.extend(given.clone());
        if !permissions["send_messages"] {
            permissions.insert("embed_links", false);
        }
        if info["forum"] != true {
            permissions.remove("create_topics");
        }
        let rights = banned(&permissions, 0);
        let restricted = rights
            .as_object()
            .is_some_and(|m| m.iter().any(|(k, v)| k != "_" && v == true));
        ensure!(
            boosts.unwrap_or(0) == 0
                || restricted
                || seconds.is_some_and(|n| n > 0)
                || (seconds.is_none() && info["slowmode_enabled"] == true),
            "Boost exemptions require a restriction or slow mode"
        );
        let mut steps = Vec::new();
        if !given.is_empty() {
            steps.push((
                "Permissions",
                "messages.editChatDefaultBannedRights",
                json!({"peer":p,"banned_rights":rights}),
            ));
        }
        if is_channel(&p) {
            if let Some(seconds) = seconds {
                steps.push((
                    "Slow mode",
                    "channels.toggleSlowMode",
                    json!({"channel":channel(&p)?,"seconds":seconds}),
                ));
            }
            if let Some(boosts) = boosts {
                steps.push((
                    "Boost exemptions",
                    "channels.setBoostsToUnblockRestrictions",
                    json!({"channel":channel(&p)?,"boosts":boosts}),
                ));
            }
            if let Some(stars) = stars {
                steps.push((
                    "Message price",
                    "channels.updatePaidMessagesPrice",
                    json!({"channel":channel(&p)?,"send_paid_messages_stars":stars}),
                ));
            }
        }
        let mut applied = Vec::new();
        for (label, method, request) in steps {
            match api.invoke(method, request).await {
                Ok(_) => applied.push(label),
                Err(_) => {
                    return Ok(
                        json!({"complete":false,"applied":applied,"failed":label,"verified":false,"warning":"Earlier steps may already have changed this group"}),
                    );
                }
            }
        }
        return Ok(
            json!({"accepted":true,"applied":applied,"verified":false,"permissions":permissions}),
        );
    }
    ensure!(is_channel(&p), "Member exceptions require a supergroup");
    let u = peer(api, a, "user_id").await?;
    let current = api
        .invoke(
            "channels.getParticipant",
            json!({"channel":channel(&p)?,"participant":u}),
        )
        .await?;
    let own = &current["participant"]["banned_rights"];
    ensure!(
        own["view_messages"] != true,
        "The member was removed; use unban_user before setting exceptions"
    );
    if name == "remove_member_exception" {
        return api.invoke("channels.editBanned",json!({"channel":channel(&p)?,"participant":u,"banned_rights":{"_":"chatBannedRights","until_date":0}})).await;
    }
    let group = allowed(&info["default_banned_rights"]);
    for (key, on) in &given {
        ensure!(
            !*on || group[key],
            "{key} is disabled for all members in Group Permissions"
        );
    }
    if given.is_empty() {
        ensure!(own.is_object(), "This member has no exception to expire");
        let mut rights = own.clone();
        rights["until_date"] = json!(until.unwrap_or(0));
        return api
            .invoke(
                "channels.editBanned",
                json!({"channel":channel(&p)?,"participant":u,"banned_rights":rights}),
            )
            .await;
    }
    let mut permissions = if own.is_null() {
        group.clone()
    } else {
        allowed(own)
    };
    permissions.extend(given);
    for (key, on) in &mut permissions {
        *on &= group[key];
    }
    if !permissions["send_messages"] {
        permissions.insert("embed_links", false);
    }
    let expiry = until.unwrap_or(int(own, "until_date", 0)?);
    ensure!(
        until.is_some() || expiry == 0 || expiry.saturating_sub(now()?) > 30,
        "The existing member restriction has expired or expires within 30 seconds; refresh membership or give an explicit until_date before changing permissions"
    );
    api.invoke(
        "channels.editBanned",
        json!({"channel":channel(&p)?,"participant":u,"banned_rights":banned(&permissions,expiry)}),
    )
    .await
}
