//! Complex Desktop Recent Actions projections (messages, participants, topics, usernames, colors).

use super::{
    AdminActionKind, Ev, FieldChange, extract_message_snapshot, i64_or0, peer_dec, str_id,
};
use crate::export::admin_log_text::{
    AdminLogPart, AdminTextPart, CUSTOM_EMOJI, PUBLIC_JOIN_LINK, REACTION_PLACEHOLDER,
    edited_text_diff, invite_link_text, json_i64, link_lines, participant_change, peer_color_index,
    tl_kind, topic_link, tr,
};
use serde_json::{Value, json};

pub(super) fn change_photo(
    ev: &mut Ev<'_>,
    event: &Value,
    action: &Value,
    place: &str,
) -> AdminActionKind {
    let np = &action["new_photo"];
    let prev_id = str_id(&action["prev_photo"], "id").filter(|s| s != "0");
    if tl_kind(np).eq_ignore_ascii_case("photo") {
        let pid = str_id(np, "id");
        let pfile = event["export_file"]
            .as_str()
            .or_else(|| np["export_file"].as_str())
            .map(str::to_owned);
        let text = tr(&format!("changed_photo_{place}"), &[("from", &ev.who)]);
        ev.parts.push(AdminLogPart::Line {
            text,
            photo_id: pid.clone(),
            photo_file: pfile,
        });
        ev.changes.push(FieldChange {
            field: "photo".into(),
            before: prev_id,
            after: pid,
        });
    } else {
        ev.say0(&format!("removed_photo_{place}"));
        ev.changes.push(FieldChange {
            field: "photo".into(),
            before: prev_id,
            after: None,
        });
    }
    AdminActionKind::ChangePhoto
}

pub(super) fn change_linked_chat(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let cid = i64_or0(action, "new_value");
    if let Some(c) = (cid != 0).then(|| ev.chat.chats.get(&cid)).flatten() {
        let title = c["title"].as_str().unwrap_or("");
        ev.tgt("chat", Some(cid.to_string()), title);
        let k = if ev.chat.broadcast {
            "changed_linked_chat"
        } else {
            "changed_linked_channel"
        };
        ev.say(k, &[("chat", title)]);
    } else {
        let k = if ev.chat.broadcast {
            "removed_linked_chat"
        } else {
            "removed_linked_channel"
        };
        ev.say0(k);
    }
    AdminActionKind::ChangeLinkedChat
}

pub(super) fn change_location(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let loc = &action["new_value"];
    if tl_kind(loc).eq_ignore_ascii_case("channelLocation") {
        let addr = loc["address"].as_str().unwrap_or("");
        ev.say("changed_location_chat", &[("address", addr)]);
        let prev_a = action["prev_value"]["address"].as_str().map(str::to_owned);
        ev.changes.push(FieldChange {
            field: "address".into(),
            before: prev_a,
            after: Some(addr.to_owned()),
        });
    } else {
        ev.say0("removed_location_chat");
    }
    AdminActionKind::ChangeLocation
}

pub(super) fn call_participant(
    ev: &mut Ev<'_>,
    short: &str,
    action: &Value,
    call_sfx: &str,
) -> AdminActionKind {
    let gp = &action["participant"];
    let user = ev.chat.peer_name(&gp["peer"]);
    ev.tgt("peer", peer_dec(&gp["peer"]), &user);
    if short == "ParticipantVolume" {
        let pct = format!(
            "{}%",
            gp.get("volume").and_then(json_i64).unwrap_or(10_000) / 100
        );
        ev.say(
            &format!("participant_volume{call_sfx}"),
            &[("user", &user), ("percent", &pct)],
        );
        AdminActionKind::ParticipantVolume
    } else {
        let mute = short == "ParticipantMute";
        let base = if mute {
            "muted_participant"
        } else {
            "unmuted_participant"
        };
        ev.say(&format!("{base}{call_sfx}"), &[("user", &user)]);
        if mute {
            AdminActionKind::ParticipantMute
        } else {
            AdminActionKind::ParticipantUnmute
        }
    }
}

