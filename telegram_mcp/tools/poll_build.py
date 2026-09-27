"""The parts a poll is built from: formatted text, attached files, voter rules.

Shared by ``poll_creation`` (a new poll) and ``poll_manage`` (an option added to a
poll that exists), so an option added later is formatted and attached exactly as
one written at creation.
"""

import re
from typing import Optional

from telegram_mcp.runtime import *
from telegram_mcp.secret_compose import formatted_text

# https://core.telegram.org/api/poll : a quiz explanation is 0-200 characters.
EXPLANATION_MAX = 200

# ISO 3166-1 alpha-2, the form `poll.countries_iso2` takes.
_ISO2 = re.compile(r"^[A-Za-z]{2}$")


def parse(text: str, parse_mode: Optional[str]) -> types.TextWithEntities:
    """Poll text with the server's own markdown/html parser applied."""
    plain, entities = formatted_text(str(text), parse_mode)
    return types.TextWithEntities(text=plain, entities=list(entities or []))


def countries_problem(countries) -> Optional[str]:
    """Why a country list cannot be sent, or ``None``."""
    bad = [c for c in countries if not _ISO2.match(str(c))]
    if bad:
        return (
            f"Error: {bad[0]!r} is not a two-letter country code (ISO 3166-1 alpha-2, "
            "like IR or DE). Nothing was sent."
        )
    return None


async def upload_attachment(cl, entity, ctx, path: str, tool_name: str):
    """``(InputMedia, error)`` for one local file, saved on Telegram without sending.

    ``messages.uploadMedia`` turns the upload into a photo or document Telegram
    keeps, which is what a poll's description, option or explanation must point at.
    The path goes through the same allowed-roots guard as every other file read.
    """
    async with _open_verified_source(raw_path=path, ctx=ctx, tool_name=tool_name) as (
        source,
        path_error,
    ):
        if path_error:
            return None, path_error
        # ponytail: Telethon-private helper, the same trade `rich_message_files` makes.
        _handle, media, _as_image = await cl._file_to_media(source.handle)
    if media is None:
        return None, f"Nothing uploadable was found at {path}."
    saved = await cl(functions.messages.UploadMediaRequest(peer=entity, media=media))
    photo = getattr(saved, "photo", None)
    if photo is not None:
        return types.InputMediaPhoto(id=utils.get_input_photo(photo)), None
    return types.InputMediaDocument(id=utils.get_input_document(saved.document)), None


__all__ = ["EXPLANATION_MAX", "countries_problem", "parse", "upload_attachment"]
