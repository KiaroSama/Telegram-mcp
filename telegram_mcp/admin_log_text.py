"""What Telegram Desktop 7.2.10's Recent actions view shows for each admin log event (spec 033 R10).

Ported from tdesktop Telegram/SourceFiles/history/admin_log/history_admin_log_item.cpp
(GenerateItems and the helpers above it) with the English strings of
Telegram/Resources/langs/lang.strings, GPL-3.0. An event becomes the parts Desktop draws for
it, in order:

* `Line` - a service line, Desktop's sentence ("{from} deleted message:");
* `Note` - a message from the actor (restrictions, admin rights, default permissions, an
  edited invite link, a new description or link), italic where Desktop's is;
* `Logged` - the message the event carries (deleted, edited, pinned, sent, a stopped poll);
* `Original` - the "Original message" / "Previous description" block under it.

Only plain text leaves here: a custom emoji is written as Desktop's own text for it, "@".
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import date
from typing import Any

from telethon.tl import types as tl

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.model import TextPart, to_time

_LANG = {
    "action_changed_title": "{from} changed group name to \u00ab{title}\u00bb",
    "changed_title_channel": "{from} changed channel name to \u00ab{title}\u00bb",
    "changed_description_group": "{from} edited group description:",
    "removed_description_group": "{from} removed group description",
    "changed_description_channel": "{from} edited channel description:",
    "removed_description_channel": "{from} removed channel description",
    "previous_description": "Previous description",
    "changed_link_group": "{from} changed group link:",
    "removed_link_group": "{from} removed group link",
    "changed_link_channel": "{from} changed channel link:",
    "removed_link_channel": "{from} removed channel link",
    "previous_link": "Previous link",
    "reordered_link_group": "{from} reordered group links:",
    "reordered_link_channel": "{from} reordered channel links:",
    "previous_links_order": "Previous order",
    "activated_link": "{from} activated @{link} username",
    "deactivated_link": "{from} deactivated @{link} username",
    "changed_photo_group": "{from} changed group photo",
    "changed_photo_channel": "{from} changed channel photo",
    "removed_photo_group": "{from} removed group photo",
    "removed_photo_channel": "{from} removed channel photo",
    "invites_enabled": "{from} enabled group invites",
    "invites_disabled": "{from} disabled group invites",
    "signatures_enabled": "{from} enabled signatures",
    "signatures_disabled": "{from} disabled signatures",
    "signature_profiles_enabled": "{from} enabled showing authors' profiles",
    "signature_profiles_disabled": "{from} disabled showing authors' profiles",
    "forwards_enabled": "{from} allowed saving content",
    "forwards_disabled": "{from} restricted saving content",
    "history_made_hidden": "{from} made the group history hidden for new members",
    "history_made_visible": "{from} made group history visible for new members",
    "pinned_message": "{from} pinned message:",
    "unpinned_message": "{from} unpinned message",
    "edited_caption": "{from} edited caption:",
    "edited_media": "{from} edited media:",
    "edited_media_and_caption": "{from} edited media and caption:",
    "edited_media_and_removed_caption": "{from} edited media and removed caption:",
    "removed_caption": "{from} removed caption",
    "previous_caption": "Original caption",
    "edited_message": "{from} edited message:",
    "previous_message": "Original message",
    "empty_text": "Empty",
    "deleted_message": "{from} deleted message:",
    "sent_message": "{from} sent this message:",
    "participant_joined": "{from} joined the group",
    "participant_joined_channel": "{from} joined the channel",
    "participant_joined_by_link": "{from} joined the group via {link}",
    "participant_joined_by_link_channel": "{from} joined the channel via {link}",
    "participant_joined_by_filter_link": "{from} joined via folder invite link {link}",
    "participant_joined_by_filter_link_channel": "{from} joined via folder invite link {link}",
    "participant_approved_by_link": "{from} was approved to join the group via {link} by {user}",
    "participant_approved_by_link_channel": (
        "{from} was approved to join the channel via {link} by {user}"
    ),
    "participant_approved_by_request": (
        "{from} joined the group via public request, approved by {user}"
    ),
    "participant_approved_by_request_channel": (
        "{from} joined the channel via public request, approved by {user}"
    ),
    "revoke_invite_link": "{from} revoked invite link {link}",
    "delete_invite_link": "{from} deleted the invite link {link}",
    "participant_left": "{from} left the group",
    "participant_left_channel": "{from} left the channel",
    "stopped_poll": "{from} stopped poll:",
    "invited": "invited {user}",
    "banned": "banned {user}",
    "banned_until": "banned {user} {until}",
    "unbanned": "unbanned {user}",
    "restricted": "changed restrictions for {user} {until}",
    "promoted": "changed privileges for {user}",
    "transferred": "transferred ownership to {user}",
    "changed_default_permissions": "changed default permissions",
    "changed_stickers_group": "{from} changed the group {sticker_set}",
    "changed_stickers_set": "sticker set",
    "removed_stickers_group": "{from} removed the group sticker set",
    "changed_emoji_group": "{from} changed the group's {sticker_set}",
    "changed_emoji_set": "emoji set",
    "removed_emoji_group": "{from} removed the group's emoji set",
    "changed_linked_chat": "{from} changed the discussion group to \u00ab{chat}\u00bb",
    "removed_linked_chat": "{from} removed the discussion group",
    "changed_linked_channel": "{from} changed the linked channel to \u00ab{chat}\u00bb",
    "removed_linked_channel": "{from} removed the linked channel",
    "changed_location_chat": "{from} changed the group location to {address}",
    "removed_location_chat": "{from} removed the group location",
    "changed_slow_mode": "{from} changed slow mode to {duration}",
    "removed_slow_mode": "{from} disabled slow mode",
    "started_group_call": "{from} started a new video chat",
    "started_group_call_channel": "{from} started a new live stream",
    "discarded_group_call": "{from} ended the video chat",
    "discarded_group_call_channel": "{from} ended the live stream",
    "muted_participant": "{from} muted {user} in a video chat",
    "muted_participant_channel": "{from} muted {user} in a live stream",
    "unmuted_participant": "{from} unmuted {user} in a video chat",
    "unmuted_participant_channel": "{from} unmuted {user} in a live stream",
    "allowed_unmute_self": "{from} allowed new video chat participants to speak",
    "allowed_unmute_self_channel": "{from} allowed new live stream participants to speak",
    "disallowed_unmute_self": "{from} muted new video chat participants",
    "disallowed_unmute_self_channel": "{from} muted new live stream participants",
    "participant_volume": "{from} changed video chat volume for {user} to {percent}",
    "participant_volume_channel": "{from} changed live stream volume for {user} to {percent}",
    "antispam_enabled": "{from} enabled aggressive anti-spam",
    "antispam_disabled": "{from} disabled aggressive anti-spam",
    "autotranslate_enabled": "{from} enabled automatic translation",
    "autotranslate_disabled": "{from} disabled automatic translation",
    "change_color": "{from} changed channel color from {previous} to {color}",
    "set_background_emoji": "{from} set channel background emoji to {emoji}",
    "change_background_emoji": (
        "{from} changed channel background emoji from {previous} to {emoji}"
    ),
    "removed_background_emoji": "{from} removed channel background emoji {emoji}",
    "change_profile_color": "{from} changed channel profile color from {previous} to {color}",
    "set_profile_background_emoji": "{from} set channel profile background emoji to {emoji}",
    "change_profile_background_emoji": (
        "{from} changed channel profile background emoji from {previous} to {emoji}"
    ),
    "removed_profile_background_emoji": "{from} removed channel profile background emoji {emoji}",
    "change_profile_color_group": "{from} changed group profile color from {previous} to {color}",
    "set_profile_background_emoji_group": "{from} set group profile background emoji to {emoji}",
    "change_profile_background_emoji_group": (
        "{from} changed group profile background emoji from {previous} to {emoji}"
    ),
    "removed_profile_background_emoji_group": (
        "{from} removed group profile background emoji {emoji}"
    ),
    "change_wallpaper": "{from} changed channel wallpaper",
    "set_status": "{from} set channel emoji status to {emoji}",
    "change_status": "{from} changed channel emoji status from {previous} to {emoji}",
    "removed_status": "{from} removed channel emoji status {emoji}",
    "set_status_until": "{from} set channel emoji status to {emoji} until {date}",
    "change_status_until": (
        "{from} changed channel emoji status from {previous} to {emoji} until {date}"
    ),
    "user_with_username": "{name} ({mention})",
    "messages_ttl_set": "{from} enabled messages auto-delete after {duration}",
    "messages_ttl_changed": (
        "{from} changed messages auto-delete period from {previous} to {duration}"
    ),
    "messages_ttl_removed": "{from} disabled messages auto-deletion after {duration}",
    "reactions_disabled": "{from} disabled reactions",
    "reactions_updated": "{from} updated the list of allowed reactions to: {emoji}",
    "reactions_allowed_all": "{from} allowed all reactions",
    "reactions_allowed_official": "{from} allowed all official reactions",
    "edited_invite_link": "edited invite link {link}",
    "invite_link_expire_date": "Expiry date: {previous} \u2192 {limit}",
    "invite_link_usage_limit": "Usage limit: {previous} \u2192 {limit}",
    "invite_link_label": "Name: {previous} \u2192 {limit}",
    "invite_link_request_needed": "Admin approval is now required to join.",
    "invite_link_request_not_needed": "Admin approval no longer required to join.",
    "topics_enabled": "{from} enabled topics",
    "topics_disabled": "{from} disabled topics",
    "topics_created": "{from} created topic {topic}",
    "topics_changed": "{from} renamed topic {topic} to {new_topic}",
    "topics_closed": "{from} closed topic {topic}",
    "topics_reopened": "{from} reopened topic {topic}",
    "topics_hidden": "{from} hid topic {topic}",
    "topics_unhidden": "{from} unhid topic {topic}",
    "topics_deleted": "{from} deleted topic {topic}",
    "topics_pinned": "{from} pinned topic {topic}",
    "topics_unpinned": "{from} unpinned topic {topic}",
    "restricted_forever": "indefinitely",
    "restricted_until": "until {date}",
    "subscription_extend": "{name} renewed subscription until {date}",
    "changed_rank_from": '{from} changed tag for {user} from "{previous}" to "{tag}"',
    "set_rank": '{from} set tag for {user} to "{tag}"',
    "removed_rank": '{from} removed tag for {user} (was "{previous}")',
    "changed_own_rank_from": '{from} changed own tag from "{previous}" to "{tag}"',
    "set_own_rank": '{from} set own tag to "{tag}"',
    "removed_own_rank": '{from} removed own tag (was "{previous}")',
    # Not lng_admin_log_*: the other strings GenerateItems uses.
    "group_invite_no_limit": "No limit",
    "mediaview_date_time": "{date} at {time}",
}
_PLURALS = {
    "seconds": "second",
    "minutes": "minute",
    "hours": "hour",
    "days": "day",
    "weeks": "week",
    "months": "month",
    "years": "year",
}
_MONTHS = (
    "January February March April May June July August September October November December"
).split()
_PLACEHOLDER = re.compile(r"\{(\w+)\}")
#: Ui::Text::SingleCustomEmoji's text, and Manager's PlaceholderReactionText.
CUSTOM_EMOJI = "@"
REACTION_PLACEHOLDER = "\U0001f4ad"
PUBLIC_JOIN_LINK = "(public_join_link)"
_MINUS = "\u2212"
_RESTRICT_FOREVER = 2**31 - 1  # ChannelData::kRestrictUntilForever


def tr(key: str, **values: str) -> str:
    """tr::lng_admin_log_<key>(tr::now, lt_x, ...): every {x} replaced at once."""
    return _PLACEHOLDER.sub(lambda m: values.get(m[1], m[0]), _LANG[key])


def plural(unit: str, count: int) -> str:
    """tr::lng_<unit>(tr::now, lt_count, count), English plural rules."""
    return f"{count} {_PLURALS[unit]}" + ("" if count == 1 else "s")


def _today() -> date:
    return date.today()


def _day_of_month(day: date, full: bool) -> str:
    """langDayOfMonth / langDayOfMonthFull: the year only when it is far from today's."""
    month = _MONTHS[day.month - 1] if full else _MONTHS[day.month - 1][:3]
    today = _today()
    far = abs(day.year - today.year) > 1 or (
        (day.year == today.year + 1 and day.month + 12 > today.month + 3)
        or (today.year == day.year + 1 and today.month + 12 > day.month + 3)
    )
    return f"{month} {day.day}, {day.year}" if far else f"{month} {day.day}"


