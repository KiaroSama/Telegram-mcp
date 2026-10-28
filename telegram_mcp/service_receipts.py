"""Whitelisted, exact machine fields from Stars gift service receipts."""

import re
from datetime import datetime
from typing import Any
from urllib.parse import urlsplit

from telethon.tl.types import MessageActionGiftStars, StarsAmount, StarsTransaction


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


def stars_transaction_details(value: Any, reference: str) -> dict[str, Any] | None:
    """Project one exact ledger observation, never infer delivery from its flags."""
    if not isinstance(value, StarsTransaction) or value.id != reference:
        return None
    amount = value.amount
    if (
        not isinstance(amount, StarsAmount)
        or type(amount.amount) is not int
        or not -(2**63) <= amount.amount < 2**63
        or type(amount.nanos) is not int
        or not -999999999 <= amount.nanos <= 999999999
        or not isinstance(value.date, datetime)
        or value.date.utcoffset() is None
    ):
        return None
    flags = {name: getattr(value, name, None) for name in ("gift", "pending", "failed", "refund")}
    if any(flag is not None and type(flag) is not bool for flag in flags.values()):
        return None
    result = {
        "id": reference,
        "amount": amount.amount,
        "nanos": amount.nanos,
        "date": value.date.isoformat(),
        "peer_type": type(value.peer).__name__,
        **{name: flag is True for name, flag in flags.items()},
    }
    date = value.transaction_date
    if isinstance(date, datetime) and date.utcoffset() is not None:
        result["transaction_date"] = date.isoformat()
    url = value.transaction_url
    if isinstance(url, str) and 0 < len(url) <= 2048 and re.fullmatch(r"[\x21-\x7e]+", url):
        try:
            parsed = urlsplit(url)
            if (
                parsed.scheme == "https"
                and parsed.hostname
                and not parsed.username
                and not parsed.password
            ):
                result["transaction_url"] = url
        except ValueError:
            pass
    return result
