"""Each admin log action as the parts Telegram Desktop 7.2.10 draws for it (spec 033 R10).

Ported from tdesktop Telegram/SourceFiles/history/admin_log/history_admin_log_item.cpp,
GenerateItems: one `create*` lambda there, one function here, in the same order, GPL-3.0.
The sentences and their helpers are `admin_log_text`.
"""

from __future__ import annotations

from typing import Any, Callable

from telethon.tl import types as tl

from telegram_mcp.admin_log_text import (
    CUSTOM_EMOJI,
    PUBLIC_JOIN_LINK,
    REACTION_PLACEHOLDER,
    Line,
    LogChat,
    Logged,
    Note,
    Original,
    Part,
    can_have_caption,
    edited_text,
    link_lines,
    media_id,
    url_part,
    date_time,
    default_rights_change,
    format_ttl,
    invite_link_change,
    invite_link_text,
    italic,
    participant_change,
    plural,
    topic_link,
    tr,
)
from telegram_mcp.tdexport.model import TextPart, parse_text, peer_color_index, to_time

A = tl  # the ChannelAdminLogEventAction* classes live in telethon.tl.types


def _group(chat: LogChat, group: str, channel: str) -> str:
    return group if chat.megagroup else channel


def _call(chat: LogChat, key: str) -> str:
    return key + "_channel" if chat.broadcast else key


def _change_title(a: Any, chat: LogChat, who: str) -> list[Part]:
    key = _group(chat, "action_changed_title", "changed_title_channel")
    return [Line(tr(key, **{"from": who, "title": a.new_value}))]


def _change_about(a: Any, chat: LogChat, who: str) -> list[Part]:
    kind = "group" if chat.megagroup else "channel"
    verb = "changed" if a.new_value else "removed"
    parts: list[Part] = [Line(tr(f"{verb}_description_{kind}", **{"from": who}))]
    # ponytail: PrepareText also finds links, mentions and hashtags in a description; the
    # export keeps it plain text, which is what the owner typed.
    parts.append(Note([TextPart(text=a.new_value)] if a.new_value else []))
    if a.prev_value:
        parts.append(Original(tr("previous_description"), [TextPart(text=a.prev_value)]))
    return parts


def _change_username(a: Any, chat: LogChat, who: str) -> list[Part]:
    kind = "group" if chat.megagroup else "channel"
    verb = "changed" if a.new_value else "removed"
    parts: list[Part] = [Line(tr(f"{verb}_link_{kind}", **{"from": who}))]
    parts.append(Note(url_part(chat.link_prefix + a.new_value) if a.new_value else []))
    if a.prev_value:
        parts.append(Original(tr("previous_link"), url_part(chat.link_prefix + a.prev_value)))
    return parts


def _change_photo(a: Any, chat: LogChat, who: str) -> list[Part]:
    kind = "group" if chat.megagroup else "channel"
    if isinstance(a.new_photo, tl.Photo):
        return [Line(tr(f"changed_photo_{kind}", **{"from": who}), photo=a.new_photo)]
    return [Line(tr(f"removed_photo_{kind}", **{"from": who}))]


def _toggle(key: str) -> Callable[[Any, LogChat, str], list[Part]]:
    """A Bool switch: `<key>_enabled` when the new value is true, else `<key>_disabled`."""

    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        state = "enabled" if a.new_value else "disabled"
        return [Line(tr(f"{key}_{state}", **{"from": who}))]

    return build


def _update_pinned(a: Any, chat: LogChat, who: str) -> list[Part]:
    message = a.message
    if isinstance(message, tl.Message):
        key = "pinned_message" if message.pinned else "unpinned_message"
        return [Line(tr(key, **{"from": who})), Logged(message)]
    return [Line(tr("unpinned_message", **{"from": who}))]