def date_time(when: int, full: bool = False) -> str:
    """langDateTime(base::unixtime::parse(when)): local time.

    ponytail: QLocale's short time format is the machine's; 24-hour HH:mm (the C locale's)
    keeps an export the same wherever it is made.
    """
    moment = model_format._local_datetime(when)
    return tr(
        "mediaview_date_time",
        date=_day_of_month(moment.date(), full),
        time=moment.strftime("%H:%M"),
    )


def italic(text: str) -> list[TextPart]:
    """A text with Desktop's whole-text Italic entity, flattened as the export flattens it."""
    return [TextPart(type=TextPart.Type.Italic, text=text)] if text else []


@dataclass
class Line:
    text: str
    photo: Any = None  # the tl.Photo Desktop shows under the line


@dataclass
class Note:
    parts: list[TextPart]


@dataclass
class Logged:
    message: Any


@dataclass
class Original:
    title: str
    parts: list[TextPart]
    #: The previous version of an edited message, when its photo or file changed.
    message: Any = None


Part = Line | Note | Logged | Original


@dataclass
class LogChat:
    """What GenerateItems reads from ChannelData and the session."""

    channel_id: int
    megagroup: bool
    broadcast: bool
    access_hash: int = 0
    anyone_can_add: bool = True
    self_id: int = 0
    link_prefix: str = "https://t.me/"
    users: dict[int, Any] = field(default_factory=dict)
    chats: dict[int, Any] = field(default_factory=dict)

    def name(self, peer: Any) -> str:
        """PeerData::name(): a user's full name, a chat's title."""
        if isinstance(peer, int):
            peer = tl.PeerUser(peer)
        if isinstance(peer, tl.PeerUser):
            user = self.users.get(peer.user_id)
            if user is None or getattr(user, "deleted", False):
                return "Deleted Account"
            return " ".join(p for p in (user.first_name, user.last_name) if p) or (
                "Deleted Account"
            )
        chat_id = getattr(peer, "channel_id", None) or getattr(peer, "chat_id", 0)
        chat = self.chats.get(chat_id)
        return getattr(chat, "title", "") or ""

    def username(self, peer: Any) -> str:
        if isinstance(peer, tl.PeerUser):
            entity = self.users.get(peer.user_id)
        else:
            entity = self.chats.get(getattr(peer, "channel_id", 0))
        if entity is None:
            return ""
        if getattr(entity, "username", None):
            return entity.username
        active = [u.username for u in getattr(entity, "usernames", None) or [] if u.active]
        return active[0] if active else ""


