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

import mimetypes

from telethon.tl.types import DocumentAttributeAudio, DocumentAttributeVideo

from telegram_mcp import ogg_tags, video_dims
from telegram_mcp.media_kinds import FAMILIES, KINDS, NO_CAPTION, family_of, infer_kind

__all__ = ["MediaKindError", "caption_flags", "flags_for", "group_sends", "resolve_kind"]

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


#: What an `.ogg` must claim to be, to arrive as a track rather than a voice note.
#:
#: Telegram picks the renderer from the MIME TYPE, not from the container and not
#: from `DocumentAttributeAudio.voice`. `mimetypes` guesses `audio/ogg` from the
#: extension, and `audio/ogg` is Telegram's VOICE type: a file sent with it is a
#: voice bubble however the attribute is set. Measured 2026-09-20, and corrected
#: from an earlier conclusion that blamed the container - the owner produced a
#: real `.ogg` in the wild that arrives as a track, and its only difference is
#: `mime_type: audio/vorbis`.
_TRACK_MIME = "audio/vorbis"
_VOICE_MIME = "audio/ogg"

#: The kinds that travel as a video document, and so need a size stated.
_VIDEO_KINDS = ("video", "video_note", "animation")


def flags_for(kind: str, header: bytes = b"", file_name: str = "", tail: bytes = b"") -> dict:
    """The keywords that make one send arrive as ``kind``.

    A copy each time: the caller merges these into a call it is building, and a
    shared dict would let one send's extra keyword leak into the next.

    ``audio`` is the one kind with no flag behind it. Telethon has
    ``voice_note=True`` and ``video_note=True`` and force-creates those
    attributes itself, but for a track it builds ``DocumentAttributeAudio`` ONLY
    when it can read the file's metadata - and with no metadata reader installed
    it builds nothing, so the send stated nothing and Telegram guessed from the
    mime type. Measured on real Telegram 2026-09-20: the same Opus recording
    arrived as a voice message, and a tagged one as a document, both while the
    reply said "as audio". So the attribute is built here instead.

    ``header`` is the head of the file, when the caller has it. It only ever
    refines a duration; without it the track is still a track.
    """
    flags = dict(_FLAGS[kind])
    if kind == "audio":
        # The extension says voice; the caller asked for a track. Telegram reads
        # the MIME, so saying `audio/ogg` here would be asking for one thing and
        # getting the other.
        if file_name and mimetypes.guess_type(file_name)[0] == _VOICE_MIME:
            flags["mime_type"] = _TRACK_MIME
        flags["attributes"] = [
            DocumentAttributeAudio(
                duration=ogg_tags.duration_seconds(header),
                voice=False,
            )
        ]
    elif kind in _VIDEO_KINDS:
        # Telethon fills this from `hachoir`, and with no metadata reader present
        # it sends `w=1, h=1, duration=0` for every video. Telegram re-derives the
        # real size so a viewer sees nothing wrong, but the attribute is wrong and
        # a client that lays out from it gets a 1x1 box. Zeros here mean the
        # container said nothing, and then no attribute is sent at all rather than
        # a wrong one - which is exactly today's behaviour, so nothing regresses.
        width, height, duration = video_dims.dimensions(header, tail)
        if width and height:
            flags["attributes"] = [
                DocumentAttributeVideo(
                    duration=duration,
                    w=width,
                    h=height,
                    round_message=kind == "video_note",
                    supports_streaming=kind == "video",
                )
            ]
    return flags


def resolve_kind(
    file_name: str, kind, caption: str = "", header: bytes = b"", tail: bytes = b""
) -> str:
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
        # `.ogg` is the one extension whose two readings are indistinguishable
        # from the outside: a voice note and a music file are both Opus, mono
        # and 48 kHz. Only there is the container worth reading, and only when
        # the caller named nothing - an explicit kind is never second-guessed.
        if header and family_of(file_name) is FAMILIES["voice"]:
            voice = ogg_tags.looks_like_voice(header)
            if voice is not None:
                kind = "voice_note" if voice else "audio"
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

    if kind == "video_note" and header:
        # Telegram does not refuse a rectangular video note - it silently drops
        # the round flag and delivers an ordinary video, so the caller believes
        # they sent one thing and the recipient sees another. Measured on real
        # Telegram 2026-09-20: 640x360 with `video_note=True` arrived as `video`.
        # Refusing here is the only point at which the caller can be told.
        width, height, _ = video_dims.dimensions(header, tail)
        if width and height and width != height:
            raise MediaKindError(
                f"{file_name} is {width}x{height}, and a video note has to be square. "
                "Telegram would accept this and deliver it as an ordinary video without "
                "saying so. Crop it to a square yourself, or send it as video. Nothing "
                "was sent, and nothing was converted."
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


def group_sends(kinds, headers=None, names=None):
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
        if len(indices) == 1:
            # One file, one set of keywords - so it can carry the per-file
            # attribute that `audio` needs. This is the common case and the only
            # one where a per-file attribute is expressible at all.
            index = indices[0]
            head = headers[index] if headers and index < len(headers) else b""
            planned.append((indices, flags_for(kinds[index], head, names[index] if names else "")))
            continue
        # A real group. `attributes` describes ONE document, so it cannot be
        # merged across members; a grouped track therefore carries the same
        # flags it did before, and states no duration. Telegram groups audio
        # with audio, so this only costs the duration, never the kind.
        flags = {}
        for index in indices:
            flags.update(_FLAGS[kinds[index]])
        planned.append((indices, flags))
    return planned


async def caption_flags(entities, caption: str, account=None) -> dict:
    """The keyword that makes a caption carry its own formatting, or nothing.

    `send_file` was the only sending path in this server with no entity
    argument, so a caption needing a premium emoji had to be sent and then
    EDITED - which marks the message "edited" in every client. Telegram has no
    such limit and neither does Telethon, whose `send_file` takes
    `formatting_entities`; only this server did.

    Built through the same `build_send_entities` the send, edit, schedule and
    quick-reply paths use, so a malformed span is refused here in the same words
    rather than arriving as a caption with its formatting silently dropped.

    Returns ``{}`` when no entities were asked for: an absent argument must not
    change the call every existing caller already makes.
    """
    if not entities:
        return {}
    from telegram_mcp.entities import build_send_entities

    built = await build_send_entities(entities, caption or "", account)
    if isinstance(built, str):
        raise MediaKindError(built)
    return {"formatting_entities": built} if built else {}