pub(super) fn logged_msg(ev: &mut Ev<'_>, short: &str, msg: &Value) -> AdminActionKind {
    ev.after = extract_message_snapshot(msg);
    let key = match short {
        "UpdatePinned" if ev.after.as_ref().is_some_and(|s| s.pinned) => "pinned_message",
        "UpdatePinned" => "unpinned_message",
        "DeleteMessage" => "deleted_message",
        "StopPoll" => "stopped_poll",
        _ => "sent_message",
    };
    ev.say0(key);
    if let Some(snapshot) = ev.after.clone() {
        let mid = snapshot.message_id.clone();
        ev.tgt("message", mid.clone(), mid.unwrap_or_default());
        ev.parts.push(AdminLogPart::Logged { snapshot });
    }
    match short {
        "UpdatePinned" => AdminActionKind::UpdatePinned,
        "DeleteMessage" => AdminActionKind::DeleteMessage,
        "StopPoll" => AdminActionKind::StopPoll,
        _ => AdminActionKind::SendMessage,
    }
}

pub(super) fn edit_msg(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    ev.before = extract_message_snapshot(&action["prev_message"]);
    ev.after = extract_message_snapshot(&action["new_message"]);
    let ed = edited_text_diff(&ev.who, ev.before.as_ref(), ev.after.as_ref());
    ev.changes = ed.field_changes;
    let mid = ev
        .after
        .as_ref()
        .or(ev.before.as_ref())
        .map(|s| s.message_id.clone());
    if let Some(mid) = mid {
        ev.tgt("message", mid.clone(), mid.unwrap_or_default());
    }
    ev.parts.push(AdminLogPart::line(ed.header_sentence));
    if let Some(ref ns) = ev.after {
        ev.parts.push(AdminLogPart::Logged {
            snapshot: ns.clone(),
        });
    }
    if ev.before.is_some() {
        let snapshot = ed
            .include_previous_media
            .then(|| ev.before.clone())
            .flatten();
        ev.parts.push(AdminLogPart::Original {
            title: ed.original_title.into(),
            parts: ed.original_parts,
            snapshot,
        });
    }
    AdminActionKind::EditMessage
}

pub(super) fn participant_diff(ev: &mut Ev<'_>, short: &str, action: &Value) -> AdminActionKind {
    let (np, pp) = if short == "ParticipantInvite" {
        (&action["participant"], None)
    } else {
        (
            &action["new_participant"],
            Some(&action["prev_participant"]),
        )
    };
    let peer = np.get("peer").cloned().or_else(|| {
        np.get("user_id")
            .and_then(json_i64)
            .map(|id| json!({"_": "peerUser", "user_id": id}))
    });
    if let Some(p) = peer {
        ev.tgt("peer", peer_dec(&p), ev.chat.participant_string(&p));
    }
    let (txt, diffs) = participant_change(ev.chat, np, pp);
    ev.changes = diffs;
    let c2a = short == "ParticipantToggleAdmin"
        && tl_kind(np).eq_ignore_ascii_case("channelParticipantAdmin")
        && pp.is_some_and(|p| tl_kind(p).eq_ignore_ascii_case("channelParticipantCreator"));
    if c2a {
        ev.fallback = Some(txt);
    } else {
        ev.parts
            .push(AdminLogPart::note(vec![AdminTextPart::italic(txt)]));
    }
    match short {
        "ParticipantInvite" => AdminActionKind::ParticipantInvite,
        "ParticipantToggleAdmin" => AdminActionKind::ParticipantToggleAdmin,
        _ => AdminActionKind::ParticipantToggleBan,
    }
}