# ChatAdminRight in flag order (tdesktop's std::map), the Telethon field and the phrase key.
_ADMIN_RIGHTS = (
    (("change_info",), "Change info"),
    (("post_messages",), "Post messages"),
    (("edit_messages",), "Edit messages"),
    (("delete_messages",), "Delete messages"),
    (("ban_users",), "Ban users"),
    (("invite_users",), None),  # invitePhrase
    (("pin_messages",), "Pin messages"),
    (("add_admins",), "Add new admins"),
    (("anonymous",), "Remain anonymous"),
    (("manage_call",), None),  # callPhrase
    (("manage_topics",), "Manage topics"),
    (("manage_direct_messages",), "Manage direct messages"),
    (("manage_ranks",), "Edit member tags"),
    (("manage_welcome_messages",), "Manage Welcome Messages"),
)
# ChatRestriction in flag order; stickers, GIFs, games and inline share one phrase.
_RESTRICTIONS = (
    (("view_messages",), "Read messages"),
    (("send_stickers", "send_gifs", "send_games", "send_inline"), "Send stickers & GIFs"),
    (("embed_links",), "Embed links"),
    (("send_polls",), "Send polls"),
    (("change_info",), "Change info"),
    (("invite_users",), "Add users"),
    (("pin_messages",), "Pin messages"),
    (("manage_topics",), "Create topics"),
    (("send_photos",), "Send photos"),
    (("send_videos",), "Send video files"),
    (("send_roundvideos",), "Send video messages"),
    (("send_audios",), "Send music"),
    (("send_voices",), "Send voice messages"),
    (("send_docs",), "Send files"),
    (("send_plain",), "Send messages"),
    (("edit_rank",), None),  # edit_rank / edit_rank_single
    (("send_reactions",), "Send reactions"),
)


