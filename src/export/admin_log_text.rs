//! Telegram Desktop 7.2.10 human-readable Recent Actions sentence and diff builders.

pub mod lang;
pub mod rights;

pub use lang::{
    ADMIN_RIGHTS, RESTRICTIONS, format_date_time, format_date_time_at, format_history_ttl,
    format_ttl, peer_color_index, plural, tr, unknown_action_sentence,
};
pub use rights::{admin_rights_diff, default_rights_change, participant_change};

use super::admin_log::{FieldChange, MessageSnapshot};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CUSTOM_EMOJI: &str = "@";
pub const REACTION_PLACEHOLDER: &str = "\u{1f4ad}";
pub const PUBLIC_JOIN_LINK: &str = "(public_join_link)";
pub const MINUS_SIGN: char = '\u{2212}';
pub const RESTRICT_FOREVER: i64 = i32::MAX as i64;
pub type DiffResult = (String, Vec<FieldChange>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminTextKind {
    Plain,
    Bold,
    Italic,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminTextPart {
    pub kind: AdminTextKind,
    pub text: String,
}

impl AdminTextPart {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            kind: AdminTextKind::Plain,
            text: text.into(),
        }
    }

    pub fn italic(text: impl Into<String>) -> Self {
        Self {
            kind: AdminTextKind::Italic,
            text: text.into(),
        }
    }

    pub fn url(text: impl Into<String>) -> Self {
        Self {
            kind: AdminTextKind::Url,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdminLogPart {
    Line {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        photo_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        photo_file: Option<String>,
    },
    Note {
        parts: Vec<AdminTextPart>,
    },
    Logged {
        snapshot: MessageSnapshot,
    },
    Original {
        title: String,
        parts: Vec<AdminTextPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot: Option<MessageSnapshot>,
    },
}

impl AdminLogPart {
    pub fn line(text: impl Into<String>) -> Self {
        Self::Line {
            text: text.into(),
            photo_id: None,
            photo_file: None,
        }
    }

    pub fn note(parts: Vec<AdminTextPart>) -> Self {
        Self::Note { parts }
    }

    pub fn orig(title: impl Into<String>, parts: Vec<AdminTextPart>) -> Self {
        Self::Original {
            title: title.into(),
            parts,
            snapshot: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditedTextDiff {
    pub header_sentence: String,
    pub original_title: &'static str,
    pub original_parts: Vec<AdminTextPart>,
    pub include_previous_media: bool,
    pub field_changes: Vec<FieldChange>,
}

#[derive(Debug, Clone, Default)]
pub struct LogChat {
    pub channel_id: i64,
    pub megagroup: bool,
    pub broadcast: bool,
    pub anyone_can_add: bool,
    pub self_id: i64,
    pub link_prefix: String,
    pub reference_date: Option<NaiveDate>,
    pub utc_offset_seconds: i32,
    pub users: BTreeMap<i64, Value>,
    pub chats: BTreeMap<i64, Value>,
}

impl LogChat {
    pub fn new(channel_id: i64, megagroup: bool, broadcast: bool) -> Self {
        Self {
            channel_id,
            megagroup,
            broadcast,
            anyone_can_add: true,
            link_prefix: "https://t.me/".into(),
            ..Self::default()
        }
    }

    pub fn from_peers(channel_id: i64, peers: &BTreeMap<String, Value>, self_id: i64) -> Self {
        let (mut users, mut chats) = (BTreeMap::new(), BTreeMap::new());
        for (k, v) in peers {
            let cid = k
                .strip_prefix("channel:")
                .or_else(|| k.strip_prefix("chat:"));
            if let Some(Ok(id)) = k.strip_prefix("user:").map(str::parse::<i64>) {
                users.insert(id, v.clone());
            } else if let Some(Ok(id)) = cid.map(str::parse::<i64>) {
                chats.insert(id, v.clone());
            }
        }
        let ent = chats.get(&channel_id);
        let mg = ent.and_then(|c| c["megagroup"].as_bool()).unwrap_or(false);
        let bc = ent.and_then(|c| c["broadcast"].as_bool()).unwrap_or(!mg);
        let mut chat = Self::new(channel_id, mg, bc);
        let inv_ban = ent.and_then(|c| c["default_banned_rights"]["invite_users"].as_bool());
        chat.anyone_can_add = !inv_ban.unwrap_or(false);
        chat.self_id = self_id;
        chat.users = users;
        chat.chats = chats;
        chat
    }

    pub fn user_name(&self, user_id: i64) -> String {
        let Some(u) = self
            .users
            .get(&user_id)
            .filter(|u| !u["deleted"].as_bool().unwrap_or(false))
        else {
            return "Deleted Account".into();
        };
        let f = u["first_name"].as_str().unwrap_or("").trim();
        let l = u["last_name"].as_str().unwrap_or("").trim();
        match (f.is_empty(), l.is_empty()) {
            (true, true) => "Deleted Account".into(),
            (false, true) => f.to_owned(),
            (true, false) => l.to_owned(),
            (false, false) => format!("{f} {l}"),
        }
    }

    pub fn peer_name(&self, peer: &Value) -> String {
        if let Some(uid) = json_i64(peer).or_else(|| peer.get("user_id").and_then(json_i64)) {
            return self.user_name(uid);
        }
        let cid = peer
            .get("channel_id")
            .or_else(|| peer.get("chat_id"))
            .and_then(json_i64)
            .unwrap_or(0);
        self.chats
            .get(&cid)
            .and_then(|c| c["title"].as_str())
            .unwrap_or("")
            .to_owned()
    }

    pub fn peer_username(&self, peer: &Value) -> Option<String> {
        let ent = match json_i64(peer).or_else(|| peer.get("user_id").and_then(json_i64)) {
            Some(uid) => self.users.get(&uid)?,
            None => {
                let cid = peer
                    .get("channel_id")
                    .or_else(|| peer.get("chat_id"))
                    .and_then(json_i64)?;
                self.chats.get(&cid)?
            }
        };
        if let Some(u) = ent["username"].as_str().filter(|s| !s.is_empty()) {
            return Some(u.to_owned());
        }
        ent["usernames"]
            .as_array()?
            .iter()
            .find(|i| i["active"].as_bool().unwrap_or(false))?["username"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    pub fn participant_string(&self, peer: &Value) -> String {
        let name = self.peer_name(peer);
        match self.peer_username(peer) {
            Some(u) => tr(
                "user_with_username",
                &[("name", &name), ("mention", &format!("@{u}"))],
            ),
            None => name,
        }
    }

    pub fn date_time(&self, when: i64, full: bool) -> String {
        let today = self
            .reference_date
            .unwrap_or_else(|| Utc::now().date_naive());
        format_date_time_at(when, full, today, self.utc_offset_seconds)
    }
}

pub(crate) fn json_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
}

pub(crate) fn tl_kind(v: &Value) -> &str {
    v.get("_").and_then(Value::as_str).unwrap_or("")
}

pub(super) fn fchange(
    f: impl Into<String>,
    b: impl Into<String>,
    a: impl Into<String>,
) -> FieldChange {
    FieldChange {
        field: f.into(),
        before: Some(b.into()),
        after: Some(a.into()),
    }
}

pub fn topic_link(topic: &Value) -> String {
    if !tl_kind(topic).eq_ignore_ascii_case("forumTopic") {
        return "Deleted".into();
    }
    let title = topic["title"].as_str().unwrap_or("");
    if json_i64(&topic["id"]).unwrap_or(0) == 1 {
        format!("# {title}")
    } else if json_i64(&topic["icon_emoji_id"]).unwrap_or(0) != 0 {
        format!("{CUSTOM_EMOJI} {title}")
    } else {
        title.to_owned()
    }
}

pub fn invite_link(invite: &Value) -> String {
    if tl_kind(invite).eq_ignore_ascii_case("chatInviteExported") {
        invite["link"]
            .as_str()
            .unwrap_or(PUBLIC_JOIN_LINK)
            .to_owned()
    } else {
        PUBLIC_JOIN_LINK.into()
    }
}

pub fn invite_link_label(invite: &Value) -> String {
    if tl_kind(invite).eq_ignore_ascii_case("chatInviteExported") {
        invite["title"].as_str().unwrap_or("").to_owned()
    } else {
        PUBLIC_JOIN_LINK.into()
    }
}

pub fn invite_link_text(invite: &Value) -> String {
    let lbl = invite_link_label(invite);
    if !lbl.is_empty() {
        lbl
    } else {
        invite_link(invite)
            .replace("https://", "")
            .replace("t.me/joinchat/", "")
    }
}

pub fn invite_link_change(chat: &LogChat, new_inv: &Value, prev_inv: &Value) -> DiffResult {
    let i_of = |v: &Value, k: &str| v.get(k).and_then(json_i64).unwrap_or(0);
    let app_of = |v: &Value| {
        !tl_kind(v).eq_ignore_ascii_case("chatInviteExported")
            || v["request_needed"].as_bool().unwrap_or(false)
    };
    let no_lim = tr("group_invite_no_limit", &[]);
    let fmt_e = |t: i64| {
        if t != 0 {
            chat.date_time(t, false)
        } else {
            no_lim.clone()
        }
    };
    let fmt_u = |c: i64| {
        if c != 0 {
            c.to_string()
        } else {
            no_lim.clone()
        }
    };
    let hdr = tr(
        "edited_invite_link",
        &[("link", &invite_link_text(new_inv))],
    );
    let (mut out, mut diffs) = (format!("{hdr}\n"), Vec::new());
    let rows = [
        (
            "title",
            "invite_link_label",
            invite_link_label(prev_inv),
            invite_link_label(new_inv),
        ),
        (
            "expire_date",
            "invite_link_expire_date",
            fmt_e(i_of(prev_inv, "expire_date")),
            fmt_e(i_of(new_inv, "expire_date")),
        ),
        (
            "usage_limit",
            "invite_link_usage_limit",
            fmt_u(i_of(prev_inv, "usage_limit")),
            fmt_u(i_of(new_inv, "usage_limit")),
        ),
    ];
    for (field, tpl, pv, nv) in rows {
        if pv != nv {
            out.push_str(&format!(
                "\n{}",
                tr(tpl, &[("previous", &pv), ("limit", &nv)])
            ));
            diffs.push(fchange(field, pv, nv));
        }
    }
    let (pa, na) = (app_of(prev_inv), app_of(new_inv));
    if pa != na {
        let tpl = if na {
            "invite_link_request_needed"
        } else {
            "invite_link_request_not_needed"
        };
        out.push_str(&format!("\n{}", tr(tpl, &[])));
        diffs.push(fchange("request_needed", pa.to_string(), na.to_string()));
    }
    (out, diffs)
}

pub fn edited_text_diff(
    who: &str,
    before: Option<&MessageSnapshot>,
    after: Option<&MessageSnapshot>,
) -> EditedTextDiff {
    let captioned = after.is_some_and(|m| m.has_caption_media);
    let (old_t, new_t) = (
        before.map_or("", |m| m.text.as_str()),
        after.map_or("", |m| m.text.as_str()),
    );
    let changed_cap = old_t != new_t || before.map(|m| &m.entities) != after.map(|m| &m.entities);
    let old_m = before.and_then(|m| m.media_id.as_deref()).unwrap_or("");
    let new_m = after.and_then(|m| m.media_id.as_deref()).unwrap_or("");
    let changed_media = before.is_some() && after.is_some() && old_m != new_m;
    let key = match (
        captioned,
        changed_media,
        !old_t.is_empty() && new_t.is_empty(),
        changed_cap,
    ) {
        (false, _, _, _) => "edited_message",
        (true, true, true, _) => "edited_media_and_removed_caption",
        (true, true, false, true) => "edited_media_and_caption",
        (true, true, false, false) => "edited_media",
        (true, false, true, _) => "removed_caption",
        (true, false, false, true) => "edited_caption",
        _ => "edited_message",
    };
    let original_parts = match before {
        Some(p) if !p.text.is_empty() => vec![AdminTextPart::plain(&p.text)],
        Some(_) => vec![AdminTextPart::italic(tr("empty_text", &[]))],
        None => Vec::new(),
    };
    let prev_has_media = before
        .is_some_and(|m| m.has_caption_media && m.media_id.as_deref().is_some_and(|id| id != "0"));
    let mut field_changes = Vec::new();
    if before.is_some() && after.is_some() && old_t != new_t {
        field_changes.push(fchange(
            if captioned { "caption" } else { "text" },
            old_t,
            new_t,
        ));
    }
    if changed_media {
        field_changes.push(fchange("media_id", old_m, new_m));
    }
    EditedTextDiff {
        header_sentence: tr(key, &[("from", who)]),
        original_title: if captioned {
            "Original caption"
        } else {
            "Original message"
        },
        original_parts,
        include_previous_media: changed_media && prev_has_media,
        field_changes,
    }
}

pub fn link_lines(chat: &LogChat, names: &[String]) -> Vec<AdminTextPart> {
    let mut parts = Vec::with_capacity(names.len() * 2);
    for n in names {
        let full = format!("{}{n}", chat.link_prefix);
        if !full.is_empty() {
            parts.push(AdminTextPart::url(full));
        }
        parts.push(AdminTextPart::plain("\n"));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_topics_rights_invites_and_edits() {
        assert_eq!(topic_link(&json!({"_": "forumTopicDeleted"})), "Deleted");
        assert_eq!(
            topic_link(&json!({"_": "forumTopic", "id": 1, "title": "General"})),
            "# General"
        );
        assert_eq!(
            topic_link(&json!({"_": "forumTopic", "id": 5, "title": "News", "icon_emoji_id": 9})),
            "@ News"
        );

        let mut chat = LogChat::new(777, true, false);
        chat.reference_date = NaiveDate::from_ymd_opt(2026, 10, 1);
        chat.users.insert(7, json!({"id": 7, "first_name": "Bob"}));
        let new_adm = json!({"_": "channelParticipantAdmin", "user_id": 7, "admin_rights": {"change_info": true, "invite_users": true}});
        let (adm, diffs) = participant_change(
            &chat,
            &new_adm,
            Some(&json!({"_": "channelParticipant", "user_id": 7})),
        );
        assert!(
            adm.contains("+Change info")
                && adm.contains("+Invite users via link")
                && diffs.len() == 2
        );
        assert!(
            default_rights_change(&json!({"send_photos": true}), &json!({}))
                .0
                .contains("\u{2212}Send photos")
        );

        let ni = json!({"_": "chatInviteExported", "link": "https://t.me/+abc", "title": "VIP", "usage_limit": 10, "request_needed": true});
        let pi = json!({"_": "chatInviteExported", "link": "https://t.me/+abc", "title": "", "usage_limit": 0, "request_needed": false});
        let (inv, idiffs) = invite_link_change(&chat, &ni, &pi);
        assert!(
            inv.contains("Name:  \u{2192} VIP")
                && inv.contains("Usage limit: No limit \u{2192} 10")
                && idiffs.len() == 3
        );
    }
}
