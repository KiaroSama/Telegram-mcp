"""What members of a group may do - Telegram Desktop 7.2.10's Permissions screen, in one call.

Desktop's screen (``boxes/peers/edit_peer_permissions_box.cpp``) lists, in this order:
Send messages; "Send media" with ten kinds under it; Add members; Create topics (forums);
Pin messages; Edit own tags; Change group info - then Charge Stars for Messages, Do not
restrict boosters, and Slow mode. ``set_group_permissions`` sets any of them; an item not
passed keeps its current value. ``set_member_exception`` / ``remove_member_exception`` are
Desktop's Exceptions: one member's own restrictions. Removed members stay with
``get_banned_users`` / ``unban_user``.

``ChatBannedRights`` has the inverted sense: a ``True`` field means *restricted*. The
arguments here say what is *allowed*, as Desktop's checkboxes do.

Two pure functions serve callers outside the tools: ``permissions_line(arguments)`` renders
what a call sets, for the safeguard approval, and ``refusal(arguments)`` answers from the
arguments alone why a call cannot run.
"""

import copy
from typing import Any, Dict, Optional

from telegram_mcp import approval_details, preflight
from telegram_mcp.runtime import *
from telegram_mcp.safe_log import log_event

__all__ = ["set_group_permissions", "set_member_exception", "remove_member_exception"]

# (argument, Desktop label, ChatBannedRights fields), Desktop's order
# (NestedRestrictionLabelsList). "Stickers & GIFs" is one checkbox over four fields.
SEND_MESSAGES = ("send_messages", "Send messages", ("send_plain",))
MEDIA = (
    ("photos", "Photos", ("send_photos",)),
    ("video_files", "Video files", ("send_videos",)),
    ("video_messages", "Video messages", ("send_roundvideos",)),
    ("music", "Music", ("send_audios",)),
    ("voice_messages", "Voice messages", ("send_voices",)),
    ("files", "Files", ("send_docs",)),
    (
        "stickers_and_gifs",
        "Stickers & GIFs",
        ("send_stickers", "send_gifs", "send_games", "send_inline"),
    ),
    ("embed_links", "Embed links", ("embed_links",)),
    ("polls", "Polls", ("send_polls",)),
    ("reactions", "Reactions", ("send_reactions",)),
)
OTHERS = (
    ("add_members", "Add members", ("invite_users",)),
    ("create_topics", "Create topics", ("manage_topics",)),
    ("pin_messages", "Pin messages", ("pin_messages",)),
    ("edit_own_tags", "Edit own tags", ("edit_rank",)),
    ("change_group_info", "Change group info", ("change_info",)),
)
ITEMS = (SEND_MESSAGES, *MEDIA, *OTHERS)
LABELS = {key: label for key, label, _ in ITEMS}

# SlowmodeDelayByIndex: Off, 5s, 10s, 30s, 1m, 5m, 15m, 1h.
SLOW_MODE = {"off": 0, "5s": 5, "10s": 10, "30s": 30, "1m": 60, "5m": 300, "15m": 900, "1h": 3600}
MAX_BOOSTS = 5  # BoostsUnrestrictByIndex: a slider of 1..5 boosts

_PUBLIC = "This permission is not available in public groups."
_DISCUSSION = "This permission is not available in discussion groups."
_FOR_ALL = "This option is disabled for all members in Group Permissions."
_SUPERGROUP = "exists only in supergroups; upgrade_to_supergroup converts this group first."


def _requested(arguments: Dict[str, Any]) -> Dict[str, bool]:
    """Item -> allowed, for the items the call names; ``send_media`` covers the ten kinds."""
    wanted = {key: arguments[key] for key in LABELS if isinstance(arguments.get(key), bool)}
    parent = arguments.get("send_media")
    if isinstance(parent, bool):
        for key, _label, _fields in MEDIA:
            wanted.setdefault(key, parent)
    return wanted


def _slow_mode_seconds(value: Any) -> Optional[int]:
    if isinstance(value, str) and value.strip().lower() in SLOW_MODE:
        return SLOW_MODE[value.strip().lower()]
    if isinstance(value, int) and not isinstance(value, bool) and value in SLOW_MODE.values():
        return value
    return None


