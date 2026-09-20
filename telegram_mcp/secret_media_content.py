"""One file path plus a kind, turned into the request body TDLib expects.

Pure functions, deliberately. The mistake this module exists to prevent is
structural rather than behavioural -- the file goes one level DOWN, inside a
per-kind wrapper, never straight into the outer field:

    inputMessagePhoto { photo = inputPhoto { photo = inputFileLocal { ... } } }

Passing it a level too high leaves the inner field null, and TDLib answers
"InputFile is not specified": an error that names the TYPE it wanted and not the
PLACE it wanted it. That cost an evening once, when two kinds existed. There are
eight now, so the shape lives in one table and is asserted for every one of
them, in tests that never open a socket -- a live test would hide the bug behind
an upload.

The kinds are the eight the encrypted protocol carries. Its media vocabulary is
`Photo`, `Video`, `Audio`, `Document`, `ExternalDocument`, `Contact`, `GeoPoint`,
`Venue`, `WebPage` and `Empty`; sticker, animation, video note and voice note all
travel as `Document` with attributes, which is why they are kinds here and not a
separate mechanism.
"""

from pathlib import Path

# The names, the families and the caption rule live in `media_kinds` so they
# outlive this file: TDLib is being removed and the vocabulary is not. Re-exported
# under the names this module has always published, so existing importers are
# untouched by the move.
from telegram_mcp.media_kinds import (  # noqa: F401  (re-exported)
    FAMILIES as _FAMILIES,
    KINDS,
    NO_CAPTION as _NO_CAPTION,
    family_of,
    infer_kind,
)

__all__ = ["KINDS", "build_content", "infer_kind"]


# kind -> (td_api content type, the field holding the wrapper, the wrapper's type)
_SHAPE = {
    "photo": ("inputMessagePhoto", "photo", "inputPhoto"),
    "video": ("inputMessageVideo", "video", "inputVideo"),
    "document": ("inputMessageDocument", "document", "inputDocument"),
    "audio": ("inputMessageAudio", "audio", "inputAudio"),
    "animation": ("inputMessageAnimation", "animation", "inputAnimation"),
    "sticker": ("inputMessageSticker", "sticker", "inputSticker"),
    "video_note": ("inputMessageVideoNote", "video_note", "inputVideoNote"),
    "voice_note": ("inputMessageVoiceNote", "voice_note", "inputVoiceNote"),
}


# The two with no caption field at all. A caption passed with either is refused
# rather than dropped: a caller who wrote one and saw it vanish cannot find out
# why, and that silent loss is the thing this feature exists to remove.
def build_content(path: str, kind: str, caption: str = "") -> dict:
    """The ``input_message_content`` for one file.

    Raises ``ValueError`` with a message meant for the caller -- every tool here
    already turns that into its reply -- when the kind is unknown, when the file
    cannot be that kind, or when a caption was given to a kind that has none.
    Each of those is refused BEFORE the upload, because Telegram rejects them
    after the bytes have crossed the wire and answers with an error naming
    neither the file nor the kind.
    """
    if kind not in _SHAPE:
        raise ValueError(
            f"'{kind}' is not a kind a secret chat carries. Use one of: "
            f"{', '.join(KINDS)}. Leave kind unset to have it chosen from the file. "
            "Nothing was sent."
        )

    family = family_of(path)
    if family is not None and kind not in family["allows"]:
        suffix = Path(path).suffix.lower() or "that file"
        raise ValueError(
            f"A {suffix} file cannot be sent as {kind}: it is {family['name']} content, which "
            f"Telegram accepts as {', '.join(sorted(family['allows']))}. Telegram refuses "
            "this after the upload rather than before it, so it is refused here instead. "
            "Nothing was sent."
        )

    if caption and kind in _NO_CAPTION:
        raise ValueError(
            f"A {kind} carries no caption - the encrypted protocol has no field for one, so "
            "the text would be dropped in transit with no error. Send the caption as a "
            "separate message, or use kind='document' to keep them together. Nothing was "
            "sent."
        )

    content_type, field, wrapper = _SHAPE[kind]
    content = {
        "@type": content_type,
        # One level down. See the module docstring; this is the whole point.
        field: {"@type": wrapper, field: {"@type": "inputFileLocal", "path": str(path)}},
    }
    if kind not in _NO_CAPTION:
        content["caption"] = {"@type": "formattedText", "text": caption or ""}
    return content