pub(super) fn topic_single(ev: &mut Ev<'_>, short: &str, action: &Value) -> AdminActionKind {
    if short == "PinTopic" {
        let pick = [
            ("new_topic", "topics_pinned"),
            ("prev_topic", "topics_unpinned"),
        ]
        .into_iter()
        .find(|(k, _)| action.get(*k).is_some_and(|v| !v.is_null()));
        if let Some((k, tpl)) = pick {
            let lbl = topic_link(&action[k]);
            ev.tgt("topic", str_id(&action[k], "id"), &lbl);
            ev.say(tpl, &[("topic", &lbl)]);
        } else {
            ev.fallback = Some(format!("{} updated pinned topics", ev.who));
        }
        return AdminActionKind::PinTopic;
    }
    let (tt, cr) = (&action["topic"], short == "CreateTopic");
    let lbl = topic_link(tt);
    ev.tgt("topic", str_id(tt, "id"), &lbl);
    ev.say(
        if cr {
            "topics_created"
        } else {
            "topics_deleted"
        },
        &[("topic", &lbl)],
    );
    if cr {
        AdminActionKind::CreateTopic
    } else {
        AdminActionKind::DeleteTopic
    }
}

pub(super) fn edit_topic(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let (pt, nt) = (&action["prev_topic"], &action["new_topic"]);
    let (pl, nl) = (topic_link(pt), topic_link(nt));
    ev.tgt("topic", str_id(nt, "id"), &nl);
    if !(pl == nl && str_id(pt, "id") == str_id(nt, "id")) {
        ev.say("topics_changed", &[("topic", &pl), ("new_topic", &nl)]);
        ev.chg("topic", pl, &nl);
    }
    for (attr, on, off) in [
        ("closed", "closed", "reopened"),
        ("hidden", "hidden", "unhidden"),
    ] {
        let (was, now) = (
            pt[attr].as_bool().unwrap_or(false),
            nt[attr].as_bool().unwrap_or(false),
        );
        if was != now {
            ev.say(
                &format!("topics_{}", if now { on } else { off }),
                &[("topic", &nl)],
            );
            ev.chg(attr, was.to_string(), now.to_string());
        }
    }
    AdminActionKind::EditTopic
}

pub(super) fn emoji_status(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let doc = |v: &Value| {
        let k = tl_kind(v);
        if k.eq_ignore_ascii_case("emojiStatus") || k.eq_ignore_ascii_case("emojiStatusCollectible")
        {
            i64_or0(v, "document_id")
        } else {
            0
        }
    };
    let (pd, nd) = (doc(&action["prev_value"]), doc(&action["new_value"]));
    let until = if tl_kind(&action["new_value"]).eq_ignore_ascii_case("emojiStatus") {
        i64_or0(&action["new_value"], "until")
    } else {
        0
    };
    let dt = if until != 0 {
        ev.chat.date_time(until, false)
    } else {
        String::new()
    };
    let k = match (pd == 0, nd == 0, until != 0) {
        (true, _, true) => "set_status_until",
        (true, _, false) => "set_status",
        (false, true, _) => "removed_status",
        (false, false, true) => "change_status_until",
        (false, false, false) => "change_status",
    };
    ev.say(
        k,
        &[
            ("emoji", CUSTOM_EMOJI),
            ("previous", CUSTOM_EMOJI),
            ("date", &dt),
        ],
    );
    ev.chg("emoji_status_document_id", pd.to_string(), nd.to_string());
    AdminActionKind::ChangeEmojiStatus
}

pub(super) fn sub_extend(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let np = &action["new_participant"];
    let (until, uid) = (
        i64_or0(np, "subscription_until_date"),
        i64_or0(np, "user_id"),
    );
    let name = ev.chat.user_name(uid);
    ev.tgt("user", Some(uid.to_string()), &name);
    if until != 0 {
        let line = tr(
            "subscription_extend",
            &[("name", &name), ("date", &ev.chat.date_time(until, true))],
        );
        ev.parts.push(AdminLogPart::line(line));
    } else {
        ev.fallback = Some(format!("{name} renewed subscription"));
    }
    AdminActionKind::ParticipantSubExtend
}

