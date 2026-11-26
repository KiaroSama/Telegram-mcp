//! Typed Telegram Desktop Recent Actions (`ChannelAdminLogEvent`) mapper.

mod actions;

use super::admin_log_text::{
    AdminLogPart, AdminTextPart, LogChat, default_rights_change, format_history_ttl,
    invite_link_change, invite_link_text, json_i64, plural, tl_kind, tr, unknown_action_sentence,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminActor {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTarget {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTextEntity {
    pub kind: String,
    pub offset: usize,
    pub length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<i64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<MessageTextEntity>,
    pub has_caption_media: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_file: Option<String>,
    pub pinned: bool,
}

macro_rules! define_action_kind {
    ($(($variant:ident, $wire:literal)),* $(,)?) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum AdminActionKind {
            $($variant,)*
            Unknown(String),
        }
        impl AdminActionKind {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)*
                    Self::Unknown(raw) => raw.as_str(),
                }
            }
            pub const fn is_unknown(&self) -> bool {
                matches!(self, Self::Unknown(_))
            }
        }
    };
}

#[rustfmt::skip]
define_action_kind!(
    (ChangeTitle, "change_title"), (ChangeAbout, "change_about"), (ChangeUsername, "change_username"),
    (ChangePhoto, "change_photo"), (ToggleInvites, "toggle_invites"), (ToggleSignatures, "toggle_signatures"),
    (UpdatePinned, "update_pinned"), (EditMessage, "edit_message"), (DeleteMessage, "delete_message"),
    (ParticipantJoin, "participant_join"), (ParticipantLeave, "participant_leave"),
    (ParticipantInvite, "participant_invite"), (ParticipantToggleBan, "participant_toggle_ban"),
    (ParticipantToggleAdmin, "participant_toggle_admin"), (ChangeStickerSet, "change_sticker_set"),
    (ChangeEmojiStickerSet, "change_emoji_sticker_set"), (TogglePreHistoryHidden, "toggle_pre_history_hidden"),
    (DefaultBannedRights, "default_banned_rights"), (StopPoll, "stop_poll"),
    (ChangeLinkedChat, "change_linked_chat"), (ChangeLocation, "change_location"),
    (ToggleSlowMode, "toggle_slow_mode"), (StartGroupCall, "start_group_call"),
    (DiscardGroupCall, "discard_group_call"), (ParticipantMute, "participant_mute"),
    (ParticipantUnmute, "participant_unmute"), (ToggleGroupCallSetting, "toggle_group_call_setting"),
    (ParticipantJoinByInvite, "participant_join_by_invite"), (ExportedInviteDelete, "exported_invite_delete"),
    (ExportedInviteRevoke, "exported_invite_revoke"), (ExportedInviteEdit, "exported_invite_edit"),
    (ParticipantVolume, "participant_volume"), (ChangeHistoryTtl, "change_history_ttl"),
    (ParticipantJoinByRequest, "participant_join_by_request"), (ToggleNoForwards, "toggle_no_forwards"),
    (SendMessage, "send_message"), (ChangeAvailableReactions, "change_available_reactions"),
    (ChangeUsernames, "change_usernames"), (ToggleForum, "toggle_forum"),
    (CreateTopic, "create_topic"), (EditTopic, "edit_topic"), (DeleteTopic, "delete_topic"),
    (PinTopic, "pin_topic"), (ToggleAntiSpam, "toggle_anti_spam"),
    (ChangePeerColor, "change_peer_color"), (ChangeProfilePeerColor, "change_profile_peer_color"),
    (ChangeWallpaper, "change_wallpaper"), (ChangeEmojiStatus, "change_emoji_status"),
    (ToggleSignatureProfiles, "toggle_signature_profiles"), (ParticipantSubExtend, "participant_sub_extend"),
    (ToggleAutotranslation, "toggle_autotranslation"), (ParticipantEditRank, "participant_edit_rank"),
);