def _whole(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def refusal(arguments: Dict[str, Any]) -> Optional[str]:
    """Why these arguments cannot run, from the arguments alone; None when they can."""
    for key in (*LABELS, "send_media"):
        value = arguments.get(key)
        if value is not None and not isinstance(value, bool):
            return f"{key} must be true (allowed) or false (not allowed), not {value!r}."
    wanted = _requested(arguments)
    if wanted.get("embed_links") is True and wanted.get("send_messages") is False:
        return "Embed links cannot be allowed while Send messages is not (Desktop couples them)."
    slow = arguments.get("slow_mode")
    if slow is not None and _slow_mode_seconds(slow) is None:
        return f"slow_mode must be one of Desktop's values {list(SLOW_MODE)}, not {slow!r}."
    stars = arguments.get("charge_stars_per_message")
    if stars is not None and (not _whole(stars) or stars < 0):
        return f"charge_stars_per_message must be a whole number, 0 = off, not {stars!r}."
    boosts = arguments.get("do_not_restrict_boosters")
    if boosts is not None and (not _whole(boosts) or not 0 <= boosts <= MAX_BOOSTS):
        return f"do_not_restrict_boosters must be 0 (off) to {MAX_BOOSTS} boosts, not {boosts!r}."
    until = arguments.get("until_date")
    if until is not None and (not _whole(until) or until < 0):
        return f"until_date must be a Unix time, 0 = forever, not {until!r}."
    return None


def _render(keys) -> list:
    """Desktop's words: top-level items, the media kinds after "Send media: "."""
    keys = set(keys)
    media = [label for key, label, _ in MEDIA if key in keys]
    parts = [LABELS["send_messages"]] if "send_messages" in keys else []
    if media:
        parts.append("Send media: " + ", ".join(media))
    return parts + [label for key, label, _ in OTHERS if key in keys]


def permissions_line(arguments: Dict[str, Any]) -> str:
    """The approval line for set_group_permissions / set_member_exception.

    ``permissions: <allowed> | <group settings>; not allowed: <restricted>``, top-level items
    joined " | ", media kinds after "Send media: " joined ", ", Desktop's order and words.
    """
    member = "user_id" in arguments
    wanted = _requested(arguments)
    if wanted.get("send_messages") is False:
        wanted["embed_links"] = False
    allowed = _render(k for k, v in wanted.items() if v)
    denied = _render(k for k, v in wanted.items() if not v)
    if member:
        allowed = [p.replace("Edit own tags", "Edit own tag") for p in allowed]
        denied = [p.replace("Edit own tags", "Edit own tag") for p in denied]
        until = arguments.get("until_date")
        if until is not None:
            allowed.append("forever" if not until else f"until {until}")
    else:
        stars = arguments.get("charge_stars_per_message")
        if stars is not None:
            allowed.append(f"Charge Stars for Messages: {stars or 'off'}")
        boosts = arguments.get("do_not_restrict_boosters")
        if boosts is not None:
            allowed.append(f"Do not restrict boosters: {boosts or 'off'}")
        slow = arguments.get("slow_mode")
        if slow is not None:
            seconds = _slow_mode_seconds(slow)
            names = {v: k for k, v in SLOW_MODE.items()}
            shown = "Off" if seconds == 0 else names.get(seconds, str(slow))
            allowed.append(f"Slow mode: {shown}")
    segments = []
    if allowed:
        segments.append(" | ".join(allowed))
    if denied:
        segments.append("not allowed: " + " | ".join(denied))
    return "permissions: " + ("; ".join(segments) or "nothing changes")


def _allowed_from(rights) -> Dict[str, bool]:
    """Desktop's checkboxes for a ChatBannedRights (None = nothing restricted)."""
    media_keys = {key for key, _label, _fields in MEDIA}
    return {
        key: (
            not (
                getattr(rights, "view_messages", False)
                or any(getattr(rights, f, False) for f in fields)
                or (
                    (key == "send_messages" or key in media_keys)
                    and getattr(rights, "send_messages", False)
                )
                or (key in media_keys and getattr(rights, "send_media", False))
            )
            if rights
            else True
        )
        for key, _label, fields in ITEMS
    }


def _fixed(allowed: Dict[str, bool]) -> Dict[str, bool]:
    """FixDependentRestrictions: no embed links without send messages."""
    return dict(allowed, embed_links=False) if not allowed["send_messages"] else allowed


def _banned_rights(allowed: Dict[str, bool], until_date: int = 0) -> ChatBannedRights:
    """Desktop never sends view_messages, send_messages or send_media here."""
    fields = {f: True for key, _label, fs in ITEMS if not allowed.get(key, True) for f in fs}
    return ChatBannedRights(until_date=until_date or None, **fields)


def _group_refusal(entity, wanted: Dict[str, bool]) -> Optional[str]:
    """What Desktop disables on this group, for the items the call wants to allow."""
    if isinstance(entity, Channel) and not getattr(entity, "megagroup", False):
        return "A broadcast channel has no member permissions; this is for groups."
    if "create_topics" in wanted and not getattr(entity, "forum", False):
        return "Create topics exists only in forum groups."
    locked = [k for k in ("pin_messages", "change_group_info") if wanted.get(k)]
    if locked and getattr(entity, "username", None):
        return f"{', '.join(LABELS[k] for k in locked)}: {_PUBLIC}"
    if locked and getattr(entity, "megagroup", False) and getattr(entity, "has_link", False):
        return f"{', '.join(LABELS[k] for k in locked)}: {_DISCUSSION}"
    return None


async def _step(label: str, cl, request, applied: list) -> Optional[str]:
    """Send one request; on failure say what was applied before it and what was not."""
    try:
        await cl(request)
    except telethon.errors.rpcerrorlist.ChatNotModifiedError:
        pass
    except Exception as e:
        log_event(logging.ERROR, "group permission step failed", step=label, error=e)
        done = ", ".join(applied) or "nothing"
        return f"{label} was not applied ({type(e).__name__}). Requests accepted before it: {done}. Final state was not verified."
    applied.append(label)
    return None


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Group Permissions",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_group_permissions(
    chat_id: Union[int, str],
    send_messages: Optional[bool] = None,
    send_media: Optional[bool] = None,
    photos: Optional[bool] = None,
    video_files: Optional[bool] = None,
    video_messages: Optional[bool] = None,
    music: Optional[bool] = None,
    voice_messages: Optional[bool] = None,
    files: Optional[bool] = None,
    stickers_and_gifs: Optional[bool] = None,
    embed_links: Optional[bool] = None,
    polls: Optional[bool] = None,
    reactions: Optional[bool] = None,
    add_members: Optional[bool] = None,
    create_topics: Optional[bool] = None,
    pin_messages: Optional[bool] = None,
    edit_own_tags: Optional[bool] = None,
    change_group_info: Optional[bool] = None,
    charge_stars_per_message: Optional[int] = None,
    do_not_restrict_boosters: Optional[int] = None,
    slow_mode: Optional[Union[str, int]] = None,
    account: str = None,
) -> str:
    """
    Set what members of a group may do - Telegram Desktop's Permissions screen.

    Every item is optional; one not passed keeps its current value. true = allowed.

    Args:
        chat_id: The group.
        send_messages: Send messages (off also turns Embed links off, as in Desktop).
        send_media: All ten media kinds at once; a kind passed by name wins over it.
        photos, video_files, video_messages, music, voice_messages, files,
            stickers_and_gifs, embed_links, polls, reactions: the ten "Send media" kinds.
            Stickers & GIFs also covers games and inline bots.
        add_members: Add members.
        create_topics: Create topics (forum groups only).
        pin_messages: Pin messages (never in public or discussion groups).
        edit_own_tags: Edit own tags.
        change_group_info: Change group info (never in public or discussion groups).
        charge_stars_per_message: Charge Stars for Messages: the price, 0 = off. Telegram
            decides whether the group is eligible and the highest price.
        do_not_restrict_boosters: Boosts (1-5) that exempt a member from the restrictions
            and slow mode; 0 = off. Only when something is restricted or slow mode is on.
        slow_mode: off, 5s, 10s, 30s, 1m, 5m, 15m or 1h (or those seconds).

    Stars, boosters and slow mode need a supergroup.
    """
    arguments = dict(locals())
    try:
        reason = refusal(arguments)
        if reason:
            return f"Error: {reason}"
        wanted = _requested(arguments)
        settings = {
            k: arguments[k]
            for k in ("charge_stars_per_message", "do_not_restrict_boosters", "slow_mode")
            if arguments[k] is not None
        }
        if not wanted and not settings:
            return "Nothing to change: pass at least one permission or setting."

        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)
        reason = _group_refusal(entity, wanted)
        if reason:
            return f"Error: {reason}"
        supergroup = isinstance(entity, Channel)
        seconds = _slow_mode_seconds(slow_mode) if slow_mode is not None else None
        switched_on = [
            k for k, v in settings.items() if (seconds if k == "slow_mode" else v)
        ]  # a basic group has none of these, so "off" there is already true
        if not supergroup and switched_on:
            return f"Error: {', '.join(switched_on)} {_SUPERGROUP}"

        allowed = _fixed(
            dict(_allowed_from(getattr(entity, "default_banned_rights", None)), **wanted)
        )
        if not getattr(entity, "forum", False):
            allowed.pop("create_topics")
        rights = _banned_rights(allowed)
        slow_on = (
            seconds > 0
            if seconds is not None
            else bool(getattr(entity, "slowmode_enabled", False))
        )
        restricted = any(v for k, v in rights.to_dict().items() if k != "until_date" and v is True)
        if do_not_restrict_boosters and not (restricted or slow_on):
            return (
                "Error: Do not restrict boosters applies only while something is restricted "
                "or slow mode is on."
            )

        title = sanitize_name(getattr(entity, "title", str(chat_id)))
        steps = []
        if wanted:
            steps.append(
                (
                    "Permissions",
                    functions.messages.EditChatDefaultBannedRightsRequest(
                        peer=entity, banned_rights=rights
                    ),
                )
            )
        if seconds is not None and supergroup:
            steps.append(
                (
                    "Slow mode",
                    functions.channels.ToggleSlowModeRequest(channel=entity, seconds=seconds),
                )
            )
        if do_not_restrict_boosters is not None and supergroup:
            steps.append(
                (
                    "Do not restrict boosters",
                    functions.channels.SetBoostsToUnblockRestrictionsRequest(
                        channel=entity, boosts=do_not_restrict_boosters
                    ),
                )
            )
        if charge_stars_per_message is not None and supergroup:
            steps.append(
                (
                    "Charge Stars for Messages",
                    functions.channels.UpdatePaidMessagesPriceRequest(
                        channel=entity, send_paid_messages_stars=charge_stars_per_message
                    ),
                )
            )
        applied = []
        for label, request in steps:
            failed = await _step(label, cl, request, applied)
            if failed:
                return f"{title}: {failed}"
        now = permissions_line(dict(allowed, **settings))
        return (
            f"{title}: requests accepted for {', '.join(applied)}. "
            f"Requested permissions: {now.removeprefix('permissions: ')}. "
            "Final state was not verified."
        )
    except Exception as e:
        return log_and_format_error("set_group_permissions", e, chat_id=chat_id)


