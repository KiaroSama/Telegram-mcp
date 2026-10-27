"""Whitelisted, exact machine fields from Stars gift service receipts."""

import re
from typing import Any

from telethon.tl.types import MessageActionGiftStars


def stars_receipt_details(action: Any) -> dict[str, Any] | None:
    """Omit invalid fields rather than rewriting them into plausible evidence."""
    if not isinstance(action, MessageActionGiftStars):
        return None
    details: dict[str, Any] = {}
    for name in ("stars", "amount", "crypto_amount"):
        value = getattr(action, name, None)
        if type(value) is int and 0 <= value < 2**63:
            details[name] = value
    for name, pattern in (
        ("currency", r"[A-Z]{3}"),
        ("crypto_currency", r"[A-Z0-9]{1,16}"),
        ("transaction_id", r"[\x21-\x7e]{1,512}"),
    ):
        value = getattr(action, name, None)
        if isinstance(value, str) and re.fullmatch(pattern, value):
            details[name] = value
    return details or None
