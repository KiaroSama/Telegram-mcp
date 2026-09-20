"""Encoding and re-reading the opaque handle `get_gif_search` returns.

A GIF cannot be sent by its Telegram document id: the access hash and file
reference are missing and Telethon refuses to cast it to any InputMedia. What can
be sent is the inline query that found it, so the handle carries the query id, the
result id, the account it was obtained on and when Telegram will forget it - and
every one of those is a distinct way a caller can be wrong, each with its own
refusal.

Its own file because `tools/media.py` reached the project's 800-line ceiling; this
is the piece with the least to do with sending media and the most self-contained
rules of its own.
"""

import time
from typing import Optional

__all__ = ["GIF_HANDLE_PREFIX", "account_label", "gif_handle", "parse_gif_handle"]

GIF_HANDLE_PREFIX = "gif"


def account_label(account: Optional[str]) -> str:
    """The label a GIF handle is scoped to. Single-account mode has no label."""
    return (account or "default").lower()


def gif_handle(account: Optional[str], expires_at: int, query_id: int, result_id: str) -> str:
    return f"{GIF_HANDLE_PREFIX}:{account_label(account)}:{expires_at}:{query_id}:{result_id}"


def parse_gif_handle(handle, account: Optional[str]) -> tuple:
    """``((query_id, result_id), None)`` or ``(None, refusal)``."""
    parts = str(handle).split(":", 4)
    if len(parts) != 5 or parts[0] != GIF_HANDLE_PREFIX:
        return None, (
            "gif_id must be the opaque handle get_gif_search returned. A Telegram "
            "document id on its own cannot be sent: the access hash and file "
            "reference are missing, and Telethon refuses to cast it to any InputMedia."
        )
    _, label, expires_at, query_id, result_id = parts
    if label != account_label(account):
        return None, (
            f"This GIF handle was obtained on account '{label}' and cannot be sent from "
            f"'{account_label(account)}': the inline query id belongs to that session. "
            "Run get_gif_search again on this account."
        )
    try:
        expires_at, query_id = int(expires_at), int(query_id)
    except ValueError:
        return None, "Malformed GIF handle. Run get_gif_search again."
    if time.time() >= expires_at:
        return None, (
            "This GIF handle has expired: Telegram caches an inline query for a "
            "limited time and then forgets its query id. Run get_gif_search again."
        )
    return (query_id, result_id), None
