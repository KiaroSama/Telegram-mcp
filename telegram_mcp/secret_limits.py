"""What a secret chat cannot do, and why -- the one place that knows.

A secret chat is not an ordinary chat with a flag on it. MTProto's encrypted
layer has a CLOSED vocabulary: thirteen actions and ten media types, and
anything outside those lists does not exist there at all, whatever the ordinary
Telegram API offers. That single fact is what makes an honest "you cannot do
this" possible, and it is why the verdicts below are evidence rather than
opinion.

Two limits live here because they are the same kind of claim and fail the same
way -- silently, in the direction of promising too much:

**Which formatting survives** depends on the LAYER the two devices negotiated
for that one chat, so the same message is whole in one chat and thinned in
another. Five kinds have no encrypted entity at all, and blockquote and its
expandable form have one written into every client and then disabled, which is as
deliberate as a drop gets. Four more are gated on the layer.

**Which operations exist** is fixed by the protocol. Every `impossible` verdict
below cites an absent constructor or a measured refusal -- never "not supported",
which is the absence of a reason rather than one.

The rounding direction is the load-bearing decision. A layer this server could
not read is treated as the protocol FLOOR, never as current: reporting a format
as delivered when it was dropped is exactly the failure this module exists to
prevent, while the opposite error costs one redundant re-send.

Evidence: the MTProto secret-chat schema's `DecryptedMessageAction` and
`DecryptedMessageMedia` vocabularies, the layer constants below, and refusals
measured against Telegram on this owner's own accounts.
"""

from typing import Optional

# SecretChatLayer.h. `Current` is what a fully updated pair negotiates; `FLOOR`
# is what the protocol guarantees and what an unknown layer is treated as.
FLOOR_LAYER = 73
NEW_ENTITIES_LAYER = 101
SPOILER_AND_CUSTOM_EMOJI_LAYER = 144
CURRENT_LAYER = SPOILER_AND_CUSTOM_EMOJI_LAYER

__all__ = [
    "CAPABILITIES",
    "CURRENT_LAYER",
    "FLOOR_LAYER",
    "dropped_entities",
    "require_ready_chat",
    "secret_chat_layer",
]


_PROTOCOL_GAP = "the encrypted protocol has no such formatting"
_OLD_APP = "the other side's app is too old to receive it"


# The kinds with no representation at all in the encrypted layer. Blockquote and
# its expandable form reach the layer check in every client and then find the
# push_back commented out, so they belong here rather than among the gated four:
# no layer delivers them.
#
# Keyed on Telethon's entity classes because that is what the sender now parses
# into. Two rows the previous backend had are GONE rather than renamed: media
# timestamp and formatted date were synthesised by that client and have no
# MTProto entity, so they can no longer arrive here to be dropped.
_NEVER_CARRIED = {
    "MessageEntityCashtag": "cashtag",
    "MessageEntityBotCommand": "bot command",
    "MessageEntityPhone": "phone number",
    "MessageEntityBankCard": "bank card number",
    "MessageEntityMentionName": "mention by name",
    "InputMessageEntityMentionName": "mention by name",
    "MessageEntityBlockquote": "blockquote",
}

# Carried, but only once the chat's negotiated layer reaches the floor beside it.
_NEEDS_LAYER = {
    "MessageEntityUnderline": ("underline", NEW_ENTITIES_LAYER),
    "MessageEntityStrike": ("strikethrough", NEW_ENTITIES_LAYER),
    "MessageEntitySpoiler": ("spoiler", SPOILER_AND_CUSTOM_EMOJI_LAYER),
    "MessageEntityCustomEmoji": ("custom emoji", SPOILER_AND_CUSTOM_EMOJI_LAYER),
}

# Carried at every layer including the floor. Listed explicitly rather than
# inferred as "everything else", so a NEW entity type Telegram adds later lands
# in the unknown branch below instead of being waved through as safe.
_ALWAYS_CARRIED = frozenset(
    {
        "MessageEntityMention",
        "MessageEntityHashtag",
        "MessageEntityUrl",
        "MessageEntityEmail",
        "MessageEntityBold",
        "MessageEntityItalic",
        "MessageEntityCode",
        "MessageEntityPre",
        "MessageEntityTextUrl",
    }
)


def _effective_layer(layer: Optional[int]) -> int:
    """The layer to reason with, rounded DOWN when it is not known.

    A chat whose layer could not be read is assumed to carry only what the
    protocol floor guarantees. Assuming current would make every report a
    promise this server cannot keep.
    """
    if not isinstance(layer, int) or isinstance(layer, bool) or layer < FLOOR_LAYER:
        return FLOOR_LAYER
    return layer


