//! Telegram Desktop 7.2.10 English Recent Actions phrase catalog, TTL, and timestamp formatting.

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};

#[rustfmt::skip]
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
];

#[rustfmt::skip]
pub const ADMIN_RIGHTS: [(&[&str], Option<&str>); 14] = [
    (&["change_info"], Some("Change info")), (&["post_messages"], Some("Post messages")),
    (&["edit_messages"], Some("Edit messages")), (&["delete_messages"], Some("Delete messages")),
    (&["ban_users"], Some("Ban users")), (&["invite_users"], None),
    (&["pin_messages"], Some("Pin messages")), (&["add_admins"], Some("Add new admins")),
    (&["anonymous"], Some("Remain anonymous")), (&["manage_call"], None),
    (&["manage_topics"], Some("Manage topics")), (&["manage_direct_messages"], Some("Manage direct messages")),
    (&["manage_ranks"], Some("Edit member tags")), (&["manage_welcome_messages"], Some("Manage Welcome Messages")),
];

#[rustfmt::skip]
pub const RESTRICTIONS: [(&[&str], Option<&str>); 17] = [
    (&["view_messages"], Some("Read messages")),
    (&["send_stickers", "send_gifs", "send_games", "send_inline"], Some("Send stickers & GIFs")),
    (&["embed_links"], Some("Embed links")), (&["send_polls"], Some("Send polls")),
    (&["change_info"], Some("Change info")), (&["invite_users"], Some("Add users")),
    (&["pin_messages"], Some("Pin messages")), (&["manage_topics"], Some("Create topics")),
    (&["send_photos"], Some("Send photos")), (&["send_videos"], Some("Send video files")),
    (&["send_roundvideos"], Some("Send video messages")), (&["send_audios"], Some("Send music")),
    (&["send_voices"], Some("Send voice messages")), (&["send_docs"], Some("Send files")),
    (&["send_plain"], Some("Send messages")), (&["edit_rank"], None),
    (&["send_reactions"], Some("Send reactions")),
];

