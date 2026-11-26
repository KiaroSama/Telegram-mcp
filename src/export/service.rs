//! Telegram Desktop service-action descriptions for HTML and JSON exports.

use super::files::clean_relative_path;
use super::render::{html_escape, person_name, relative_href};
use crate::text_fidelity::{
    DEFAULT_MAX_NAME_CHARS, DEFAULT_MAX_TEXT_CHARS, display_name, display_text,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct ServiceProjection {
    pub action_slug: Option<String>,
    pub summary_text: String,
    pub html_body: String,
    pub extra_json: Map<String, Value>,
}

fn str_id(value: &Value) -> Option<String> {
    if let Some(n) = value.as_i64() {
        Some(n.to_string())
    } else {
        value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
    }
}

fn user_display(user_id: &str, peers: &BTreeMap<String, Value>) -> String {
    peers
        .get(&format!("user:{user_id}"))
        .or_else(|| peers.get(user_id))
        .map(person_name)
        .unwrap_or_else(|| format!("user{user_id}"))
}

fn user_names(list: &Value, peers: &BTreeMap<String, Value>) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(str_id)
        .map(|uid| user_display(&uid, peers))
        .collect()
}

fn quoted_html(title: &str) -> String {
    format!("&laquo;{}&raquo;", html_escape(title))
}

fn format_ttl_days(seconds: i64) -> String {
    if seconds <= 0 {
        return "disabled".into();
    }
    let days = seconds / 86_400;
    if days >= 1 && seconds % 86_400 == 0 {
        return if days == 1 {
            "1 day".into()
        } else {
            format!("{days} days")
        };
    }
    let hours = seconds / 3_600;
    if hours >= 1 && seconds % 3_600 == 0 {
        return if hours == 1 {
            "1 hour".into()
        } else {
            format!("{hours} hours")
        };
    }
    format!("{seconds} seconds")
}

fn discard_reason_slug(reason: &Value) -> Option<&'static str> {
    match reason["_"].as_str().unwrap_or_default() {
        "phoneCallDiscardReasonBusy" => Some("busy"),
        "phoneCallDiscardReasonDisconnect" => Some("disconnect"),
        "phoneCallDiscardReasonHangup" => Some("hangup"),
        "phoneCallDiscardReasonMissed" => Some("missed"),
        _ => None,
    }
}