#[rustfmt::skip]
const TOGGLES: &[(&str, &str, &str, &str, &str, AdminActionKind)] = &[
    ("ToggleInvites", "new_value", "invites", "invites_enabled", "invites_disabled", AdminActionKind::ToggleInvites),
    ("ToggleSignatures", "new_value", "signatures", "signatures_enabled", "signatures_disabled", AdminActionKind::ToggleSignatures),
    ("ToggleForum", "new_value", "topics", "topics_enabled", "topics_disabled", AdminActionKind::ToggleForum),
    ("ToggleAntiSpam", "new_value", "antispam", "antispam_enabled", "antispam_disabled", AdminActionKind::ToggleAntiSpam),
    ("ToggleSignatureProfiles", "new_value", "signature_profiles", "signature_profiles_enabled", "signature_profiles_disabled", AdminActionKind::ToggleSignatureProfiles),
    ("ToggleAutotranslation", "new_value", "autotranslate", "autotranslate_enabled", "autotranslate_disabled", AdminActionKind::ToggleAutotranslation),
    ("TogglePreHistoryHidden", "new_value", "prehistory_hidden", "history_made_hidden", "history_made_visible", AdminActionKind::TogglePreHistoryHidden),
    ("ToggleNoForwards", "new_value", "no_forwards", "forwards_disabled", "forwards_enabled", AdminActionKind::ToggleNoForwards),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLogEntry {
    pub event_id: String,
    pub timestamp: i64,
    pub actor: AdminActor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<AdminTarget>,
    pub action_kind: AdminActionKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_changes: Vec<FieldChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<MessageSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<MessageSnapshot>,
}

pub fn sanitize_action_type(raw: &str) -> String {
    let s: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '_' | '.'))
        .take(96)
        .collect();
    if s.is_empty() {
        "unknown_action".into()
    } else {
        s
    }
}

pub(super) fn str_id(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(json_i64).map(|n| n.to_string())
}

pub(super) fn i64_or0(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(json_i64).unwrap_or(0)
}

pub(super) fn peer_dec(p: &Value) -> Option<String> {
    json_i64(p)
        .or_else(|| p.get("user_id").and_then(json_i64))
        .or_else(|| p.get("channel_id").and_then(json_i64))
        .or_else(|| p.get("chat_id").and_then(json_i64))
        .map(|n| n.to_string())
}

pub fn extract_message_snapshot(message: &Value) -> Option<MessageSnapshot> {
    let k = message.is_object().then(|| tl_kind(message))?;
    let valid = k.is_empty()
        || k.eq_ignore_ascii_case("message")
        || k.eq_ignore_ascii_case("messageService");
    if !valid || k.eq_ignore_ascii_case("messageEmpty") {
        return None;
    }
    let entities = message["entities"]
        .as_array()
        .map(|a| a.iter().filter_map(parse_entity).collect())
        .unwrap_or_default();
    let (media, mt) = (&message["media"], tl_kind(&message["media"]));
    let is_photo = mt.eq_ignore_ascii_case("messageMediaPhoto")
        && tl_kind(&media["photo"]).eq_ignore_ascii_case("photo");
    let is_doc = mt.eq_ignore_ascii_case("messageMediaDocument")
        && tl_kind(&media["document"]).eq_ignore_ascii_case("document");
    let (media_id, media_kind) = if is_photo {
        (str_id(&media["photo"], "id"), Some("photo".into()))
    } else if is_doc {
        (str_id(&media["document"], "id"), Some("document".into()))
    } else {
        let other = (!mt.is_empty() && !mt.eq_ignore_ascii_case("messageMediaEmpty"))
            .then(|| sanitize_action_type(mt));
        (None, other)
    };
    Some(MessageSnapshot {
        message_id: str_id(message, "id"),
        sender_id: peer_dec(&message["from_id"]).or_else(|| peer_dec(&message["peer_id"])),
        date: message.get("date").and_then(json_i64),
        text: message["message"].as_str().unwrap_or("").to_owned(),
        entities,
        has_caption_media: is_photo || is_doc,
        media_id,
        media_kind,
        export_file: message["export_file"].as_str().map(str::to_owned),
        thumbnail_file: message["export_thumb"].as_str().map(str::to_owned),
        pinned: message["pinned"].as_bool().unwrap_or(false),
    })
}