def _flags(rights: Any, table: tuple) -> frozenset:
    names = {name for fields, _ in table for name in fields}
    return frozenset(n for n in names if rights is not None and getattr(rights, n, False))


def _collect_changes(table: tuple, plus: frozenset, minus: frozenset) -> str:
    """CollectChanges: "\\n+phrase" for what only `plus` has, then "\\n\u2212phrase"."""
    result = ""
    for flags, prefix in ((plus - minus, "+"), (minus - plus, _MINUS)):
        for fields, phrase in table:
            if flags.intersection(fields):
                result += "\n" + prefix + phrase
    return result


def _admin_change(chat: LogChat, user: str, new: frozenset, prev: frozenset) -> str:
    """GenerateAdminChangeText."""
    invite = "Invite users via link" if chat.megagroup and chat.anyone_can_add else "Add users"
    call = "Manage live streams" if chat.broadcast else "Manage video chats"
    table = tuple(
        (fields, phrase or (invite if fields == ("invite_users",) else call))
        for fields, phrase in _ADMIN_RIGHTS
    )
    changes = _collect_changes(table, new, prev)
    return tr("promoted", user=user) + ("\n" + changes if changes else "")


def _restriction_changes(new: frozenset, prev: frozenset, user_specific: bool) -> str:
    rank = "Edit own tag" if user_specific else "Edit own tags"
    table = tuple((fields, phrase or rank) for fields, phrase in _RESTRICTIONS)
    return _collect_changes(table, prev, new)