pub(super) fn edit_rank(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let uid = i64_or0(action, "user_id");
    let prev = action["prev_rank"].as_str().unwrap_or("");
    let nr = action["new_rank"].as_str().unwrap_or("");
    let user = ev.chat.user_name(uid);
    ev.tgt("user", Some(uid.to_string()), &user);
    let k = match (uid == ev.actor_uid, prev.is_empty(), nr.is_empty()) {
        (true, _, true) => "removed_own_rank",
        (true, true, false) => "set_own_rank",
        (true, false, false) => "changed_own_rank_from",
        (false, _, true) => "removed_rank",
        (false, true, false) => "set_rank",
        (false, false, false) => "changed_rank_from",
    };
    ev.say(k, &[("user", &user), ("previous", prev), ("tag", nr)]);
    ev.chg("rank", prev, nr);
    AdminActionKind::ParticipantEditRank
}

pub(super) fn username_parts(ev: &mut Ev<'_>, place: &str, prev: &str, nv: &str) {
    let verb = if nv.is_empty() { "removed" } else { "changed" };
    ev.say0(&format!("{verb}_link_{place}"));
    let note = if nv.is_empty() {
        Vec::new()
    } else {
        vec![AdminTextPart::url(format!("{}{nv}", ev.chat.link_prefix))]
    };
    ev.parts.push(AdminLogPart::note(note));
    if !prev.is_empty() {
        let old_u = vec![AdminTextPart::url(format!("{}{prev}", ev.chat.link_prefix))];
        ev.parts
            .push(AdminLogPart::orig(tr("previous_link", &[]), old_u));
    }
    ev.chg("username", prev, nv);
}

pub(super) fn usernames_parts(ev: &mut Ev<'_>, action: &Value, place: &str) {
    let sv = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (nl, ol) = (sv(&action["new_value"]), sv(&action["prev_value"]));
    if nl.len() == ol.len() {
        if nl.len() == 1 {
            return username_parts(ev, place, &ol[0], &nl[0]);
        }
        if nl.iter().all(|u| ol.contains(u)) {
            ev.say0(&format!("reordered_link_{place}"));
            ev.parts.push(AdminLogPart::note(link_lines(ev.chat, &nl)));
            let prev_t = tr("previous_links_order", &[]);
            ev.parts
                .push(AdminLogPart::orig(prev_t, link_lines(ev.chat, &ol)));
            return ev.chg("usernames", ol.join(","), nl.join(","));
        }
    } else if nl.len().abs_diff(ol.len()) == 1 {
        let act = nl.len() > ol.len();
        let (longer, shorter) = if act { (&nl, &ol) } else { (&ol, &nl) };
        let ch = longer
            .iter()
            .find(|u| !shorter.contains(*u))
            .map_or("", String::as_str);
        ev.say(
            if act {
                "activated_link"
            } else {
                "deactivated_link"
            },
            &[("link", ch)],
        );
        return ev.chg("usernames", ol.join(","), nl.join(","));
    }
    ev.parts.push(AdminLogPart::line(format!(
        "{} changed list of {place} links:",
        ev.who
    )));
    ev.parts.push(AdminLogPart::note(link_lines(ev.chat, &nl)));
    ev.parts.push(AdminLogPart::orig(
        "Previous links",
        link_lines(ev.chat, &ol),
    ));
    ev.chg("usernames", ol.join(","), nl.join(","));
}

pub(super) fn color_parts(ev: &mut Ev<'_>, action: &Value, p: &str, sfx: &str) {
    let col = |v: &Value| {
        tl_kind(v)
            .eq_ignore_ascii_case("peerColor")
            .then(|| v.get("color").and_then(json_i64))
            .flatten()
    };
    let emo = |v: &Value| {
        if tl_kind(v).eq_ignore_ascii_case("peerColor") {
            i64_or0(v, "background_emoji_id")
        } else {
            0
        }
    };
    let (cw, cn) = (col(&action["prev_value"]), col(&action["new_value"]));
    if cw != cn {
        let wrap = |c: Option<i64>| {
            format!(
                "#{}",
                c.unwrap_or_else(|| peer_color_index(ev.chat.channel_id)) + 1
            )
        };
        let (pw, nw) = (wrap(cw), wrap(cn));
        ev.say(
            &format!("change{p}_color{sfx}"),
            &[("previous", &pw), ("color", &nw)],
        );
        ev.chg("color", pw, nw);
    }
    let (ew, en) = (emo(&action["prev_value"]), emo(&action["new_value"]));
    if ew != en {
        match (ew == 0, en == 0) {
            (true, _) => ev.say(
                &format!("set{p}_background_emoji{sfx}"),
                &[("emoji", CUSTOM_EMOJI)],
            ),
            (false, true) => ev.say(
                &format!("removed{p}_background_emoji{sfx}"),
                &[("emoji", CUSTOM_EMOJI)],
            ),
            (false, false) => ev.say(
                &format!("change{p}_background_emoji{sfx}"),
                &[("previous", CUSTOM_EMOJI), ("emoji", CUSTOM_EMOJI)],
            ),
        }
        ev.chg("background_emoji_id", ew.to_string(), en.to_string());
    }
}

