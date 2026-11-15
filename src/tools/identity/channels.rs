use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub const NAMES: &[&str] = &[
    "set_join_to_send",
    "set_join_request",
    "set_prehistory_hidden",
    "set_participants_hidden",
    "set_signatures",
    "set_view_forum_as_messages",
    "toggle_channel_username",
    "reorder_channel_usernames",
    "deactivate_channel_usernames",
    "list_discussion_candidates",
    "set_anti_spam",
    "report_anti_spam_false_positive",
    "set_boosts_to_unblock",
    "set_channel_color",
    "set_channel_emoji_status",
    "set_channel_stickers",
    "set_channel_emoji_pack",
    "convert_to_gigagroup",
    "delete_channel",
    "set_channel_location",
    "set_autotranslation",
    "check_channel_username",
    "set_channel_username",
    "get_similar_channels",
    "set_discussion_group",
];

pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "list_discussion_candidates" {
        return api
            .invoke("channels.getGroupsForDiscussion", json!({}))
            .await;
    }
    if name == "set_discussion_group" {
        let p = peer(api, a, "channel_id").await?;
        let info = chat_info(api, &p).await?;
        ensure!(
            info["broadcast"] == true,
            "channel_id must name a broadcast channel"
        );
        let group = if let Some(v) = a.get("group_id").filter(|v| !v.is_null()) {
            let p = api.resolve_peer(v).await?;
            let info = chat_info(api, &p).await?;
            ensure!(info["megagroup"] == true, "group_id must name a supergroup");
            channel(&p)?
        } else {
            json!({"_":"inputChannelEmpty"})
        };
        return api
            .invoke(
                "channels.setDiscussionGroup",
                json!({"broadcast":channel(&p)?,"group":group}),
            )
            .await;
    }
    // Validate form and range before resolution, which may itself perform network I/O.
    let normalized = if matches!(name, "set_channel_username" | "check_channel_username") {
        Some(username(
            text(a, "username")?,
            name == "set_channel_username",
        )?)
    } else {
        None
    };
    let toggled = if name == "toggle_channel_username" {
        let raw = text(a, "username")?.trim().trim_start_matches('@');
        ensure!(!raw.is_empty(), "username must not be empty");
        Some(raw.to_owned())
    } else {
        None
    };
    let order = if name == "reorder_channel_usernames" {
        let values = array(a, "order")?;
        ensure!(
            !values.is_empty(),
            "order must contain at least one username"
        );
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for value in values {
            let username = value
                .as_str()
                .context("order must contain usernames")?
                .trim()
                .trim_start_matches('@');
            ensure!(
                !username.is_empty()
                    && username
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "order contains an invalid username"
            );
            ensure!(
                seen.insert(username.to_lowercase()),
                "order contains duplicate usernames"
            );
            result.push(username.to_owned());
        }
        Some(result)
    } else {
        None
    };
    if name == "set_channel_color"
        && let Some(v) = a.get("color").filter(|v| !v.is_null())
    {
        ensure!(
            int_value(v, "color")? >= 0,
            "color must be a palette index from zero upwards"
        );
    }
    let location = if name == "set_channel_location" {
        let lat = req(a, "latitude")?
            .as_f64()
            .context("latitude must be a number")?;
        let lon = req(a, "longitude")?
            .as_f64()
            .context("longitude must be a number")?;
        ensure!(
            lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon),
            "latitude must be -90..90 and longitude -180..180"
        );
        let address = text(a, "address")?.trim();
        ensure!(!address.is_empty(), "address must not be empty");
        Some((lat, lon, address))
    } else {
        None
    };
    if name == "set_boosts_to_unblock" {
        ensure!(int(a, "boosts", -1)? >= 0, "boosts must be zero or more");
    }
    if name == "report_anti_spam_false_positive" {
        ensure!(int(a, "message_id", 0)? > 0, "message_id must be positive");
    }
    let p = peer(api, a, "chat_id").await?;
    let c = channel(&p)?;
    let mut request = json!({"channel":c});
    let method = match name {
        "set_join_to_send" => "channels.toggleJoinToSend",
        "set_join_request" => "channels.toggleJoinRequest",
        "set_prehistory_hidden" => "channels.togglePreHistoryHidden",
        "set_participants_hidden" => "channels.toggleParticipantsHidden",
        "set_signatures" => "channels.toggleSignatures",
        "set_view_forum_as_messages" => "channels.toggleViewForumAsMessages",
        "toggle_channel_username" => "channels.toggleUsername",
        "reorder_channel_usernames" => "channels.reorderUsernames",
        "deactivate_channel_usernames" => "channels.deactivateAllUsernames",
        "set_anti_spam" => "channels.toggleAntiSpam",
        "report_anti_spam_false_positive" => "channels.reportAntiSpamFalsePositive",
        "set_boosts_to_unblock" => "channels.setBoostsToUnblockRestrictions",
        "set_channel_color" => "channels.updateColor",
        "set_channel_emoji_status" => "channels.updateEmojiStatus",
        "set_channel_stickers" => "channels.setStickers",
        "set_channel_emoji_pack" => "channels.setEmojiStickers",
        "convert_to_gigagroup" => "channels.convertToGigagroup",
        "delete_channel" => "channels.deleteChannel",
        "set_channel_location" => "channels.editLocation",
        "set_autotranslation" => "channels.toggleAutotranslation",
        "check_channel_username" => "channels.checkUsername",
        "set_channel_username" => "channels.updateUsername",
        "get_similar_channels" => "channels.getChannelRecommendations",
        _ => unreachable!(),
    };
    match name {
        "set_join_to_send"
        | "set_join_request"
        | "set_view_forum_as_messages"
        | "set_anti_spam"
        | "set_autotranslation" => {
            request["enabled"] = json!(
                req(a, "enabled")?
                    .as_bool()
                    .context("enabled must be boolean")?
            )
        }
        "set_prehistory_hidden" | "set_participants_hidden" => {
            request["enabled"] = json!(
                req(a, "hidden")?
                    .as_bool()
                    .context("hidden must be boolean")?
            )
        }
        "set_signatures" => {
            let enabled = req(a, "enabled")?
                .as_bool()
                .context("enabled must be boolean")?;
            request["signatures_enabled"] = enabled.into();
            request["profiles_enabled"] = json!(enabled && boolean(a, "show_profiles", false)?);
        }
        "toggle_channel_username" => {
            request["username"] = json!(toggled);
            request["active"] = json!(
                req(a, "active")?
                    .as_bool()
                    .context("active must be boolean")?
            );
        }
        "reorder_channel_usernames" => request["order"] = json!(order),
        "report_anti_spam_false_positive" => request["msg_id"] = json!(int(a, "message_id", 0)?),
        "set_boosts_to_unblock" => request["boosts"] = json!(int(a, "boosts", 0)?),
        "set_channel_color" => {
            copy_present(a, &mut request, &["color", "background_emoji_id"]);
            request["for_profile"] = json!(boolean(a, "for_profile", false)?);
        }
        "set_channel_emoji_status" => {
            request["emoji_status"] =
                if let Some(v) = a.get("custom_emoji_id").filter(|v| !v.is_null()) {
                    let id = int_value(v, "custom_emoji_id")?;
                    if id == 0 {
                        json!({"_":"emojiStatusEmpty"})
                    } else {
                        json!({"_":"emojiStatus","document_id":id.to_string()})
                    }
                } else {
                    json!({"_":"emojiStatusEmpty"})
                };
        }
        "set_channel_stickers" | "set_channel_emoji_pack" => {
            let raw = string(a, "set_short_name", "")?;
            let name = raw
                .trim()
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("");
            request["stickerset"] = if name.is_empty() {
                json!({"_":"inputStickerSetEmpty"})
            } else {
                json!({"_":"inputStickerSetShortName","short_name":name})
            };
        }
        "set_channel_location" => {
            let (lat, lon, address) = location.context("validated location")?;
            request["geo_point"] = json!({"_":"inputGeoPoint","lat":lat,"long":lon});
            request["address"] = address.into();
        }
        "check_channel_username" | "set_channel_username" => {
            request["username"] = json!(normalized)
        }
        _ => {}
    }
    if name == "set_channel_username" && normalized.as_ref().is_some_and(|v| !v.is_empty()) {
        let free = api
            .invoke(
                "channels.checkUsername",
                json!({"channel":c,"username":normalized}),
            )
            .await?;
        ensure!(
            free == true,
            "Username is already taken; channel username was not changed"
        );
    }
    api.invoke(method, request).await
}