pub fn project_service_action(
    message: &Value,
    peers: &BTreeMap<String, Value>,
    actor_name: &str,
) -> Option<ServiceProjection> {
    let action = message.get("action").filter(|a| a.is_object())?;
    let raw_kind = action["_"].as_str().unwrap_or("messageActionUnknown");
    let safe_kind = display_name(Some(raw_kind), DEFAULT_MAX_NAME_CHARS);
    let actor = if actor_name.is_empty() {
        "Telegram"
    } else {
        actor_name
    };
    let actor_html = html_escape(actor);
    let is_channel = message["peer_id"]["_"].as_str() == Some("peerChannel")
        && message.get("from_id").is_none_or(Value::is_null);
    let reply_msg_id = message["reply_to"]["reply_to_msg_id"].as_i64();
    let mut extra = Map::new();

    let (slug, summary, mut html) = match raw_kind {
        "messageActionChatCreate" => {
            let title = display_name(action["title"].as_str(), DEFAULT_MAX_NAME_CHARS);
            let members = user_names(&action["users"], peers);
            extra.insert("title".into(), Value::String(title.clone()));
            extra.insert("members".into(), json!(members));
            let suffix = if members.is_empty() {
                String::new()
            } else {
                format!(" with members {}", members.join(", "))
            };
            (
                Some("create_group"),
                format!("{actor} created group \"{title}\"{suffix}"),
                format!(
                    "{actor_html} created group {}{}",
                    quoted_html(&title),
                    html_escape(&suffix)
                ),
            )
        }
        "messageActionChannelCreate" => {
            let title = display_name(action["title"].as_str(), DEFAULT_MAX_NAME_CHARS);
            extra.insert("title".into(), Value::String(title.clone()));
            (
                Some("create_channel"),
                format!("Channel \"{title}\" created"),
                format!("Channel {} created", quoted_html(&title)),
            )
        }
        "messageActionChatEditTitle" => {
            let title = display_name(action["title"].as_str(), DEFAULT_MAX_NAME_CHARS);
            extra.insert("title".into(), Value::String(title.clone()));
            if is_channel {
                (
                    Some("edit_group_title"),
                    format!("Channel title changed to \"{title}\""),
                    format!("Channel title changed to {}", quoted_html(&title)),
                )
            } else {
                (
                    Some("edit_group_title"),
                    format!("{actor} changed group title to \"{title}\""),
                    format!(
                        "{actor_html} changed group title to {}",
                        quoted_html(&title)
                    ),
                )
            }
        }
        "messageActionChatEditPhoto" | "messageActionSuggestProfilePhoto" => {
            let suggest = raw_kind == "messageActionSuggestProfilePhoto";
            let slug = if suggest {
                "suggest_profile_photo"
            } else {
                "edit_group_photo"
            };
            let plain = if suggest {
                format!("{actor} suggested a new profile photo")
            } else if is_channel {
                "Channel photo changed".into()
            } else {
                format!("{actor} changed group photo")
            };
            (Some(slug), plain.clone(), html_escape(&plain))
        }
        "messageActionChatDeletePhoto" => {
            let plain = if is_channel {
                "Channel photo removed".into()
            } else {
                format!("{actor} removed group photo")
            };
            (
                Some("delete_group_photo"),
                plain.clone(),
                html_escape(&plain),
            )
        }
        "messageActionChatAddUser" | "messageActionInviteToGroupCall" => {
            let call = raw_kind == "messageActionInviteToGroupCall";
            let members = user_names(&action["users"], peers);
            extra.insert("members".into(), json!(members));
            let joined = members.join(", ");
            let verb = if call {
                "invited to video chat"
            } else {
                "invited"
            };
            let plain = format!("{actor} {verb} {joined}");
            (
                Some(if call {
                    "invite_to_group_call"
                } else {
                    "invite_members"
                }),
                plain.clone(),
                html_escape(&plain),
            )
        }
        "messageActionChatDeleteUser" => {
            let target = str_id(&action["user_id"])
                .map(|uid| user_display(&uid, peers))
                .unwrap_or_else(|| "member".into());
            extra.insert("members".into(), json!([target]));
            let plain = format!("{actor} removed {target}");
            (Some("remove_members"), plain.clone(), html_escape(&plain))
        }
        "messageActionChatJoinedByLink" => {
            let inviter = str_id(&action["inviter_id"])
                .map(|uid| user_display(&uid, peers))
                .unwrap_or_else(|| "inviter".into());
            extra.insert("inviter".into(), Value::String(inviter.clone()));
            let plain = format!("{actor} joined group by link from {inviter}");
            (
                Some("join_group_by_link"),
                plain.clone(),
                html_escape(&plain),
            )
        }
        "messageActionChatJoinedByRequest" => {
            let plain = format!("{actor} was accepted into the group");
            (
                Some("join_group_by_request"),
                plain.clone(),
                html_escape(&plain),
            )
        }
        "messageActionChatMigrateTo" => {
            let plain = format!("{actor} converted this group to a supergroup");
            (
                Some("migrate_to_supergroup"),
                plain.clone(),
                html_escape(&plain),
            )
        }
        "messageActionChannelMigrateFrom" => {
            let title = display_name(action["title"].as_str(), DEFAULT_MAX_NAME_CHARS);
            extra.insert("title".into(), Value::String(title.clone()));
            (
                Some("migrate_from_group"),
                format!("{actor} converted basic group \"{title}\" to this supergroup"),
                format!(
                    "{actor_html} converted a basic group to this supergroup {}",
                    quoted_html(&title)
                ),
            )
        }
        "messageActionPinMessage" => {
            if let Some(mid) = reply_msg_id {
                extra.insert("message_id".into(), json!(mid));
                (
                    Some("pin_message"),
                    format!("{actor} pinned message #{mid}"),
                    format!("{actor_html} pinned <a href=\"#message{mid}\">this message</a>"),
                )
            } else {
                let plain = format!("{actor} pinned a message");
                (Some("pin_message"), plain.clone(), html_escape(&plain))
            }
        }
        "messageActionHistoryClear" => (
            Some("clear_history"),
            "History cleared".into(),
            "History cleared".into(),
        ),
        "messageActionScreenshotTaken" => {
            let plain = format!("{actor} took a screenshot");
            (Some("take_screenshot"), plain.clone(), html_escape(&plain))
        }
        "messageActionContactSignUp" => {
            let plain = format!("{actor} joined Telegram");
            (Some("joined_telegram"), plain.clone(), html_escape(&plain))
        }
        "messageActionGameScore" => {
            let score = action["score"].as_i64().unwrap_or(0);
            extra.insert("score".into(), json!(score));
            if let Some(mid) = reply_msg_id {
                extra.insert("game_message_id".into(), json!(mid));
            }
            let plain = format!("{actor} scored {score} in a game");
            (Some("score_in_game"), plain.clone(), html_escape(&plain))
        }
        "messageActionPhoneCall" => {
            let duration = action["duration"].as_i64().unwrap_or(0);
            if duration > 0 {
                extra.insert("duration_seconds".into(), json!(duration));
            }
            if let Some(reason) = discard_reason_slug(&action["reason"]) {
                extra.insert("discard_reason".into(), Value::String(reason.into()));
            }
            let status = if duration > 0 {
                format!("Call ({duration} seconds)")
            } else {
                "Call".into()
            };
            (Some("phone_call"), status.clone(), html_escape(&status))
        }
        "messageActionSetMessagesTTL" => {
            let period = action["period"].as_i64().unwrap_or(0);
            extra.insert("period".into(), json!(period));
            let label = format_ttl_days(period);
            let plain = if period <= 0 {
                format!("{actor} disabled auto-delete timer")
            } else {
                format!("{actor} set messages to auto-delete in {label}")
            };
            (Some("set_messages_ttl"), plain.clone(), html_escape(&plain))
        }
        "messageActionTopicCreate" => {
            let title = display_name(action["title"].as_str(), DEFAULT_MAX_NAME_CHARS);
            extra.insert("title".into(), Value::String(title.clone()));
            (
                Some("topic_created"),
                format!("{actor} created topic \"{title}\""),
                format!("{actor_html} created topic {}", quoted_html(&title)),
            )
        }
        "messageActionTopicEdit" => {
            let mut changes = Vec::new();
            if let Some(t) = action["title"].as_str() {
                let clean = display_name(Some(t), DEFAULT_MAX_NAME_CHARS);
                extra.insert("new_title".into(), Value::String(clean.clone()));
                changes.push(format!("title to \"{clean}\""));
            }
            if let Some(closed) = action["closed"].as_bool() {
                changes.push(if closed {
                    "closed".into()
                } else {
                    "reopened".into()
                });
            }
            if let Some(hidden) = action["hidden"].as_bool() {
                changes.push(if hidden {
                    "hidden".into()
                } else {
                    "unhidden".into()
                });
            }
            let detail = if changes.is_empty() {
                "updated topic".into()
            } else {
                format!("updated topic ({})", changes.join(", "))
            };
            let plain = format!("{actor} {detail}");
            (Some("topic_edited"), plain.clone(), html_escape(&plain))
        }
        "messageActionCustomAction" => {
            let info = display_text(action["message"].as_str(), DEFAULT_MAX_TEXT_CHARS);
            extra.insert("information_text".into(), Value::String(info.clone()));
            (Some("custom_action"), info.clone(), html_escape(&info))
        }
        _ => {
            let fallback = format!("Service action: {safe_kind}");
            (None, fallback.clone(), html_escape(&fallback))
        }
    };

    if let Some(photo_rel) = message
        .get("export_service_photo")
        .and_then(Value::as_str)
        .and_then(clean_relative_path)
        && let Some(href) = relative_href(&photo_rel)
    {
        extra.insert("photo".into(), Value::String(photo_rel.clone()));
        html.push_str(&format!(
            " <a class=\"media service-photo\" href=\"{}\">{}</a>",
            html_escape(&href),
            html_escape(&photo_rel)
        ));
    }

    let html_body = format!(
        "<div class=\"service details\" data-action=\"{}\">{}</div>",
        html_escape(&safe_kind),
        html
    );

    Some(ServiceProjection {
        action_slug: slug.map(str::to_owned),
        summary_text: summary,
        html_body,
        extra_json: extra,
    })
}