pub(super) fn invite_single(
    ev: &mut Ev<'_>,
    short: &str,
    action: &Value,
    grp_sfx: &str,
) -> AdminActionKind {
    let link = invite_link_text(&action["invite"]);
    ev.tgt("invite_link", None, &link);
    let flt = action["via_chatlist"].as_bool().unwrap_or(false);
    let jb = if flt {
        "participant_joined_by_filter_link"
    } else {
        "participant_joined_by_link"
    };
    let (k, ke) = match short {
        "ParticipantJoinByInvite" => (
            format!("{jb}{grp_sfx}"),
            AdminActionKind::ParticipantJoinByInvite,
        ),
        "ExportedInviteDelete" => (
            "delete_invite_link".into(),
            AdminActionKind::ExportedInviteDelete,
        ),
        _ => (
            "revoke_invite_link".into(),
            AdminActionKind::ExportedInviteRevoke,
        ),
    };
    ev.say(&k, &[("link", &link)]);
    ke
}

pub(super) fn join_by_request(ev: &mut Ev<'_>, action: &Value, grp_sfx: &str) -> AdminActionKind {
    let aid = i64_or0(action, "approved_by");
    let (user, link) = (ev.chat.user_name(aid), invite_link_text(&action["invite"]));
    if link == PUBLIC_JOIN_LINK {
        ev.say(
            &format!("participant_approved_by_request{grp_sfx}"),
            &[("user", &user)],
        );
    } else {
        ev.say(
            &format!("participant_approved_by_link{grp_sfx}"),
            &[("link", &link), ("user", &user)],
        );
    }
    ev.tgt("user", Some(aid.to_string()), user);
    AdminActionKind::ParticipantJoinByRequest
}

pub(super) fn available_reactions(ev: &mut Ev<'_>, action: &Value) -> AdminActionKind {
    let (val, vt) = (&action["new_value"], tl_kind(&action["new_value"]));
    if vt.eq_ignore_ascii_case("chatReactionsSome") {
        let emoji = val["reactions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| {
                        if tl_kind(r).eq_ignore_ascii_case("reactionEmoji") {
                            r["emoticon"].as_str().unwrap_or(REACTION_PLACEHOLDER)
                        } else {
                            REACTION_PLACEHOLDER
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        ev.say("reactions_updated", &[("emoji", &emoji)]);
    } else if vt.eq_ignore_ascii_case("chatReactionsAll") {
        let all = val["allow_custom"].as_bool().unwrap_or(false);
        ev.say0(if all {
            "reactions_allowed_all"
        } else {
            "reactions_allowed_official"
        });
    } else {
        ev.say0("reactions_disabled");
    }
    AdminActionKind::ChangeAvailableReactions
}

pub(super) fn summarize_parts(parts: &[AdminLogPart]) -> Option<String> {
    let lines: Vec<&str> = parts
        .iter()
        .filter_map(|p| {
            if let AdminLogPart::Line { text, .. } = p {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    if !lines.is_empty() {
        return Some(lines.join("\n"));
    }
    parts.iter().find_map(|p| match p {
        AdminLogPart::Note { parts: inner } if !inner.is_empty() => Some(
            inner
                .iter()
                .map(|tp| tp.text.as_str())
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    })
}