def _edit_message(a: Any, chat: LogChat, who: str) -> list[Part]:
    new, prev = a.new_message, a.prev_message
    new_value, old_value = edited_text(new), edited_text(prev)
    captioned = can_have_caption(new)
    changed_caption = new_value != old_value
    changed_media = media_id(new) != media_id(prev)
    removed_caption = bool(old_value[0]) and not new_value[0]
    if not captioned:
        key = "edited_message"
    elif changed_media and removed_caption:
        key = "edited_media_and_removed_caption"
    elif changed_media and changed_caption:
        key = "edited_media_and_caption"
    elif changed_media:
        key = "edited_media"
    elif removed_caption:
        key = "removed_caption"
    elif changed_caption:
        key = "edited_caption"
    else:
        key = "edited_message"
    if old_value[0]:
        old_parts = parse_text(prev.message, prev.entities)
    else:
        old_parts = italic(tr("empty_text"))
    with_media = changed_media and can_have_caption(prev) and media_id(prev) != 0
    title = tr("previous_caption" if captioned else "previous_message")
    return [
        Line(tr(key, **{"from": who})),
        Logged(new),
        Original(title, old_parts, prev if with_media else None),
    ]


def _carried(key: str) -> Callable[[Any, LogChat, str], list[Part]]:
    """A sentence followed by the message the event carries (deleted, sent, stopped poll)."""

    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        return [Line(tr(key, **{"from": who})), Logged(a.message)]

    return build


def _simple(group: str, channel: str) -> Callable[[Any, LogChat, str], list[Part]]:
    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        return [Line(tr(_group(chat, group, channel), **{"from": who}))]

    return build


def _by_call(key: str) -> Callable[[Any, LogChat, str], list[Part]]:
    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        return [Line(tr(_call(chat, key), **{"from": who}))]

    return build


def _participant_invite(a: Any, chat: LogChat, who: str) -> list[Part]:
    return [Note(italic(participant_change(chat, a.participant)))]


def _participant_toggle_ban(a: Any, chat: LogChat, who: str) -> list[Part]:
    return [Note(italic(participant_change(chat, a.new_participant, a.prev_participant)))]


def _participant_toggle_admin(a: Any, chat: LogChat, who: str) -> list[Part]:
    if isinstance(a.new_participant, tl.ChannelParticipantAdmin) and isinstance(
        a.prev_participant, tl.ChannelParticipantCreator
    ):
        # An ownership transfer shows in its "User > Creator" part only.
        return []
    return [Note(italic(participant_change(chat, a.new_participant, a.prev_participant)))]


def _sticker_set(kind: str) -> Callable[[Any, LogChat, str], list[Part]]:
    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        if isinstance(a.new_stickerset, tl.InputStickerSetEmpty):
            return [Line(tr(f"removed_{kind}_group", **{"from": who}))]
        set_name = tr(f"changed_{kind}_set")
        return [Line(tr(f"changed_{kind}_group", **{"from": who, "sticker_set": set_name}))]

    return build


def _pre_history(a: Any, chat: LogChat, who: str) -> list[Part]:
    key = "history_made_hidden" if a.new_value else "history_made_visible"
    return [Line(tr(key, **{"from": who}))]


def _default_banned_rights(a: Any, chat: LogChat, who: str) -> list[Part]:
    return [Note(italic(default_rights_change(a.new_banned_rights, a.prev_banned_rights)))]


def _linked_chat(a: Any, chat: LogChat, who: str) -> list[Part]:
    linked = chat.chats.get(a.new_value) if a.new_value else None
    if linked is None:
        key = "removed_linked_chat" if chat.broadcast else "removed_linked_channel"
        return [Line(tr(key, **{"from": who}))]
    key = "changed_linked_chat" if chat.broadcast else "changed_linked_channel"
    return [Line(tr(key, **{"from": who, "chat": linked.title}))]


def _location(a: Any, chat: LogChat, who: str) -> list[Part]:
    if isinstance(a.new_value, tl.ChannelLocation):
        text = tr("changed_location_chat", **{"from": who, "address": a.new_value.address})
        return [Line(text)]
    return [Line(tr("removed_location_chat", **{"from": who}))]