def _restricted_forever(until: int) -> bool:
    return not until or until == _RESTRICT_FOREVER


def _permissions_change(peer: Any, user: str, new: tuple, prev: tuple) -> str:
    """GeneratePermissionsChangeText for one participant; `new`/`prev` are (flags, until)."""
    (new_flags, until), (prev_flags, _) = new, prev
    forever = _restricted_forever(until)
    if "view_messages" in new_flags:
        if forever:
            return tr("banned", user=user)
        until_text = tr("restricted_until", date=date_time(until))
        return tr("banned_until", user=user, until=until_text)
    if not new_flags and "view_messages" in prev_flags and not isinstance(peer, tl.PeerUser):
        return tr("unbanned", user=user)
    until_text = (
        tr("restricted_forever") if forever else tr("restricted_until", date=date_time(until))
    )
    result = tr("restricted", user=user, until=until_text)
    changes = _restriction_changes(new_flags, prev_flags, True)
    return result + ("\n" + changes if changes else "")


def participant_string(chat: LogChat, peer: Any) -> str:
    """GenerateParticipantString: "Name (@username)"."""
    name = chat.name(peer)
    username = chat.username(peer)
    return tr("user_with_username", name=name, mention="@" + username) if username else name


@dataclass
class _Participant:
    """Api::ChatParticipant: its type, peer, rights and restrictions."""

    kind: str
    peer: Any
    rights: frozenset = frozenset()
    restrictions: tuple = (frozenset(), 0)
    subscription: int = 0


def _participant(data: Any) -> _Participant:
    if isinstance(data, (tl.ChannelParticipantBanned, tl.ChannelParticipantLeft)):
        peer = data.peer
    else:
        peer = tl.PeerUser(data.user_id)
    if isinstance(data, tl.ChannelParticipantCreator):
        return _Participant("creator", peer, _flags(data.admin_rights, _ADMIN_RIGHTS))
    if isinstance(data, tl.ChannelParticipantAdmin):
        return _Participant("admin", peer, _flags(data.admin_rights, _ADMIN_RIGHTS))
    if isinstance(data, tl.ChannelParticipantBanned):
        rights = data.banned_rights
        flags = _flags(rights, _RESTRICTIONS)
        kind = "banned" if "view_messages" in flags else "restricted"
        return _Participant(kind, peer, restrictions=(flags, to_time(rights.until_date)))
    if isinstance(data, tl.ChannelParticipantLeft):
        return _Participant("left", peer)
    subscription = to_time(getattr(data, "subscription_until_date", None))
    return _Participant("member", peer, subscription=subscription)


def participant_change(chat: LogChat, new_data: Any, old_data: Any = None) -> str:
    """GenerateParticipantChangeText (without the Italic, which the caller adds)."""
    new = _participant(new_data)
    old = _participant(old_data) if old_data is not None else None
    old_rights = old.rights if old else frozenset()
    old_restrictions = old.restrictions if old else (frozenset(), 0)
    user = participant_string(chat, new.peer)
    empty = (frozenset(), 0)
    if new.kind == "creator":
        if isinstance(new.peer, tl.PeerUser) and new.peer.user_id == chat.self_id:
            return _admin_change(chat, user, new.rights, old_rights)
        return tr("transferred", user=user)
    if new.kind == "admin":
        return _admin_change(chat, user, new.rights, old_rights)
    if new.kind in ("restricted", "banned"):
        return _permissions_change(new.peer, user, new.restrictions, old_restrictions)
    if old and old.kind == "admin":
        return _admin_change(chat, user, frozenset(), old_rights)
    if old and (old.kind == "banned" or old.kind == "restricted"):
        return _permissions_change(new.peer, user, empty, old_restrictions)
    return tr("invited", user=user)


