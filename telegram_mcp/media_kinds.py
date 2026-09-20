"""The eight shapes Telegram gives a sent file, and which files may take which.

A media kind is a property of the SENDING and never of the bytes: the same
recording is an audio file with a play button or a voice note with a waveform
depending only on what was asked for, and the same clip is a video, a round video
note or a soundless animation. `CONTEXT.md` carries the term; this module is the
list and the rules.

It names no backend on purpose. These names lived inside `secret_media_content.py`
until 2026-09-20, next to the TDLib request bodies that were their only consumer -
and that file is deleted with TDLib, which would have taken the only description of
a complete media surface with it. `docs/adr/0003-the-media-kind-vocabulary-outlives-tdlib.md`
records the move. Anything here that mentions a transport is a mistake, and
`tests/test_media_kinds.py` says so mechanically.
"""

from pathlib import Path

__all__ = ["KINDS", "NO_CAPTION", "FAMILIES", "family_of", "infer_kind"]

#: The eight the encrypted protocol carries. Ordinary chats carry more message
#: types, but those are not files.
KINDS = (
    "photo",
    "video",
    "document",
    "audio",
    "animation",
    "sticker",
    "video_note",
    "voice_note",
)

#: The two with no caption field at all. A caption passed with either is refused
#: rather than dropped: a caller who wrote one and saw it vanish cannot find out
#: why, and that silent loss is the thing this vocabulary exists to remove.
NO_CAPTION = frozenset({"sticker", "video_note"})

# Extension -> the family it belongs to. Inference picks the family's default;
# compatibility allows any kind within the family, because the default is a
# guess and the caller's explicit kind is not.
FAMILIES = {
    "image": {
        "name": "image",
        "suffixes": {".jpg", ".jpeg", ".png", ".webp", ".bmp", ".heic", ".heif"},
        "default": "photo",
        "allows": {"photo", "sticker", "document"},
    },
    "video": {
        "name": "video",
        "suffixes": {".mp4", ".mov", ".mkv", ".webm", ".avi", ".m4v"},
        "default": "video",
        "allows": {"video", "video_note", "animation", "document"},
    },
    # No `voice_note`. A voice message is Telegram's own OGG/Opus recording, and
    # an mp3 is not one - the owner's ruling on 2026-09-20 after the real-client
    # pass accepted a 10 MB mp3 as a voice note and started uploading it. The
    # `voice` family below is where both readings genuinely live.
    "audio": {
        "name": "audio",
        "suffixes": {".mp3", ".m4a", ".flac", ".wav", ".aac"},
        "default": "audio",
        "allows": {"audio", "document"},
    },
    # Telegram's voice format. Defaulting .ogg to a voice note rather than to
    # music is the commoner intent by a wide margin, and `kind="audio"` is
    # inside the same family, so the other reading costs one argument.
    "voice": {
        "name": "voice",
        "suffixes": {".ogg", ".oga", ".opus"},
        "default": "voice_note",
        "allows": {"voice_note", "audio", "document"},
    },
    "animation": {
        "name": "animation",
        "suffixes": {".gif"},
        "default": "animation",
        "allows": {"animation", "video", "document"},
    },
    "sticker": {
        "name": "sticker",
        "suffixes": {".tgs"},
        "default": "sticker",
        "allows": {"sticker", "document"},
    },
}


def family_of(path: str):
    """The family a path's extension belongs to, or ``None`` for an unknown one.

    Each family carries its own ``name``, so a refusal can say "it is video
    content" rather than making every caller keep a second lookup to find out.
    """
    suffix = Path(path).suffix.lower()
    for family in FAMILIES.values():
        if suffix in family["suffixes"]:
            return family
    return None


def infer_kind(path: str) -> str:
    """The kind to send ``path`` as when the caller did not choose one.

    Anything unrecognised becomes ``document``, which carries any bytes at all.
    Guessing a specific kind at an unknown extension would fail at the protocol
    instead of simply arriving as a file.
    """
    family = family_of(path)
    return family["default"] if family else "document"