def dropped_entities(entities, layer: Optional[int]) -> list:
    """Every format in ``entities`` that a chat at ``layer`` will not carry.

    ``entities`` are Telethon ``MessageEntity*`` objects, exactly as the client's
    own parser returns them. The result is one record per OCCURRENCE, not per
    kind: two spoilers in one message are two losses, and a caller rewriting the
    text needs to know it is not one.

    An empty list is returned when nothing is dropped, so the caller's signal is
    the field's presence rather than its contents.
    """
    effective = _effective_layer(layer)
    dropped = []

    for entity in entities or []:
        type_name = type(entity).__name__
        if type_name in _ALWAYS_CARRIED:
            continue

        if type_name in _NEVER_CARRIED:
            kind = _NEVER_CARRIED[type_name]
            # One class, two published kinds: an expandable blockquote is an
            # ordinary one with a flag, and a caller rewriting the message is
            # rewriting a different thing in each case.
            if type_name == "MessageEntityBlockquote" and getattr(entity, "collapsed", False):
                kind = "expandable blockquote"
            dropped.append({"kind": kind, "reason": _PROTOCOL_GAP})
            continue

        if type_name in _NEEDS_LAYER:
            kind, floor = _NEEDS_LAYER[type_name]
            if effective < floor:
                dropped.append({"kind": kind, "reason": _OLD_APP, "needs_layer": floor})
            continue

        # Not in any of the three tables. Telegram gains entity types and this
        # module does not update itself, so an unrecognised one is reported as
        # unknown rather than assumed safe -- assuming safe is precisely how a
        # newly droppable format becomes a silent loss all over again.
        dropped.append(
            {
                "kind": str(type_name or "unrecognised formatting"),
                "reason": (
                    "unknown to this server, so whether the encrypted protocol carries it "
                    "could not be determined"
                ),
            }
        )

    return dropped


# Every verdict carries its evidence. An `impossible` entry without a concrete
# reason is a bug, not a note: the caller is an agent that will otherwise keep
# trying, and "not supported" tells it nothing it can act on.
CAPABILITIES = [
    {
        "operation": "send text, and text with formatting",
        "verdict": "available",
        "tool": "send_secret_message",
    },
    {
        "operation": "send a photo, video, document, audio, animation, sticker, "
        "video note or voice note",
        "verdict": "available",
        "tool": "send_secret_media",
    },
    {
        "operation": "reply to a message",
        "verdict": "differs",
        "tool": "send_secret_message(reply_to_message_id=...)",
        "note": "the protocol addresses a reply by the wire id of the original, so a "
        "message this device never received cannot be replied to at all.",
    },
    {
        "operation": "delete a message",
        "verdict": "differs",
        "tool": "delete_secret_message",
        "note": "always reaches both sides. The protocol has no delete-for-me-only, so "
        "there is no choice to make.",
    },
    {
        "operation": "clear the whole conversation",
        "verdict": "differs",
        "tool": "clear_secret_history",
        "note": "always reaches both sides and cannot be undone.",
    },
    {
        "operation": "mark messages read",
        "verdict": "differs",
        "tool": "mark_secret_read",
        "note": "addressed by moment rather than by message: everything at or before the "
        "named message's timestamp is marked read.",
    },
    {
        "operation": "show a typing or recording indicator",
        "verdict": "available",
        "tool": "send_secret_typing",
    },
    {
        "operation": "search the conversation",
        "verdict": "differs",
        "tool": "search_secret_messages",
        "note": "searches this device's local copy, which is the only copy. No matches is "
        "not evidence that no such message was ever sent.",
    },
    {
        "operation": "read the history",
        "verdict": "differs",
        "tool": "read_secret_messages",
        "note": "local-only and permanently gappy; a message this login never received is "
        "gone rather than late.",
    },
    {
        "operation": "copy a message in from another chat",
        "verdict": "differs",
        "tool": "copy_into_secret_chat",
        "note": "arrives as a new message with no attribution, because the encrypted "
        "message carries no forwarding information at all.",
    },
    {
        "operation": "make messages self-destruct",
        "verdict": "differs",
        "tool": "set_secret_chat_timer, send_timed_secret_message",
        "note": "the timer belongs to the CHAT and applies to everything sent afterwards, "
        "including text. A per-message timer is refused with 'Messages can "
        "self-destruct only in private chats'.",
    },
    {
        "operation": "edit a message already sent",
        "verdict": "impossible",
        "note": "there is no edit action among the thirteen the encrypted protocol "
        "defines. An ordinary chat edits server-side; a secret chat has no server "
        "copy to edit.",
    },
    {
        "operation": "react to a message",
        "verdict": "impossible",
        "note": "no reaction action exists in the encrypted protocol, and reactions are "
        "stored against a server-side message this chat does not have.",
    },
    {
        "operation": "pin a message",
        "verdict": "impossible",
        "note": "Telegram refuses it outright: 'Secret chats can't have pinned messages'.",
    },
    {
        "operation": "schedule a message for later",
        "verdict": "impossible",
        "note": "Telegram refuses it outright: 'Can't schedule messages in secret chats'. "
        "Scheduling needs a server to hold the message, which is what a secret "
        "chat is designed not to have.",
    },
    {
        "operation": "forward a message out of the chat",
        "verdict": "impossible",
        "note": "Telegram refuses with 'Can't forward content of', and the encrypted "
        "message has no attribution field, so there is nothing to forward WITH.",
    },
    {
        "operation": "send a poll, dice, a game or an invoice",
        "verdict": "impossible",
        "note": "none of these appears among the ten media types the encrypted protocol "
        "defines.",
    },
    {
        "operation": "share a live location",
        "verdict": "impossible",
        "note": "the protocol carries a static geo point only; there is no live variant "
        "among its media types.",
    },
    {
        "operation": "use message threads or comments",
        "verdict": "impossible",
        "note": "the encrypted message carries no thread field.",
    },
    {
        "operation": "see who has read a message",
        "verdict": "impossible",
        "note": "Telegram refuses with 'Can't get message viewers in secret chats'. The "
        "read receipt carries a timestamp and nothing else.",
    },
    {
        "operation": "set a title or photo for the chat",
        "verdict": "impossible",
        "note": "Telegram refuses with 'Can't change secret chat title' and the same for "
        "photo and description. A secret chat is named after its peer.",
    },
    {
        "operation": "block the other person from inside the chat",
        "verdict": "impossible",
        "note": "Telegram refuses with 'The secret chat can't be blocked'. Block the USER "
        "instead, with block_user.",
    },
    {
        "operation": "notify the other side that a screenshot was taken",
        "verdict": "impossible",
        "note": "the protocol defines the action and the encryption backend can send "
        "it, but nothing here takes screenshots, so there is never anything to "
        "notify about. No tool sends it, and one that did would be announcing an "
        "act this server did not perform.",
    },
]


