"""Which admin rights each chat type has, in Telegram Desktop's words and order (spec 033).

Pure data and functions, no Telegram calls. The source is `NestedAdminRightLabels` in
Telegram Desktop 7.2.10 (`boxes/peers/edit_peer_permissions_box.cpp`) and the English
strings of its `lang.strings`; `.ai/RESEARCH/admin-rights-sets.md` holds the digest.

Three chat types, three different screens: a group, a channel and a community do not share
one list, which is how an agent once sent every right to a channel and Telegram answered
RIGHT_FORBIDDEN. Each right is named by its Telethon ``ChatAdminRights`` field.

Desktop's "Process join requests" has no field at layer 229 (it is not on the wire in
Desktop either), so it is reported as unavailable and never faked.
"""

import inspect
from typing import Dict, NamedTuple, Optional, Tuple

from telethon.tl.types import ChatAdminRights

__all__ = [
    "KINDS",
    "Right",
    "Section",
    "effective_rights",
    "fields",
    "full_admin",
    "permissions_line",
    "sections",
    "to_telethon",
    "validate",
]

KINDS = ("group", "channel", "community")

# Desktop lists it (when the chat allows join requests) but the wire format has no bit for it.
UNAVAILABLE = {"process_join_requests": "Process join requests"}


class Right(NamedTuple):
    field: str
    label: str


class Section(NamedTuple):
    """Rights shown together: under a parent row ("Manage stories"), or at the top level."""

    parent: Optional[str]
    rights: Tuple[Right, ...]


def _stories() -> Section:
    return Section(
        "Manage stories",
        (
            Right("post_stories", "Post stories"),
            Right("edit_stories", "Edit stories of others"),
            Right("delete_stories", "Delete stories of others"),
        ),
    )


def _welcome(is_bot: bool) -> Right:
    # Desktop words it by who is being promoted: a bot "sends", a person "manages".
    label = "Send Welcome Messages" if is_bot else "Manage Welcome Messages"
    return Right("manage_welcome_messages", label)


def _group(is_forum: bool, is_bot: bool, anyone_can_add: bool) -> Tuple[Section, ...]:
    invite = "Invite users via link" if anyone_can_add else "Add members"
    first = [
        Right("change_info", "Change group info"),
        _welcome(is_bot),
        Right("delete_messages", "Delete messages"),
        Right("ban_users", "Ban users"),
        Right("invite_users", invite),
        Right("manage_topics", "Manage topics"),
        Right("pin_messages", "Pin messages"),
    ]
    if not is_forum:  # topics exist only in forum groups
        first = [right for right in first if right.field != "manage_topics"]
    second = (
        Right("manage_call", "Manage video chats"),
        Right("manage_ranks", "Edit member tags"),
        Right("anonymous", "Remain anonymous"),
        Right("add_admins", "Add new admins"),
    )
    return (Section(None, tuple(first)), _stories(), Section(None, second))


def _channel(is_bot: bool) -> Tuple[Section, ...]:
    return (
        Section(None, (Right("change_info", "Change channel info"), _welcome(is_bot))),
        Section(
            "Manage messages",
            (
                Right("post_messages", "Post messages"),
                Right("edit_messages", "Edit messages of others"),
                Right("delete_messages", "Delete messages of others"),
            ),
        ),
        _stories(),
        Section(
            None,
            (
                Right("invite_users", "Add members"),
                Right("manage_call", "Manage live streams"),
                Right("manage_direct_messages", "Manage direct messages"),
                Right("add_admins", "Add new admins"),
                Right("ban_users", "Ban users"),
            ),
        ),
    )


def _community() -> Tuple[Section, ...]:
    return (
        Section(
            None,
            (
                Right("change_info", "Edit Community Name"),
                Right("manage_linked_peers", "Edit Group List"),
                Right("ban_users", "Ban Members"),
                Right("add_admins", "Add new admins"),
            ),
        ),
    )


