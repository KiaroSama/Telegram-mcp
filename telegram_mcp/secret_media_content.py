"""One file path plus a kind, checked before a single byte is uploaded.

Pure functions, deliberately, and the timing is the point. Telegram refuses a
mismatched kind only AFTER the bytes have crossed the wire, and answers with an
error naming neither the file nor the kind - so a caller who sent a photo as a
voice note pays for the upload and learns nothing. Each of the three refusals here
happens first, in front of the caller, with the reason.

The kinds are the eight the encrypted protocol carries. Its media vocabulary is
`Photo`, `Video`, `Audio`, `Document`, `ExternalDocument`, `Contact`, `GeoPoint`,
`Venue`, `WebPage` and `Empty`; sticker, animation, video note and voice note all
travel as `Document` with attributes, which is why they are kinds here and not a
separate mechanism.

This module used to also BUILD the request body, whose one hard-won lesson was that
the file went a level deeper than it read. That shape belonged to the previous
backend and went with it; the encryption package now builds its own, from the same
eight-kind vocabulary. ADR 0003 made that vocabulary backend-neutral for exactly
this moment, and it survived the swap untouched.
"""

from pathlib import Path

# The names, the families and the caption rule live in `media_kinds` so they
# outlive this file: the previous backend was removed and the vocabulary was not.
# Re-exported
# under the names this module has always published, so existing importers are
# untouched by the move.
from telegram_mcp.media_kinds import (  # noqa: F401  (re-exported)
    FAMILIES as _FAMILIES,
    KINDS,
    NO_CAPTION as _NO_CAPTION,
    family_of,
    infer_kind,
)

__all__ = ["KINDS", "infer_kind", "validate_kind"]


def validate_kind(path: str, kind: str, caption: str = "") -> str:
    """The kind, once it is proved to be one this file and this protocol allow.

    Raises ``ValueError`` with a message meant for the caller -- every tool here
    already turns that into its reply -- when the kind is unknown, when the file
    cannot be that kind, or when a caption was given to a kind that has none.
    Each of those is refused BEFORE the upload, because Telegram rejects them
    after the bytes have crossed the wire and answers with an error naming
    neither the file nor the kind.
    """
    if kind not in KINDS:
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

    return kind