#[rustfmt::skip]
const LANG: &[(&str, &str)] = &[
    ("action_changed_title", "{from} changed group name to \u{00ab}{title}\u{00bb}"),
    ("changed_title_channel", "{from} changed channel name to \u{00ab}{title}\u{00bb}"),
    ("changed_description_group", "{from} edited group description:"),
    ("removed_description_group", "{from} removed group description"),
    ("changed_description_channel", "{from} edited channel description:"),
    ("removed_description_channel", "{from} removed channel description"),
    ("previous_description", "Previous description"),
    ("previous_link", "Previous link"),
    ("previous_links_order", "Previous order"),
    ("changed_link_group", "{from} changed group link:"),
    ("removed_link_group", "{from} removed group link"),
    ("changed_link_channel", "{from} changed channel link:"),
    ("removed_link_channel", "{from} removed channel link"),
    ("reordered_link_group", "{from} reordered group links:"),
    ("reordered_link_channel", "{from} reordered channel links:"),
    ("activated_link", "{from} activated @{link} username"),
    ("deactivated_link", "{from} deactivated @{link} username"),
    ("changed_photo_group", "{from} changed group photo"),
    ("changed_photo_channel", "{from} changed channel photo"),
    ("removed_photo_group", "{from} removed group photo"),
    ("removed_photo_channel", "{from} removed channel photo"),
    ("invites_enabled", "{from} enabled group invites"),
    ("invites_disabled", "{from} disabled group invites"),
    ("signatures_enabled", "{from} enabled signatures"),
    ("signatures_disabled", "{from} disabled signatures"),
    ("signature_profiles_enabled", "{from} enabled showing authors' profiles"),
    ("signature_profiles_disabled", "{from} disabled showing authors' profiles"),
    ("forwards_enabled", "{from} allowed saving content"),
    ("forwards_disabled", "{from} restricted saving content"),
    ("history_made_hidden", "{from} made the group history hidden for new members"),
    ("history_made_visible", "{from} made group history visible for new members"),
    ("pinned_message", "{from} pinned message:"),
    ("unpinned_message", "{from} unpinned message"),
    ("edited_caption", "{from} edited caption:"),
    ("edited_media", "{from} edited media:"),
    ("edited_media_and_caption", "{from} edited media and caption:"),
    ("edited_media_and_removed_caption", "{from} edited media and removed caption:"),
    ("removed_caption", "{from} removed caption"),
    ("previous_caption", "Original caption"),
    ("edited_message", "{from} edited message:"),
    ("previous_message", "Original message"),
    ("empty_text", "Empty"),
    ("deleted_message", "{from} deleted message:"),
    ("sent_message", "{from} sent this message:"),
    ("stopped_poll", "{from} stopped poll:"),
    ("participant_joined", "{from} joined the group"),
    ("participant_joined_channel", "{from} joined the channel"),
    ("participant_joined_by_link", "{from} joined the group via {link}"),
    ("participant_joined_by_link_channel", "{from} joined the channel via {link}"),
    ("participant_joined_by_filter_link", "{from} joined via folder invite link {link}"),
    ("participant_joined_by_filter_link_channel", "{from} joined via folder invite link {link}"),
    ("participant_approved_by_link", "{from} was approved to join the group via {link} by {user}"),
    ("participant_approved_by_link_channel", "{from} was approved to join the channel via {link} by {user}"),
    ("participant_approved_by_request", "{from} joined the group via public request, approved by {user}"),
    ("participant_approved_by_request_channel", "{from} joined the channel via public request, approved by {user}"),
    ("revoke_invite_link", "{from} revoked invite link {link}"),
    ("delete_invite_link", "{from} deleted the invite link {link}"),
    ("participant_left", "{from} left the group"),
    ("participant_left_channel", "{from} left the channel"),
    ("invited", "invited {user}"),
    ("banned", "banned {user}"),
    ("banned_until", "banned {user} {until}"),
    ("unbanned", "unbanned {user}"),
    ("restricted", "changed restrictions for {user} {until}"),
    ("promoted", "changed privileges for {user}"),
    ("transferred", "transferred ownership to {user}"),
    ("changed_default_permissions", "changed default permissions"),
    ("changed_stickers_group", "{from} changed the group {sticker_set}"),
    ("changed_stickers_set", "sticker set"),
    ("removed_stickers_group", "{from} removed the group sticker set"),
    ("changed_emoji_group", "{from} changed the group's {sticker_set}"),
    ("changed_emoji_set", "emoji set"),
    ("removed_emoji_group", "{from} removed the group's emoji set"),
    ("changed_linked_chat", "{from} changed the discussion group to \u{00ab}{chat}\u{00bb}"),
    ("removed_linked_chat", "{from} removed the discussion group"),
    ("changed_linked_channel", "{from} changed the linked channel to \u{00ab}{chat}\u{00bb}"),
    ("removed_linked_channel", "{from} removed the linked channel"),
    ("changed_location_chat", "{from} changed the group location to {address}"),
    ("removed_location_chat", "{from} removed the group location"),
    ("changed_slow_mode", "{from} changed slow mode to {duration}"),
    ("removed_slow_mode", "{from} disabled slow mode"),
    ("started_group_call", "{from} started a new video chat"),
    ("started_group_call_channel", "{from} started a new live stream"),
    ("discarded_group_call", "{from} ended the video chat"),
    ("discarded_group_call_channel", "{from} ended the live stream"),
    ("muted_participant", "{from} muted {user} in a video chat"),
    ("muted_participant_channel", "{from} muted {user} in a live stream"),
    ("unmuted_participant", "{from} unmuted {user} in a video chat"),
    ("unmuted_participant_channel", "{from} unmuted {user} in a live stream"),
    ("allowed_unmute_self", "{from} allowed new video chat participants to speak"),
    ("allowed_unmute_self_channel", "{from} allowed new live stream participants to speak"),
    ("disallowed_unmute_self", "{from} muted new video chat participants"),
    ("disallowed_unmute_self_channel", "{from} muted new live stream participants"),
    ("participant_volume", "{from} changed video chat volume for {user} to {percent}"),
    ("participant_volume_channel", "{from} changed live stream volume for {user} to {percent}"),
    ("antispam_enabled", "{from} enabled aggressive anti-spam"),
    ("antispam_disabled", "{from} disabled aggressive anti-spam"),
    ("autotranslate_enabled", "{from} enabled automatic translation"),
    ("autotranslate_disabled", "{from} disabled automatic translation"),
    ("change_color", "{from} changed channel color from {previous} to {color}"),
    ("set_background_emoji", "{from} set channel background emoji to {emoji}"),
    ("change_background_emoji", "{from} changed channel background emoji from {previous} to {emoji}"),
    ("removed_background_emoji", "{from} removed channel background emoji {emoji}"),
    ("change_profile_color", "{from} changed channel profile color from {previous} to {color}"),
    ("set_profile_background_emoji", "{from} set channel profile background emoji to {emoji}"),
    ("change_profile_background_emoji", "{from} changed channel profile background emoji from {previous} to {emoji}"),
    ("removed_profile_background_emoji", "{from} removed channel profile background emoji {emoji}"),
    ("change_profile_color_group", "{from} changed group profile color from {previous} to {color}"),
    ("set_profile_background_emoji_group", "{from} set group profile background emoji to {emoji}"),
    ("change_profile_background_emoji_group", "{from} changed group profile background emoji from {previous} to {emoji}"),
    ("removed_profile_background_emoji_group", "{from} removed group profile background emoji {emoji}"),
    ("change_wallpaper", "{from} changed channel wallpaper"),
    ("user_with_username", "{name} ({mention})"),
    ("set_status", "{from} set channel emoji status to {emoji}"),
    ("removed_status", "{from} removed channel emoji status {emoji}"),
    ("change_status", "{from} changed channel emoji status from {previous} to {emoji}"),
    ("set_status_until", "{from} set channel emoji status to {emoji} until {date}"),
    ("change_status_until", "{from} changed channel emoji status from {previous} to {emoji} until {date}"),
    ("messages_ttl_set", "{from} enabled messages auto-delete after {duration}"),
    ("messages_ttl_changed", "{from} changed messages auto-delete period from {previous} to {duration}"),
    ("messages_ttl_removed", "{from} disabled messages auto-deletion after {duration}"),
    ("reactions_disabled", "{from} disabled reactions"),
    ("reactions_allowed_all", "{from} allowed all reactions"),
    ("reactions_updated", "{from} updated the list of allowed reactions to: {emoji}"),
    ("reactions_allowed_official", "{from} allowed all official reactions"),
    ("edited_invite_link", "edited invite link {link}"),
    ("invite_link_expire_date", "Expiry date: {previous} \u{2192} {limit}"),
    ("invite_link_usage_limit", "Usage limit: {previous} \u{2192} {limit}"),
    ("invite_link_label", "Name: {previous} \u{2192} {limit}"),
    ("invite_link_request_needed", "Admin approval is now required to join."),
    ("invite_link_request_not_needed", "Admin approval no longer required to join."),
    ("topics_enabled", "{from} enabled topics"),
    ("topics_disabled", "{from} disabled topics"),
    ("topics_created", "{from} created topic {topic}"),
    ("topics_changed", "{from} renamed topic {topic} to {new_topic}"),
    ("topics_closed", "{from} closed topic {topic}"),
    ("topics_reopened", "{from} reopened topic {topic}"),
    ("topics_hidden", "{from} hid topic {topic}"),
    ("topics_unhidden", "{from} unhid topic {topic}"),
    ("topics_deleted", "{from} deleted topic {topic}"),
    ("topics_pinned", "{from} pinned topic {topic}"),
    ("topics_unpinned", "{from} unpinned topic {topic}"),
    ("restricted_forever", "indefinitely"),
    ("restricted_until", "until {date}"),
    ("subscription_extend", "{name} renewed subscription until {date}"),
    ("changed_rank_from", "{from} changed tag for {user} from \"{previous}\" to \"{tag}\""),
    ("set_rank", "{from} set tag for {user} to \"{tag}\""),
    ("removed_rank", "{from} removed tag for {user} (was \"{previous}\")"),
    ("changed_own_rank_from", "{from} changed own tag from \"{previous}\" to \"{tag}\""),
    ("set_own_rank", "{from} set own tag to \"{tag}\""),
    ("removed_own_rank", "{from} removed own tag (was \"{previous}\")"),
    ("group_invite_no_limit", "No limit"),
    ("mediaview_date_time", "{date} at {time}"),
];