async def _member(chat_id, user_id, cl):
    """(group, user, participant) or a refusal: exceptions exist in supergroups only."""
    entity = await resolve_entity(chat_id, cl)
    if not isinstance(entity, Channel):
        return None, None, None, f"Error: member exceptions {_SUPERGROUP}"
    if not getattr(entity, "megagroup", False):
        return None, None, None, "Error: a broadcast channel has no member exceptions."
    user = await resolve_entity(user_id, cl)
    result = await cl(functions.channels.GetParticipantRequest(channel=entity, participant=user))
    participant = result.participant
    rights = getattr(participant, "banned_rights", None)
    if rights is not None and getattr(rights, "view_messages", False):
        return (
            None,
            None,
            None,
            (
                f"Error: {user_id} is removed from the group, not restricted; "
                "unban_user lets them back."
            ),
        )
    return entity, user, participant, None


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Member Exception",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id", "user_id")
async def set_member_exception(
    chat_id: Union[int, str],
    user_id: Union[int, str],
    send_messages: Optional[bool] = None,
    send_media: Optional[bool] = None,
    photos: Optional[bool] = None,
    video_files: Optional[bool] = None,
    video_messages: Optional[bool] = None,
    music: Optional[bool] = None,
    voice_messages: Optional[bool] = None,
    files: Optional[bool] = None,
    stickers_and_gifs: Optional[bool] = None,
    embed_links: Optional[bool] = None,
    polls: Optional[bool] = None,
    reactions: Optional[bool] = None,
    add_members: Optional[bool] = None,
    create_topics: Optional[bool] = None,
    pin_messages: Optional[bool] = None,
    edit_own_tags: Optional[bool] = None,
    change_group_info: Optional[bool] = None,
    until_date: Optional[int] = None,
    account: str = None,
) -> str:
    """
    Give one member their own permissions - Telegram Desktop's Exceptions.

    Starts from the member's current exception, or the group's permissions when there is
    none; an item not passed keeps that value. true = allowed. Nothing the group forbids
    for everyone can be allowed here. Supergroups only.

    Args:
        chat_id: The group.
        user_id: The member.
        send_messages ... change_group_info: as in set_group_permissions ("Edit own tag").
        until_date: Restricted until, as a Unix time; 0 = forever; not passed = keep the
            member's current end date (forever for a new exception).
    """
    arguments = dict(locals())
    try:
        reason = refusal(arguments)
        if reason:
            return f"Error: {reason}"
        wanted = _requested(arguments)
        if not wanted and until_date is None:
            return "Nothing to change: pass at least one permission or until_date."

        cl = get_client(account)
        await ensure_connected(cl)
        entity, user, participant, reason = await _member(chat_id, user_id, cl)
        if reason:
            return reason
        reason = _group_refusal(entity, wanted)
        if reason:
            return f"Error: {reason}"
        group = _fixed(_allowed_from(getattr(entity, "default_banned_rights", None)))
        forbidden = [k for k, v in wanted.items() if v and not group[k]]
        if forbidden:
            return f"Error: {', '.join(LABELS[k] for k in forbidden)}: {_FOR_ALL}"

        own = getattr(participant, "banned_rights", None)
        if not wanted:
            if own is None:
                return "Nothing to change: this member has no exception to expire."
            # An expiry edit must not make inherited group bans personal restrictions.
            banned = copy.copy(own)
            banned.until_date = until_date or None
            allowed = _allowed_from(own)
        else:
            start = _allowed_from(own) if own is not None else group
            if until_date is None:  # editing must not make a temporary one permanent
                until_date = getattr(own, "until_date", None) or 0
            allowed = _fixed({k: v and group[k] for k, v in dict(start, **wanted).items()})
            if not getattr(entity, "forum", False):
                allowed.pop("create_topics")
            banned = _banned_rights(allowed, until_date)
        await cl(
            functions.channels.EditBannedRequest(
                channel=entity, participant=user, banned_rights=banned
            )
        )
        now = permissions_line(dict(allowed, user_id=user_id)).removeprefix("permissions: ")
        until = "forever" if not until_date else f"until {until_date}"
        return f"Exception request accepted for {user_id} ({until}): {now}. Final state was not verified."
    except Exception as e:
        return log_and_format_error("set_member_exception", e, chat_id=chat_id, user_id=user_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Remove Member Exception",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id", "user_id")
