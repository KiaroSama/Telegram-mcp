"""What a media kind means to the ordinary send path.

`media_kinds` says which kinds exist and which files may take them, and names no
backend. This says what each one becomes on the wire for an ORDINARY chat, which is
Telethon's business - the same question has a different answer in
`secret_media_content.py` for TDLib and in the Telethon Secret Chat package for the
encrypted protocol.

Every flag below already existed on `send_file`. None was ever passed: this server
handed Telethon a path and let the extension decide, so the operator could not ask
for a voice note, could not ask for a round video note, and could not ask for a
`.jpg` to go as a file - the three things their own client offers on every drag.

It is a module of its own because `tools/media.py` stood at 789 lines when this was
written, and the project closes a file to new code at about 700.
"""

from telegram_mcp.media_kinds import KINDS, NO_CAPTION, family_of, infer_kind

__all__ = ["MediaKindError", "flags_for", "group_sends", "resolve_kind"]

# kind -> the Telethon `send_file` keywords that make it that kind.
#
# `force_document` is stated for the compressed kinds too, though False is the
# library's default: one request then carries the caller's choice either way, and
# the pair "compressed or as a file" is the choice the operator actually makes.
#
# `animation` has no flag. A video with no audio track is displayed as an animated
# GIF already - Telethon's own `nosound_video` documentation says exactly that - so
# a flag here would name behaviour that happens without it.
_FLAGS = {
    "photo": {"force_document": False},
    "video": {"force_document": False, "supports_streaming": True},
    "document": {"force_document": True},
    "audio": {"force_document": False},
    "animation": {"force_document": False},
    "sticker": {"force_document": False},
    "video_note": {"video_note": True},
    "voice_note": {"voice_note": True},
}


class MediaKindError(ValueError):
    """A kind that cannot be, said in words the caller can act on.

    A type of its own rather than a bare ValueError because the tool layer has to
    hand this text back verbatim while still redacting every other exception. The
    refusals name the file and the kind on purpose; `mcp_errors.log` and an error
    code do not, and a caller who cannot see which file was wrong cannot fix it.
    """


def flags_for(kind: str) -> dict:
    """The keywords that make one send arrive as ``kind``.

    A copy each time: the caller merges these into a call it is building, and a
    shared dict would let one send's extra keyword leak into the next.
    """
    return dict(_FLAGS[kind])


def resolve_kind(file_name: str, kind, caption: str = "") -> str:
    """Settle the kind, or refuse - and refuse before a byte is uploaded.

    ``None`` infers from the name. A named kind is checked against what the file
    can be, and an unrecognised extension is not proof of anything, so it passes:
    the check exists to catch a file that demonstrably cannot be what was asked
    for, not to require an extension this server happens to know.

    Refusals happen here rather than at Telegram because Telegram refuses them
    only after the bytes have crossed and answers with an error naming neither the
    file nor the kind.
    """
    if kind is None:
        kind = infer_kind(file_name)
    elif kind not in KINDS:
        raise MediaKindError(
            f"'{kind}' is not a media kind. Use one of: {', '.join(KINDS)}. "
            "Leave kind unset to have it chosen from the file. Nothing was sent."
        )

    family = family_of(file_name)
    if family is not None and kind not in family["allows"]:
        raise MediaKindError(
            f"{file_name} cannot be sent as {kind}: it is {family['name']} content, "
            f"which Telegram accepts as {', '.join(sorted(family['allows']))}. "
            "Nothing was sent, and nothing was converted."
        )

    if caption and kind in NO_CAPTION:
        raise MediaKindError(
            f"A {kind} carries no caption - the protocol has no field a client would "
            f"show one in, so the text given with {file_name} would be dropped in "
            "transit with no error. Send it as its own message. Nothing was sent."
        )

    return kind


# Which kinds may share ONE Telegram media group, and which travel alone.
#
# `force_document` is a property of the group, not of a file in it, so "send this
# one as a file and that one compressed" is two messages or it is a lie. Telegram
# groups compressed photos and videos together, files with files and tracks with
# tracks; the three missing names below have no media group in the protocol at
# all, so each is always its own message.
_GROUPS = {
    "photo": "media",
    "video": "media",
    "animation": "media",
    "document": "file",
    "audio": "track",
}


def group_sends(kinds):
    """Split one request into the messages Telegram will actually accept.

    Takes the kinds in the order the caller wrote them and returns one
    ``(indices, flags)`` pair per message. Consecutive entries that can share a
    group do; anything else starts a new message, and the order is never
    rearranged to make fewer messages - the operator chose that order.

    The flags are merged across the group, which is safe because only kinds that
    share a group are ever put together and those agree on ``force_document`` by
    construction; ``supports_streaming`` is read per document, so a photo beside
    a video is unharmed by it.
    """
    messages = []
    for index, kind in enumerate(kinds):
        group = _GROUPS.get(kind)
        if messages and group is not None and messages[-1][0] == group:
            messages[-1][1].append(index)
        else:
            messages.append([group, [index]])

    planned = []
    for _, indices in messages:
        flags = {}
        for index in indices:
            flags.update(_FLAGS[kinds[index]])
        planned.append((indices, flags))
    return planned