pub fn tr(key: &str, vars: &[(&str, &str)]) -> String {
    let tpl = LANG.iter().find(|(k, _)| *k == key).map_or("", |(_, v)| *v);
    let (mut out, mut rest) = (String::with_capacity(tpl.len() + 16), tpl);
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let tok = &after[..close];
            if !tok.is_empty() && tok.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                let rep = vars.iter().find(|(k, _)| *k == tok);
                out.push_str(rep.map_or(&rest[open..=open + 1 + close], |(_, val)| *val));
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

pub fn plural(unit: &str, count: i64) -> String {
    let known = matches!(
        unit,
        "seconds" | "minutes" | "hours" | "days" | "weeks" | "months" | "years"
    );
    let s = unit.strip_suffix('s').filter(|_| known).unwrap_or(unit);
    if count == 1 {
        format!("{count} {s}")
    } else {
        format!("{count} {s}s")
    }
}

pub fn format_ttl(seconds: i64) -> String {
    let day = 86_400_i64;
    if seconds < day {
        plural("hours", seconds.div_euclid(3_600))
    } else if seconds < day * 7 {
        plural("days", seconds.div_euclid(day))
    } else if seconds < day * 31 {
        let days = seconds.div_euclid(day);
        let weeks = plural("weeks", days.div_euclid(7));
        if seconds.rem_euclid(7) == 0 {
            weeks
        } else {
            format!("{weeks} {}", plural("days", days.rem_euclid(7)))
        }
    } else if seconds <= day * 31 * 11 {
        plural("months", seconds.div_euclid(day * 31))
    } else {
        plural(
            "years",
            ((seconds as f64) / ((day * 365) as f64)).round_ties_even() as i64,
        )
    }
}

pub fn format_history_ttl(seconds: i64) -> String {
    if seconds == 5 {
        "5 seconds".into()
    } else {
        format_ttl(seconds)
    }
}

pub fn format_date_time(when: i64, full: bool) -> String {
    format_date_time_at(when, full, Utc::now().date_naive(), 0)
}

pub fn format_date_time_at(
    when: i64,
    full: bool,
    today: NaiveDate,
    utc_offset_seconds: i32,
) -> String {
    let shifted = when.saturating_add(i64::from(utc_offset_seconds));
    let dt = DateTime::from_timestamp(shifted, 0).unwrap_or(DateTime::UNIX_EPOCH);
    let day = dt.date_naive();
    let mname = MONTHS[(day.month() - 1) as usize];
    let month = if full { mname } else { &mname[..3] };
    let (dy, ty, dm, tm) = (
        day.year(),
        today.year(),
        day.month() as i32,
        today.month() as i32,
    );
    let far = (dy - ty).abs() > 1
        || (dy == ty + 1 && dm + 12 > tm + 3)
        || (ty == dy + 1 && tm + 12 > dm + 3);
    let dstr = if far {
        format!("{month} {}, {dy}", day.day())
    } else {
        format!("{month} {}", day.day())
    };
    let tstr = format!("{:02}:{:02}", dt.hour(), dt.minute());
    tr("mediaview_date_time", &[("date", &dstr), ("time", &tstr)])
}

pub fn peer_color_index(bare_id: i64) -> i64 {
    [0, 7, 4, 1, 6, 3, 5][bare_id.rem_euclid(7) as usize]
}

pub fn unknown_action_sentence(who: &str, safe_action: &str) -> String {
    format!("{who} performed an unrecognized action ({safe_action})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_single_pass_templates_and_ttl_ranges() {
        assert_eq!(
            tr(
                "action_changed_title",
                &[("from", "{title}"), ("title", "Real")]
            ),
            "{title} changed group name to \u{00ab}Real\u{00bb}"
        );
        assert_eq!(format_history_ttl(5), "5 seconds");
        assert_eq!(format_ttl(3_600), "1 hour");
        assert_eq!(format_ttl(86_400 * 14), "2 weeks");
        assert_eq!(format_ttl(86_400 * 10), "1 week 3 days");
        assert_eq!(format_ttl(86_400 * 62), "2 months");
        assert_eq!(format_ttl(86_400 * 730), "2 years");
    }
}