async def remove_member_exception(
    chat_id: Union[int, str], user_id: Union[int, str], account: str = None
) -> str:
    """
    Remove one member's exception, so the group's permissions apply to them again.

    A removed (banned) member is not an exception: unban_user lets them back.

    Args:
        chat_id: The group.
        user_id: The member.
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        entity, user, participant, reason = await _member(chat_id, user_id, cl)
        if reason:
            return reason
        if getattr(participant, "banned_rights", None) is None:
            return f"{user_id} has no exception; the group's permissions already apply."
        await cl(
            functions.channels.EditBannedRequest(
                channel=entity, participant=user, banned_rights=ChatBannedRights(until_date=None)
            )
        )
        return f"Exception removal request accepted for {user_id}. Final state was not verified."
    except Exception as e:
        return log_and_format_error("remove_member_exception", e, chat_id=chat_id, user_id=user_id)


# The approval names what is about to be set (spec 033 FR-005), and a value these tools
# refuse by themselves is refused before the owner is asked (preflight).
approval_details.register("set_group_permissions", permissions_line)
approval_details.register("set_member_exception", permissions_line)
approval_details.register(
    "remove_member_exception",
    lambda arguments: "permissions: the member's exception is removed; the group's permissions "
    "apply again",
)
preflight.RULES["set_group_permissions"] = refusal
preflight.RULES["set_member_exception"] = refusal