def sections(
    kind: str,
    is_forum: bool = False,
    is_bot: bool = False,
    anyone_can_add: bool = True,
) -> Tuple[Section, ...]:
    """Desktop's rows for one chat type, in order. ``kind``: group, channel or community."""
    if kind == "group":
        return _group(is_forum, is_bot, anyone_can_add)
    if kind == "channel":
        return _channel(is_bot)
    if kind == "community":
        return _community()
    raise ValueError(f"unknown chat type {kind!r}; expected one of {', '.join(KINDS)}")


def fields(kind: str, is_forum: bool = False) -> Tuple[str, ...]:
    return tuple(r.field for s in sections(kind, is_forum=is_forum) for r in s.rights)


def full_admin(kind: str, is_forum: bool = False) -> Dict[str, bool]:
    """Every right of the type, except "Remain anonymous" (the owner's definition)."""
    return {name: name != "anonymous" for name in fields(kind, is_forum)}


def effective_rights(
    kind: str, given: Dict[str, Optional[bool]], is_forum: bool = False, exact: bool = False
) -> Dict[str, bool]:
    """What a call grants. A right left as None is "not mentioned".

    Promoting (``exact=False``) starts from a full admin and applies what was named, so
    declining one right does not silently decline the rest. Editing (``exact=True``) grants
    exactly what was named True and nothing else.
    """
    rights = (
        {name: False for name in fields(kind, is_forum)} if exact else full_admin(kind, is_forum)
    )
    for name, value in given.items():
        if value is not None and name in rights:
            rights[name] = bool(value)
    return rights


def _labels_of(field: str) -> Dict[str, str]:
    """``{kind: label}`` for every chat type that has the right."""
    found = {}
    for kind in KINDS:
        for section in sections(kind, is_forum=True):
            for right in section.rights:
                if right.field == field:
                    found[kind] = right.label
    return found


def validate(
    kind: str, rights: Dict[str, Optional[bool]], is_forum: bool = False
) -> Optional[str]:
    """A refusal text naming the foreign right and its chat type, or None when all fit."""
    own = fields(kind, is_forum=True)
    for name, value in rights.items():
        if value is None:
            continue
        if name in own:
            if name == "manage_topics" and not is_forum:
                return (
                    "manage_topics (Manage topics) applies only to forum groups; this group "
                    "is not a forum. Enable topics first (enable_forum_topics), or leave it out."
                )
            continue
        if name in UNAVAILABLE:
            return (
                f"{name} ({UNAVAILABLE[name]}) is shown by Telegram Desktop but is not "
                "available at this Telegram layer; leave it out."
            )
        owners = _labels_of(name)
        if owners:
            label = next(iter(owners.values()))
            where = " and ".join(owners)
            return (
                f"{name} ({label}) is not an admin right of a {kind}; only a {where} has it. "
                f"A {kind} offers: {', '.join(own)}."
            )
        return (
            f"{name} is not an admin right Telegram Desktop offers. "
            f"A {kind} offers: {', '.join(own)}."
        )
    return None


def permissions_line(
    kind: str,
    rights: Dict[str, bool],
    is_forum: bool = False,
    is_bot: bool = False,
    anyone_can_add: bool = True,
) -> str:
    """``permissions: A | B | Parent: sub1, sub2 | C``: granted rights only, Desktop's order."""
    items = []
    for section in sections(kind, is_forum, is_bot, anyone_can_add):
        granted = [r.label for r in section.rights if rights.get(r.field)]
        if not granted:
            continue
        if section.parent:
            items.append(f"{section.parent}: {', '.join(granted)}")
        else:
            items.extend(granted)
    return "permissions: " + (" | ".join(items) if items else "none")


def telethon_fields() -> Tuple[str, ...]:
    """Every right on the installed ``ChatAdminRights``, read off the type."""
    return tuple(n for n in inspect.signature(ChatAdminRights.__init__).parameters if n != "self")


def to_telethon(rights: Dict[str, bool]) -> ChatAdminRights:
    """The object Telegram receives: every field explicit, a right not of this type off."""
    return ChatAdminRights(**{name: bool(rights.get(name, False)) for name in telethon_fields()})