fn parse_entity(e: &Value) -> Option<MessageTextEntity> {
    Some(MessageTextEntity {
        kind: sanitize_action_type(tl_kind(e)),
        offset: e.get("offset").and_then(Value::as_u64)? as usize,
        length: e.get("length").and_then(Value::as_u64)? as usize,
        url: e["url"].as_str().map(str::to_owned),
        language: e["language"].as_str().map(str::to_owned),
        document_id: str_id(e, "document_id"),
    })
}

pub fn map_events(events: &[Value], chat: &LogChat) -> Vec<AdminLogEntry> {
    events.iter().map(|e| map_event(e, chat)).collect()
}

pub fn map_events_chronological(events: &[Value], chat: &LogChat) -> Vec<AdminLogEntry> {
    let mut list: Vec<_> = events
        .iter()
        .map(|e| (i64_or0(e, "date"), i64_or0(e, "id"), map_event(e, chat)))
        .collect();
    list.sort_by_key(|(ts, id, _)| (*ts, *id));
    list.into_iter().map(|(_, _, entry)| entry).collect()
}

pub fn map_event(event: &Value, chat: &LogChat) -> AdminLogEntry {
    map_event_parts(event, chat).0
}

pub(super) struct Ev<'a> {
    pub chat: &'a LogChat,
    pub who: String,
    pub actor_uid: i64,
    pub target: Option<AdminTarget>,
    pub changes: Vec<FieldChange>,
    pub before: Option<MessageSnapshot>,
    pub after: Option<MessageSnapshot>,
    pub parts: Vec<AdminLogPart>,
    pub fallback: Option<String>,
}

impl<'a> Ev<'a> {
    pub fn say0(&mut self, key: &str) {
        self.parts
            .push(AdminLogPart::line(tr(key, &[("from", &self.who)])));
    }

    pub fn say(&mut self, key: &str, extra: &[(&str, &str)]) {
        let mut vars = vec![("from", self.who.as_str())];
        vars.extend_from_slice(extra);
        self.parts.push(AdminLogPart::line(tr(key, &vars)));
    }

    pub fn chg(&mut self, f: &str, b: impl Into<String>, a: impl Into<String>) {
        let (bs, as_) = (b.into(), a.into());
        if bs != as_ {
            self.changes.push(FieldChange {
                field: f.into(),
                before: Some(bs),
                after: Some(as_),
            });
        }
    }

    pub fn tgt(&mut self, kind: &str, id: Option<String>, label: impl Into<String>) {
        self.target = Some(AdminTarget {
            kind: kind.into(),
            id,
            label: label.into(),
        });
    }
}

