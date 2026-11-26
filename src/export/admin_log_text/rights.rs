//! Telegram Desktop participant privilege, restriction, and default-rights diff builders.

use super::{
    ADMIN_RIGHTS, DiffResult, LogChat, MINUS_SIGN, RESTRICT_FOREVER, RESTRICTIONS, fchange,
    json_i64, tl_kind, tr,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

type FlagSet = BTreeSet<&'static str>;

fn collect_flags(rights: &Value, table: &[(&[&'static str], Option<&str>)]) -> FlagSet {
    table
        .iter()
        .flat_map(|&(fields, _)| fields.iter().copied())
        .filter(|&n| rights.get(n).and_then(Value::as_bool).unwrap_or(false))
        .collect()
}

fn collect_changes(table: &[(&[&str], String)], plus: &FlagSet, minus: &FlagSet) -> DiffResult {
    let (mut out, mut diffs) = (String::new(), Vec::new());
    for (added, pfx) in [(true, '+'), (false, MINUS_SIGN)] {
        let (want, skip) = if added { (plus, minus) } else { (minus, plus) };
        for (fields, phrase) in table {
            if fields.iter().any(|f| want.contains(f) && !skip.contains(f)) {
                out.push_str(&format!("\n{pfx}{phrase}"));
                diffs.push(fchange(
                    phrase.clone(),
                    (!added).to_string(),
                    added.to_string(),
                ));
            }
        }
    }
    (out, diffs)
}

pub fn admin_rights_diff(chat: &LogChat, user: &str, new_r: &Value, prev_r: &Value) -> DiffResult {
    admin_change_flags(
        chat,
        user,
        &collect_flags(new_r, &ADMIN_RIGHTS),
        &collect_flags(prev_r, &ADMIN_RIGHTS),
    )
}

fn admin_change_flags(chat: &LogChat, user: &str, new_f: &FlagSet, prev_f: &FlagSet) -> DiffResult {
    let inv = if chat.megagroup && chat.anyone_can_add {
        "Invite users via link"
    } else {
        "Add users"
    };
    let call = if chat.broadcast {
        "Manage live streams"
    } else {
        "Manage video chats"
    };
    let table: Vec<(&[&str], String)> = ADMIN_RIGHTS
        .iter()
        .map(|&(f, p)| {
            (
                f,
                p.unwrap_or(if f == ["invite_users"] { inv } else { call })
                    .to_owned(),
            )
        })
        .collect();
    let (ch, diffs) = collect_changes(&table, new_f, prev_f);
    let base = tr("promoted", &[("user", user)]);
    (
        if ch.is_empty() {
            base
        } else {
            format!("{base}\n{ch}")
        },
        diffs,
    )
}

fn restriction_flags(new_f: &FlagSet, prev_f: &FlagSet, user_specific: bool) -> DiffResult {
    let rank = if user_specific {
        "Edit own tag"
    } else {
        "Edit own tags"
    };
    let table: Vec<(&[&str], String)> = RESTRICTIONS
        .iter()
        .map(|&(f, p)| (f, p.unwrap_or(rank).to_owned()))
        .collect();
    collect_changes(&table, prev_f, new_f)
}

pub fn default_rights_change(new_r: &Value, prev_r: &Value) -> DiffResult {
    let (nf, pf) = (
        collect_flags(new_r, &RESTRICTIONS),
        collect_flags(prev_r, &RESTRICTIONS),
    );
    let (ch, diffs) = restriction_flags(&nf, &pf, false);
    let base = tr("changed_default_permissions", &[]);
    (
        if ch.is_empty() {
            base
        } else {
            format!("{base}\n{ch}")
        },
        diffs,
    )
}

fn parse_participant(data: &Value) -> (&'static str, Value, FlagSet, FlagSet, i64) {
    let kr = tl_kind(data);
    let is_ban = kr.eq_ignore_ascii_case("channelParticipantBanned");
    let is_left = kr.eq_ignore_ascii_case("channelParticipantLeft");
    let peer = if is_ban || is_left {
        data.get("peer").cloned().unwrap_or(Value::Null)
    } else {
        json!({"_": "peerUser", "user_id": data.get("user_id").and_then(json_i64).unwrap_or(0)})
    };
    let is_cr = kr.eq_ignore_ascii_case("channelParticipantCreator");
    if is_cr || kr.eq_ignore_ascii_case("channelParticipantAdmin") {
        let r = collect_flags(&data["admin_rights"], &ADMIN_RIGHTS);
        return (
            if is_cr { "creator" } else { "admin" },
            peer,
            r,
            BTreeSet::new(),
            0,
        );
    }
    if is_ban {
        let fl = collect_flags(&data["banned_rights"], &RESTRICTIONS);
        let k = if fl.contains("view_messages") {
            "banned"
        } else {
            "restricted"
        };
        let until = data["banned_rights"]
            .get("until_date")
            .and_then(json_i64)
            .unwrap_or(0);
        return (k, peer, BTreeSet::new(), fl, until);
    }
    (
        if is_left { "left" } else { "member" },
        peer,
        BTreeSet::new(),
        BTreeSet::new(),
        0,
    )
}

pub fn participant_change(
    chat: &LogChat,
    new_data: &Value,
    old_data: Option<&Value>,
) -> DiffResult {
    let (nk, npeer, nr, nban, nuntil) = parse_participant(new_data);
    let old = old_data.filter(|v| !v.is_null()).map(parse_participant);
    let empty = BTreeSet::new();
    let (ok, or_f, oban) = match old.as_ref() {
        Some((k, _, r, b, _)) => (*k, r, b),
        None => ("member", &empty, &empty),
    };
    let user = chat.participant_string(&npeer);
    let self_cr =
        npeer.get("user_id").and_then(json_i64) == Some(chat.self_id) && chat.self_id != 0;
    match nk {
        "creator" if self_cr => admin_change_flags(chat, &user, &nr, or_f),
        "creator" => (
            tr("transferred", &[("user", &user)]),
            vec![fchange("role", ok, "creator")],
        ),
        "admin" => admin_change_flags(chat, &user, &nr, or_f),
        "restricted" | "banned" => {
            permissions_change_text(chat, &npeer, &user, &nban, nuntil, oban)
        }
        _ => match old.as_ref().map(|t| t.0) {
            Some("admin") => admin_change_flags(chat, &user, &empty, or_f),
            Some("banned" | "restricted") => {
                permissions_change_text(chat, &npeer, &user, &empty, 0, oban)
            }
            _ => (tr("invited", &[("user", &user)]), Vec::new()),
        },
    }
}

fn permissions_change_text(
    chat: &LogChat,
    peer: &Value,
    user: &str,
    new_f: &FlagSet,
    until: i64,
    prev_f: &FlagSet,
) -> DiffResult {
    let forever = until == 0 || until == RESTRICT_FOREVER;
    let u_str = if forever {
        tr("restricted_forever", &[])
    } else {
        tr(
            "restricted_until",
            &[("date", &chat.date_time(until, false))],
        )
    };
    if new_f.contains("view_messages") {
        let txt = if forever {
            tr("banned", &[("user", user)])
        } else {
            tr("banned_until", &[("user", user), ("until", &u_str)])
        };
        let was_able = (!prev_f.contains("view_messages")).to_string();
        return (txt, vec![fchange("Read messages", was_able, "false")]);
    }
    let is_user = tl_kind(peer).eq_ignore_ascii_case("peerUser") || peer.get("user_id").is_some();
    if new_f.is_empty() && prev_f.contains("view_messages") && !is_user {
        return (
            tr("unbanned", &[("user", user)]),
            vec![fchange("Read messages", "false", "true")],
        );
    }
    let base = tr("restricted", &[("user", user), ("until", &u_str)]);
    let (ch, diffs) = restriction_flags(new_f, prev_f, true);
    (
        if ch.is_empty() {
            base
        } else {
            format!("{base}\n{ch}")
        },
        diffs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_transfer_ban_unban_and_default_diffs() {
        let mut chat = LogChat::new(777, false, true);
        chat.users.insert(
            9,
            json!({"id": 9, "first_name": "Nina", "username": "nina"}),
        );
        let cr = json!({"_": "channelParticipantCreator", "user_id": 9, "admin_rights": {}});
        let (tr_txt, tr_d) = participant_change(&chat, &cr, None);
        assert_eq!(
            (tr_txt.as_str(), tr_d.len()),
            ("transferred ownership to Nina (@nina)", 1)
        );

        let ban = json!({"_": "channelParticipantBanned", "peer": {"_": "peerChannel", "channel_id": 5}, "banned_rights": {"view_messages": true}});
        let unban =
            json!({"_": "channelParticipantLeft", "peer": {"_": "peerChannel", "channel_id": 5}});
        chat.chats.insert(5, json!({"id": 5, "title": "SpamChan"}));
        assert_eq!(participant_change(&chat, &ban, None).0, "banned SpamChan");
        assert_eq!(
            participant_change(&chat, &unban, Some(&ban)).0,
            "unbanned SpamChan"
        );
    }
}