async def secret_chat_layer(manager, chat_id: int) -> int:
    """The layer this one chat negotiated, or the floor when it cannot be read.

    Read at SEND time, never cached from when the chat opened, and that timing is
    the whole point. The layer is the PEER's capability: it starts at the initial
    value and rises only when the peer announces it with a `notifyLayer` service
    message. Measured on a real chat between two of this owner's accounts, the two
    sides reported 46 and 144 for the same ready chat, seconds apart, because only
    one had seen the other's announcement yet.

    A layer still below the floor therefore means NOT YET KNOWN, and rounding it
    down to the floor is what keeps the report honest: a chat that has in fact
    negotiated 144 is told its spoilers may not cross, which costs one redundant
    re-send, while the opposite error tells a caller their custom emoji arrived
    when it did not.

    Never raises for a missing chat: a formatting report is a courtesy attached to
    a send, and it must not be the reason the send fails.
    """
    try:
        return _effective_layer(manager.status(int(chat_id)).layer)
    except Exception:
        # Rounding down on failure, for the same reason as everywhere else here.
        return FLOOR_LAYER


def require_ready_chat(manager, chat_id: int) -> Optional[str]:
    """``None`` when the chat is ready, otherwise a refusal to hand back.

    Applied to every tool that SENDS or MUTATES, and deliberately not to the two
    that read. A closed chat's history still exists on this device, and refusing to
    read it would destroy the last copy's usefulness in order to satisfy a rule
    about sending.

    The point is the error the caller gets. Without this, a send into a chat whose
    key exchange has not finished fails inside the protocol with a message that
    names neither the chat nor the state, and an agent cannot tell "wait a moment"
    from "this will never work".
    """
    try:
        chat = manager.status(int(chat_id))
    except KeyError:
        return (
            f"No secret chat {chat_id} for this login. Secret chats are per-device, so one "
            "that exists on the account's phone is not visible here and never will be. "
            "list_secret_chats shows the ones this login can see."
        )

    state = getattr(chat.state, "value", str(chat.state))
    if state in ("ready", "rekeying"):
        # Rekeying sends. The exchange holds two keys and the message goes out
        # under whichever one settles; refusing here would make a routine key
        # rotation look like an outage.
        return None
    if state in ("requested", "pending"):
        return (
            f"Secret chat {chat_id} is still {state}: the key exchange is not finished, so "
            "nothing can be sent yet. This is not an error to retry immediately - it clears "
            "when the other side opens the chat. list_secret_chats reports the state."
        )
    if state == "closed":
        return (
            f"Secret chat {chat_id} is closed. The key was discarded on both sides, so it "
            "cannot be reopened and nothing can be sent into it - a new chat with the same "
            "person is a different chat. Its history is still readable here with "
            "read_secret_messages."
        )
    return (
        f"Secret chat {chat_id} reports a state this server does not recognise "
        f"({state or 'none'}), so whether it can be sent to is unknown. Nothing was sent."
    )