pub fn map_event_parts(event: &Value, chat: &LogChat) -> (AdminLogEntry, Vec<AdminLogPart>) {
    let event_id = str_id(event, "id").unwrap_or_else(|| "0".into());
    let (timestamp, actor_uid) = (i64_or0(event, "date"), i64_or0(event, "user_id"));
    let who = chat.user_name(actor_uid);
    let actor = AdminActor {
        id: actor_uid.to_string(),
        name: who.clone(),
        username: chat.peer_username(&json!({"_": "peerUser", "user_id": actor_uid})),
    };
    let mut ev = Ev {
        chat,
        who,
        actor_uid,
        target: None,
        changes: Vec::new(),
        before: None,
        after: None,
        parts: Vec::new(),
        fallback: None,
    };
    let action = &event["action"];
    let raw_kind = tl_kind(action);
    let short = raw_kind
        .strip_prefix("channelAdminLogEventAction")
        .or_else(|| raw_kind.strip_prefix("ChannelAdminLogEventAction"))
        .unwrap_or("");
    let place = if chat.megagroup { "group" } else { "channel" };
    let call_sfx = if chat.broadcast { "_channel" } else { "" };
    let grp_sfx = if chat.megagroup { "" } else { "_channel" };
    let sstr = |k: &str| action[k].as_str().unwrap_or("");

    let kind = if let Some((_, vk, field, tk, fk, k)) = TOGGLES.iter().find(|r| r.0 == short) {
        let on = action[*vk].as_bool().unwrap_or(false);
        ev.say0(if on { tk } else { fk });
        ev.chg(field, (!on).to_string(), on.to_string());
        k.clone()
    } else {
        match short {
            "ChangeTitle" => {
                let (prev, nv) = (sstr("prev_value"), sstr("new_value"));
                let k = if chat.megagroup {
                    "action_changed_title"
                } else {
                    "changed_title_channel"
                };
                ev.say(k, &[("title", nv)]);
                ev.chg("title", prev, nv);
                AdminActionKind::ChangeTitle
            }
            "ChangeAbout" => {
                let (prev, nv) = (sstr("prev_value"), sstr("new_value"));
                let verb = if nv.is_empty() { "removed" } else { "changed" };
                ev.say0(&format!("{verb}_description_{place}"));
                let note = if nv.is_empty() {
                    Vec::new()
                } else {
                    vec![AdminTextPart::plain(nv)]
                };
                ev.parts.push(AdminLogPart::note(note));
                if !prev.is_empty() {
                    let prev_lbl = tr("previous_description", &[]);
                    ev.parts.push(AdminLogPart::orig(
                        prev_lbl,
                        vec![AdminTextPart::plain(prev)],
                    ));
                }
                ev.chg("about", prev, nv);
                AdminActionKind::ChangeAbout
            }
            "ChangeUsername" => {
                actions::username_parts(&mut ev, place, sstr("prev_value"), sstr("new_value"));
                AdminActionKind::ChangeUsername
            }
            "ChangeUsernames" => {
                actions::usernames_parts(&mut ev, action, place);
                AdminActionKind::ChangeUsernames
            }
            "ChangePhoto" => actions::change_photo(&mut ev, event, action, place),
            "ToggleGroupCallSetting" => {
                let m = action["join_muted"].as_bool().unwrap_or(false);
                let base = if m {
                    "disallowed_unmute_self"
                } else {
                    "allowed_unmute_self"
                };
                ev.say0(&format!("{base}{call_sfx}"));
                ev.chg("join_muted", (!m).to_string(), m.to_string());
                AdminActionKind::ToggleGroupCallSetting
            }
            "UpdatePinned" | "DeleteMessage" | "StopPoll" | "SendMessage" => {
                actions::logged_msg(&mut ev, short, &action["message"])
            }
            "EditMessage" => actions::edit_msg(&mut ev, action),
            "ParticipantJoin" | "ParticipantLeave" | "StartGroupCall" | "DiscardGroupCall"
            | "ChangeWallpaper" => {
                let (k, ke) = match short {
                    "ParticipantJoin" => (
                        format!("participant_joined{grp_sfx}"),
                        AdminActionKind::ParticipantJoin,
                    ),
                    "ParticipantLeave" => (
                        format!("participant_left{grp_sfx}"),
                        AdminActionKind::ParticipantLeave,
                    ),
                    "StartGroupCall" => (
                        format!("started_group_call{call_sfx}"),
                        AdminActionKind::StartGroupCall,
                    ),
                    "DiscardGroupCall" => (
                        format!("discarded_group_call{call_sfx}"),
                        AdminActionKind::DiscardGroupCall,
                    ),
                    _ => ("change_wallpaper".into(), AdminActionKind::ChangeWallpaper),
                };
                ev.say0(&k);
                ke
            }
            "ParticipantInvite" | "ParticipantToggleBan" | "ParticipantToggleAdmin" => {
                actions::participant_diff(&mut ev, short, action)
            }
            "ChangeStickerSet" | "ChangeEmojiStickerSet" => {
                let (em, ns) = (short == "ChangeEmojiStickerSet", &action["new_stickerset"]);
                let pfx = if em { "emoji" } else { "stickers" };
                if ns.is_null() || tl_kind(ns).eq_ignore_ascii_case("inputStickerSetEmpty") {
                    ev.say0(&format!("removed_{pfx}_group"));
                } else {
                    let s = tr(&format!("changed_{pfx}_set"), &[]);
                    ev.say(&format!("changed_{pfx}_group"), &[("sticker_set", &s)]);
                }
                if em {
                    AdminActionKind::ChangeEmojiStickerSet
                } else {
                    AdminActionKind::ChangeStickerSet
                }
            }
            "DefaultBannedRights" => {
                let (txt, diffs) = default_rights_change(
                    &action["new_banned_rights"],
                    &action["prev_banned_rights"],
                );
                ev.changes = diffs;
                ev.parts
                    .push(AdminLogPart::note(vec![AdminTextPart::italic(txt)]));
                AdminActionKind::DefaultBannedRights
            }
            "ChangeLinkedChat" => actions::change_linked_chat(&mut ev, action),
            "ChangeLocation" => actions::change_location(&mut ev, action),
            "ToggleSlowMode" => {
                let (prev, secs) = (i64_or0(action, "prev_value"), i64_or0(action, "new_value"));
                if secs == 0 {
                    ev.say0("removed_slow_mode");
                } else {
                    let dur = if secs >= 60 {
                        plural("minutes", secs / 60)
                    } else {
                        plural("seconds", secs)
                    };
                    ev.say("changed_slow_mode", &[("duration", &dur)]);
                }
                ev.chg("slow_mode_seconds", prev.to_string(), secs.to_string());
                AdminActionKind::ToggleSlowMode
            }
            "ParticipantMute" | "ParticipantUnmute" | "ParticipantVolume" => {
                actions::call_participant(&mut ev, short, action, call_sfx)
            }
            "ParticipantJoinByInvite" | "ExportedInviteDelete" | "ExportedInviteRevoke" => {
                actions::invite_single(&mut ev, short, action, grp_sfx)
            }
            "ExportedInviteEdit" => {
                ev.tgt("invite_link", None, invite_link_text(&action["new_invite"]));
                let (txt, diffs) =
                    invite_link_change(chat, &action["new_invite"], &action["prev_invite"]);
                ev.changes = diffs;
                ev.parts
                    .push(AdminLogPart::note(vec![AdminTextPart::italic(txt)]));
                AdminActionKind::ExportedInviteEdit
            }
            "ChangeHistoryTTL" => {
                let (was, now) = (i64_or0(action, "prev_value"), i64_or0(action, "new_value"));
                let (pw, nw) = (format_history_ttl(was), format_history_ttl(now));
                match (was == 0, now == 0) {
                    (true, _) => ev.say("messages_ttl_set", &[("duration", &nw)]),
                    (false, true) => ev.say("messages_ttl_removed", &[("duration", &pw)]),
                    (false, false) => ev.say(
                        "messages_ttl_changed",
                        &[("previous", &pw), ("duration", &nw)],
                    ),
                }
                ev.chg("history_ttl_seconds", was.to_string(), now.to_string());
                AdminActionKind::ChangeHistoryTtl
            }
            "ParticipantJoinByRequest" => actions::join_by_request(&mut ev, action, grp_sfx),
            "ChangeAvailableReactions" => actions::available_reactions(&mut ev, action),
            "CreateTopic" | "DeleteTopic" | "PinTopic" => {
                actions::topic_single(&mut ev, short, action)
            }
            "EditTopic" => actions::edit_topic(&mut ev, action),
            "ChangePeerColor" | "ChangeProfilePeerColor" => {
                let prof = short == "ChangeProfilePeerColor";
                let sfx = if prof && chat.megagroup { "_group" } else { "" };
                actions::color_parts(&mut ev, action, if prof { "_profile" } else { "" }, sfx);
                if prof {
                    AdminActionKind::ChangeProfilePeerColor
                } else {
                    AdminActionKind::ChangePeerColor
                }
            }
            "ChangeEmojiStatus" => actions::emoji_status(&mut ev, action),
            "ParticipantSubExtend" => actions::sub_extend(&mut ev, action),
            "ParticipantEditRank" => actions::edit_rank(&mut ev, action),
            _ => {
                let safe = sanitize_action_type(raw_kind);
                ev.parts
                    .push(AdminLogPart::line(unknown_action_sentence(&ev.who, &safe)));
                AdminActionKind::Unknown(safe)
            }
        }
    };

    let summary = actions::summarize_parts(&ev.parts)
        .or(ev.fallback)
        .unwrap_or_else(|| format!("{}: {}", ev.who, kind.as_str()));
    let entry = AdminLogEntry {
        event_id,
        timestamp,
        actor,
        target: ev.target,
        action_kind: kind,
        summary,
        field_changes: ev.changes,
        before: ev.before,
        after: ev.after,
    };
    (entry, ev.parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn chat() -> LogChat {
        let mut c = LogChat::new(777, true, false);
        c.reference_date = NaiveDate::from_ymd_opt(2026, 10, 1);
        c.users.insert(42, json!({"id": 42, "first_name": "Sara", "last_name": "Karimi", "username": "sara_admin"}));
        c.users.insert(7, json!({"id": 7, "first_name": "Bob"}));
        c.chats
            .insert(900, json!({"id": 900, "title": "Talk", "megagroup": true}));
        c
    }

    #[test]
    fn maps_edits_missing_snapshots_and_unknown_actions_safely() {
        let c = chat();
        let pm = json!({"_": "message", "id": 9, "message": "old", "media": {"_": "messageMediaPhoto", "photo": {"_": "photo", "id": 3}}});
        let nm = json!({"_": "message", "id": 9, "message": "new", "media": {"_": "messageMediaPhoto", "photo": {"_": "photo", "id": 4}}});
        let (e2, p2) = map_event_parts(
            &json!({"id": "102", "date": 1_790_856_060, "user_id": 42, "action": {"_": "channelAdminLogEventActionEditMessage", "prev_message": pm, "new_message": nm}}),
            &c,
        );
        assert_eq!(
            (
                e2.summary.as_str(),
                p2.len(),
                e2.before.is_some(),
                e2.after.is_some()
            ),
            ("Sara Karimi edited media and caption:", 3, true, true)
        );

        let (e3, p3) = map_event_parts(
            &json!({"id": 103, "date": 1_790_856_120, "user_id": 42, "action": {"_": "ChannelAdminLogEventActionEditMessage", "prev_message": null, "new_message": {"_": "messageEmpty", "id": 9}}}),
            &c,
        );
        assert_eq!(
            (
                e3.before.is_none(),
                e3.after.is_none(),
                p3.len(),
                e3.summary.as_str()
            ),
            (true, true, 1, "Sara Karimi edited message:")
        );

        let unk = map_event(
            &json!({"id": 104, "date": 1, "user_id": 42, "action": {"_": "channelAdminLogEventActionFuture<script>"}}),
            &c,
        );
        assert_eq!(
            unk.action_kind,
            AdminActionKind::Unknown("channelAdminLogEventActionFuturescript".into())
        );
        assert!(!unk.summary.contains('<'));
    }

    #[test]
    fn maps_diverse_desktop_event_families_and_preserves_order() {
        let c = chat();
        let events = vec![
            json!({"id": 20, "date": 200, "user_id": 42, "action": {"_": "channelAdminLogEventActionChangeHistoryTTL", "prev_value": 5, "new_value": 86400}}),
            json!({"id": 10, "date": 100, "user_id": 42, "action": {"_": "channelAdminLogEventActionParticipantEditRank", "user_id": 7, "prev_rank": "", "new_rank": "Mod"}}),
            json!({"id": 30, "date": 300, "user_id": 42, "action": {"_": "channelAdminLogEventActionEditTopic", "prev_topic": {"_": "forumTopic", "id": 5, "title": "Old", "closed": false}, "new_topic": {"_": "forumTopic", "id": 5, "title": "New", "closed": true}}}),
            json!({"id": 40, "date": 400, "user_id": 42, "action": {"_": "channelAdminLogEventActionChangePeerColor", "prev_value": null, "new_value": {"_": "peerColor", "color": 2, "background_emoji_id": 99}}}),
            json!({"id": 50, "date": 500, "user_id": 42, "action": {"_": "channelAdminLogEventActionToggleAntiSpam", "new_value": true}}),
        ];
        let same = map_events(&events, &c);
        assert_eq!(
            same.iter().map(|e| e.event_id.as_str()).collect::<Vec<_>>(),
            vec!["20", "10", "30", "40", "50"]
        );
        assert!(
            same[2].summary.contains("renamed topic Old to New")
                && same[2].summary.contains("closed topic New")
        );
        assert!(
            same[3].summary.contains("from #1 to #3")
                && same[3].summary.contains("background emoji to @")
                && same[4].summary == "Sara Karimi enabled aggressive anti-spam"
        );
        let chrono = map_events_chronological(&events[..2], &c);
        assert_eq!(
            (chrono[0].event_id.as_str(), chrono[1].event_id.as_str()),
            ("10", "20")
        );
    }
}