def _slow_mode(a: Any, chat: LogChat, who: str) -> list[Part]:
    seconds = a.new_value
    if not seconds:
        return [Line(tr("removed_slow_mode", **{"from": who}))]
    duration = plural("minutes", seconds // 60) if seconds >= 60 else plural("seconds", seconds)
    return [Line(tr("changed_slow_mode", **{"from": who, "duration": duration}))]


def _call_participant(key: str) -> Callable[[Any, LogChat, str], list[Part]]:
    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        user = chat.name(a.participant.peer)
        return [Line(tr(_call(chat, key), **{"from": who, "user": user}))]

    return build


def _call_setting(a: Any, chat: LogChat, who: str) -> list[Part]:
    key = "disallowed_unmute_self" if a.join_muted else "allowed_unmute_self"
    return [Line(tr(_call(chat, key), **{"from": who}))]


def _join_by_invite(a: Any, chat: LogChat, who: str) -> list[Part]:
    key = "participant_joined_by_filter_link" if a.via_chatlist else "participant_joined_by_link"
    if not chat.megagroup:
        key += "_channel"
    return [Line(tr(key, **{"from": who, "link": invite_link_text(a.invite)}))]


def _invite_event(key: str) -> Callable[[Any, LogChat, str], list[Part]]:
    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        return [Line(tr(key, **{"from": who, "link": invite_link_text(a.invite)}))]

    return build


def _invite_edit(a: Any, chat: LogChat, who: str) -> list[Part]:
    return [Note(italic(invite_link_change(a.new_invite, a.prev_invite)))]


def _volume(a: Any, chat: LogChat, who: str) -> list[Part]:
    participant = a.participant
    volume = participant.volume if participant.volume is not None else 10000
    user = chat.name(participant.peer)
    percent = f"{volume // 100}%"
    return [
        Line(
            tr(
                _call(chat, "participant_volume"),
                **{"from": who, "user": user, "percent": percent},
            )
        )
    ]


def _history_ttl(a: Any, chat: LogChat, who: str) -> list[Part]:
    def wrap(duration: int) -> str:
        return "5 seconds" if duration == 5 else format_ttl(duration)

    was, now = a.prev_value, a.new_value
    if not was:
        text = tr("messages_ttl_set", **{"from": who, "duration": wrap(now)})
    elif not now:
        text = tr("messages_ttl_removed", **{"from": who, "duration": wrap(was)})
    else:
        values = {"from": who, "previous": wrap(was), "duration": wrap(now)}
        text = tr("messages_ttl_changed", **values)
    return [Line(text)]


def _join_by_request(a: Any, chat: LogChat, who: str) -> list[Part]:
    user = chat.name(tl.PeerUser(a.approved_by))
    link = invite_link_text(a.invite)
    suffix = "" if chat.megagroup else "_channel"
    if link == PUBLIC_JOIN_LINK:
        text = tr("participant_approved_by_request" + suffix, **{"from": who, "user": user})
    else:
        values = {"from": who, "link": link, "user": user}
        text = tr("participant_approved_by_link" + suffix, **values)
    return [Line(text)]


def _no_forwards(a: Any, chat: LogChat, who: str) -> list[Part]:
    key = "forwards_disabled" if a.new_value else "forwards_enabled"
    return [Line(tr(key, **{"from": who}))]


def _reaction(reaction: Any) -> str:
    """Manager::ComposeReactionEmoji: the emoji, or the placeholder for a custom one.

    ponytail: Desktop shows a custom reaction's sticker alt when the document is loaded;
    the export has no documents at hand here and writes the placeholder.
    """
    if isinstance(reaction, tl.ReactionEmoji):
        return reaction.emoticon
    return REACTION_PLACEHOLDER


def _reactions(a: Any, chat: LogChat, who: str) -> list[Part]:
    value = a.new_value
    if isinstance(value, tl.ChatReactionsSome):
        emoji = ", ".join(_reaction(r) for r in value.reactions)
        return [Line(tr("reactions_updated", **{"from": who, "emoji": emoji}))]
    if isinstance(value, tl.ChatReactionsAll):
        key = "reactions_allowed_all" if value.allow_custom else "reactions_allowed_official"
        return [Line(tr(key, **{"from": who}))]
    return [Line(tr("reactions_disabled", **{"from": who}))]


def _usernames(a: Any, chat: LogChat, who: str) -> list[Part]:
    new, old = list(a.new_value), list(a.prev_value)
    kind = "group" if chat.megagroup else "channel"
    if len(new) == len(old):
        if len(new) == 1:
            single = tl.ChannelAdminLogEventActionChangeUsername(
                prev_value=old[0], new_value=new[0]
            )
            return _change_username(single, chat, who)
        if all(link in old for link in new):
            return [
                Line(tr(f"reordered_link_{kind}", **{"from": who})),
                Note(link_lines(chat, new)),
                Original(tr("previous_links_order"), link_lines(chat, old)),
            ]
    elif abs(len(new) - len(old)) == 1:
        activated = len(new) > len(old)
        longer, shorter = (new, old) if activated else (old, new)
        changed = next((link for link in longer if link not in shorter), "")
        key = "activated_link" if activated else "deactivated_link"
        return [Line(tr(key, **{"from": who, "link": changed}))]
    # "Probably will never happen." - Desktop's own words, and its own untranslated text.
    return [
        Line(who + f" changed list of {kind} links:"),
        Note(link_lines(chat, new)),
        Original("Previous links", link_lines(chat, old)),
    ]


def _create_topic(a: Any, chat: LogChat, who: str) -> list[Part]:
    return [Line(tr("topics_created", **{"from": who, "topic": topic_link(a.topic)}))]


def _edit_topic(a: Any, chat: LogChat, who: str) -> list[Part]:
    prev, new = a.prev_topic, a.new_topic
    prev_link, now_link = topic_link(prev), topic_link(new)
    parts: list[Part] = []
    same = prev_link == now_link and getattr(prev, "id", 0) == getattr(new, "id", 0)
    if not same:
        values = {"from": who, "topic": prev_link, "new_topic": now_link}
        parts.append(Line(tr("topics_changed", **values)))
    for attribute, on, off in (("closed", "closed", "reopened"), ("hidden", "hidden", "unhidden")):
        was, now = bool(getattr(prev, attribute, False)), bool(getattr(new, attribute, False))
        if was != now:
            key = "topics_" + (on if now else off)
            parts.append(Line(tr(key, **{"from": who, "topic": now_link})))
    return parts


def _delete_topic(a: Any, chat: LogChat, who: str) -> list[Part]:
    return [Line(tr("topics_deleted", **{"from": who, "topic": topic_link(a.topic)}))]


def _pin_topic(a: Any, chat: LogChat, who: str) -> list[Part]:
    if a.new_topic is not None:
        return [Line(tr("topics_pinned", **{"from": who, "topic": topic_link(a.new_topic)}))]
    if a.prev_topic is not None:
        return [Line(tr("topics_unpinned", **{"from": who, "topic": topic_link(a.prev_topic)}))]
    return []


def _color_change(keys: tuple) -> Callable[[Any, LogChat, str], list[Part]]:
    """createColorChange; `keys` are the color, set, removed and changed emoji phrases."""
    color_key, set_key, removed_key, change_key = keys

    def color(value: Any) -> Any:
        return value.color if isinstance(value, tl.PeerColor) else None

    def emoji(value: Any) -> int:
        return (value.background_emoji_id or 0) if isinstance(value, tl.PeerColor) else 0

    def build(a: Any, chat: LogChat, who: str) -> list[Part]:
        parts: list[Part] = []
        was, now = a.prev_value, a.new_value
        if color(was) != color(now):

            def wrap(value: Any) -> str:
                index = value if value is not None else peer_color_index(chat.channel_id)
                return f"#{index + 1}"

            values = {"from": who, "previous": wrap(color(was)), "color": wrap(color(now))}
            parts.append(Line(tr(color_key, **values)))
        if emoji(was) != emoji(now):
            if not emoji(was):
                text = tr(set_key, **{"from": who, "emoji": CUSTOM_EMOJI})
            elif not emoji(now):
                text = tr(removed_key, **{"from": who, "emoji": CUSTOM_EMOJI})
            else:
                values = {"from": who, "previous": CUSTOM_EMOJI, "emoji": CUSTOM_EMOJI}
                text = tr(change_key, **values)
            parts.append(Line(text))
        return parts

    return build


def _profile_color(a: Any, chat: LogChat, who: str) -> list[Part]:
    suffix = "_group" if chat.megagroup else ""
    keys = (
        "change_profile_color" + suffix,
        "set_profile_background_emoji" + suffix,
        "removed_profile_background_emoji" + suffix,
        "change_profile_background_emoji" + suffix,
    )
    return _color_change(keys)(a, chat, who)


def _emoji_status(a: Any, chat: LogChat, who: str) -> list[Part]:
    def document(status: Any) -> int:
        if isinstance(status, (tl.EmojiStatus, tl.EmojiStatusCollectible)):
            return status.document_id
        return 0

    prev, new = document(a.prev_value), document(a.new_value)
    until = to_time(a.new_value.until) if isinstance(a.new_value, tl.EmojiStatus) else 0
    values = {"from": who, "emoji": CUSTOM_EMOJI, "previous": CUSTOM_EMOJI}
    if until:
        values["date"] = date_time(until)
    if not prev:
        key = "set_status_until" if until else "set_status"
    elif not new:
        key = "removed_status"
    else:
        key = "change_status_until" if until else "change_status"
    return [Line(tr(key, **values))]


def _sub_extend(a: Any, chat: LogChat, who: str) -> list[Part]:
    participant = a.new_participant
    until = to_time(getattr(participant, "subscription_until_date", None))
    if not until:
        return []
    name = chat.name(tl.PeerUser(getattr(participant, "user_id", 0)))
    return [Line(tr("subscription_extend", name=name, date=date_time(until, full=True)))]


def _edit_rank(a: Any, chat: LogChat, who: str, actor_id: int) -> list[Part]:
    prev, new = a.prev_rank or "", a.new_rank or ""
    values = {"from": who, "previous": prev, "tag": new}
    if a.user_id == actor_id:
        key = "removed_own_rank" if not new else "set_own_rank" if not prev else None
        return [Line(tr(key or "changed_own_rank_from", **values))]
    values["user"] = chat.name(tl.PeerUser(a.user_id))
    key = "removed_rank" if not new else "set_rank" if not prev else "changed_rank_from"
    return [Line(tr(key, **values))]


_HANDLERS: dict[type, Callable[[Any, LogChat, str], list[Part]]] = {
    A.ChannelAdminLogEventActionChangeTitle: _change_title,
    A.ChannelAdminLogEventActionChangeAbout: _change_about,
    A.ChannelAdminLogEventActionChangeUsername: _change_username,
    A.ChannelAdminLogEventActionChangePhoto: _change_photo,
    A.ChannelAdminLogEventActionToggleInvites: _toggle("invites"),
    A.ChannelAdminLogEventActionToggleSignatures: _toggle("signatures"),
    A.ChannelAdminLogEventActionUpdatePinned: _update_pinned,
    A.ChannelAdminLogEventActionEditMessage: _edit_message,
    A.ChannelAdminLogEventActionDeleteMessage: _carried("deleted_message"),
    A.ChannelAdminLogEventActionParticipantJoin: _simple(
        "participant_joined", "participant_joined_channel"
    ),
    A.ChannelAdminLogEventActionParticipantLeave: _simple(
        "participant_left", "participant_left_channel"
    ),
    A.ChannelAdminLogEventActionParticipantInvite: _participant_invite,
    A.ChannelAdminLogEventActionParticipantToggleBan: _participant_toggle_ban,
    A.ChannelAdminLogEventActionParticipantToggleAdmin: _participant_toggle_admin,
    A.ChannelAdminLogEventActionChangeStickerSet: _sticker_set("stickers"),
    A.ChannelAdminLogEventActionChangeEmojiStickerSet: _sticker_set("emoji"),
    A.ChannelAdminLogEventActionTogglePreHistoryHidden: _pre_history,
    A.ChannelAdminLogEventActionDefaultBannedRights: _default_banned_rights,
    A.ChannelAdminLogEventActionStopPoll: _carried("stopped_poll"),
    A.ChannelAdminLogEventActionChangeLinkedChat: _linked_chat,
    A.ChannelAdminLogEventActionChangeLocation: _location,
    A.ChannelAdminLogEventActionToggleSlowMode: _slow_mode,
    A.ChannelAdminLogEventActionStartGroupCall: _by_call("started_group_call"),
    A.ChannelAdminLogEventActionDiscardGroupCall: _by_call("discarded_group_call"),
    A.ChannelAdminLogEventActionParticipantMute: _call_participant("muted_participant"),
    A.ChannelAdminLogEventActionParticipantUnmute: _call_participant("unmuted_participant"),
    A.ChannelAdminLogEventActionToggleGroupCallSetting: _call_setting,
    A.ChannelAdminLogEventActionParticipantJoinByInvite: _join_by_invite,
    A.ChannelAdminLogEventActionExportedInviteDelete: _invite_event("delete_invite_link"),
    A.ChannelAdminLogEventActionExportedInviteRevoke: _invite_event("revoke_invite_link"),
    A.ChannelAdminLogEventActionExportedInviteEdit: _invite_edit,
    A.ChannelAdminLogEventActionParticipantVolume: _volume,
    A.ChannelAdminLogEventActionChangeHistoryTTL: _history_ttl,
    A.ChannelAdminLogEventActionParticipantJoinByRequest: _join_by_request,
    A.ChannelAdminLogEventActionToggleNoForwards: _no_forwards,
    A.ChannelAdminLogEventActionSendMessage: _carried("sent_message"),
    A.ChannelAdminLogEventActionChangeAvailableReactions: _reactions,
    A.ChannelAdminLogEventActionChangeUsernames: _usernames,
    A.ChannelAdminLogEventActionToggleForum: _toggle("topics"),
    A.ChannelAdminLogEventActionCreateTopic: _create_topic,
    A.ChannelAdminLogEventActionEditTopic: _edit_topic,
    A.ChannelAdminLogEventActionDeleteTopic: _delete_topic,
    A.ChannelAdminLogEventActionPinTopic: _pin_topic,
    A.ChannelAdminLogEventActionToggleAntiSpam: _toggle("antispam"),
    A.ChannelAdminLogEventActionChangePeerColor: _color_change(
        (
            "change_color",
            "set_background_emoji",
            "removed_background_emoji",
            "change_background_emoji",
        )
    ),
    A.ChannelAdminLogEventActionChangeProfilePeerColor: _profile_color,
    A.ChannelAdminLogEventActionChangeWallpaper: lambda a, chat, who: [
        Line(tr("change_wallpaper", **{"from": who}))
    ],
    A.ChannelAdminLogEventActionChangeEmojiStatus: _emoji_status,
    A.ChannelAdminLogEventActionToggleSignatureProfiles: _toggle("signature_profiles"),
    A.ChannelAdminLogEventActionParticipantSubExtend: _sub_extend,
    A.ChannelAdminLogEventActionToggleAutotranslation: _toggle("autotranslate"),
}


def event_parts(event: Any, chat: LogChat) -> list[Part]:
    """GenerateItems: what Desktop's Recent actions shows for one event, in order.

    An action this port does not know (a newer layer than 229) yields nothing, as
    Desktop's own `action.match` has no case for it either.
    """
    who = chat.name(tl.PeerUser(event.user_id))
    action = event.action
    if isinstance(action, A.ChannelAdminLogEventActionParticipantEditRank):
        return _edit_rank(action, chat, who, event.user_id)
    handler = _HANDLERS.get(type(action))
    return handler(action, chat, who) if handler is not None else []


#: Every action GenerateItems draws, for the tests' completeness check.
HANDLED = frozenset(_HANDLERS) | {A.ChannelAdminLogEventActionParticipantEditRank}