def default_rights_change(new: Any, prev: Any) -> str:
    """GenerateDefaultBannedRightsChangeText (without the Italic)."""
    changes = _restriction_changes(_flags(new, _RESTRICTIONS), _flags(prev, _RESTRICTIONS), False)
    return tr("changed_default_permissions") + ("\n" + changes if changes else "")


def invite_link(invite: Any) -> str:
    """ExtractInviteLink."""
    return invite.link if isinstance(invite, tl.ChatInviteExported) else PUBLIC_JOIN_LINK


def _invite_label(invite: Any) -> str:
    if isinstance(invite, tl.ChatInviteExported):
        return invite.title or ""
    return PUBLIC_JOIN_LINK


def invite_link_text(invite: Any) -> str:
    """GenerateInviteLinkText: the link's name, or the link without its scheme and path."""
    label = _invite_label(invite)
    return label or invite_link(invite).replace("https://", "").replace("t.me/joinchat/", "")


def invite_link_change(new: Any, prev: Any) -> str:
    """GenerateInviteLinkChangeText (without the Italic)."""

    def expire(link: Any) -> int:
        return to_time(getattr(link, "expire_date", None))

    def usage(link: Any) -> int:
        return getattr(link, "usage_limit", None) or 0

    def approval(link: Any) -> bool:
        return bool(getattr(link, "request_needed", True))

    def wrap_date(when: int) -> str:
        return date_time(when) if when else tr("group_invite_no_limit")

    def wrap_usage(count: int) -> str:
        return str(count) if count else tr("group_invite_no_limit")

    result = tr("edited_invite_link", link=invite_link_text(new)) + "\n"
    if _invite_label(prev) != _invite_label(new):
        line = tr("invite_link_label", previous=_invite_label(prev), limit=_invite_label(new))
        result += "\n" + line
    if expire(prev) != expire(new):
        line = tr(
            "invite_link_expire_date",
            previous=wrap_date(expire(prev)),
            limit=wrap_date(expire(new)),
        )
        result += "\n" + line
    if usage(prev) != usage(new):
        line = tr(
            "invite_link_usage_limit",
            previous=wrap_usage(usage(prev)),
            limit=wrap_usage(usage(new)),
        )
        result += "\n" + line
    if approval(prev) != approval(new):
        key = "invite_link_request_needed" if approval(new) else "invite_link_request_not_needed"
        result += "\n" + tr(key)
    return result


def format_ttl(seconds: int) -> str:
    """Ui::FormatTTL, the week branch's `int(ttl) % 7` test included."""
    day = 86400
    if seconds < day:
        return plural("hours", seconds // 3600)
    if seconds < day * 7:
        return plural("days", seconds // day)
    if seconds < day * 31:
        days = seconds // day
        weeks = plural("weeks", days // 7)
        return weeks if seconds % 7 == 0 else weeks + " " + plural("days", days % 7)
    if seconds <= day * 31 * 11:
        return plural("months", seconds // (day * 31))
    return plural("years", round(seconds / (day * 365)))


def topic_link(topic: Any) -> str:
    """GenerateTopicLink's text: Data::ForumTopicIconWithTitle, or "Deleted"."""
    if not isinstance(topic, tl.ForumTopic):
        return "Deleted"
    if topic.id == 1:  # ForumTopic::kGeneralId
        return "# " + topic.title
    return (CUSTOM_EMOJI + " " + topic.title) if topic.icon_emoji_id else topic.title


def url_part(text: str) -> list[TextPart]:
    """PrepareText(value, QString()) of one link: the link is found and marked."""
    return [TextPart(type=TextPart.Type.Url, text=text)] if text else []


def link_lines(chat: LogChat, names: list[str]) -> list[TextPart]:
    parts: list[TextPart] = []
    for name in names:
        parts += url_part(chat.link_prefix + name) + [TextPart(text="\n")]
    return parts


def edited_text(message: Any) -> tuple:
    """ExtractEditedText, comparable: the text and its entities."""
    if not isinstance(message, tl.Message):
        return "", []
    return message.message or "", [e.to_dict() for e in message.entities or []]


def can_have_caption(message: Any) -> bool:
    media = getattr(message, "media", None) if isinstance(message, tl.Message) else None
    return isinstance(media, (tl.MessageMediaDocument, tl.MessageMediaPhoto))


def media_id(message: Any) -> int:
    """MediaId: the photo's or document's id."""
    if not can_have_caption(message):
        return 0
    media = message.media
    inner = media.photo if isinstance(media, tl.MessageMediaPhoto) else media.document
    return getattr(inner, "id", 0) if isinstance(inner, (tl.Photo, tl.Document)) else 0
