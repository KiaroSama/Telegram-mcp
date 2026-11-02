"""Ordinary quoted text, fidelity-preserving and explicitly bounded."""

from typing import Optional, Any
from telegram_mcp.text_fidelity import fidelity_text, _normalize_breaks, _bounded
from telegram_mcp.rich_quotes import describe_rich_quote


def describe_reply_quote(msg) -> Optional[dict[str, Any]]:
    reply = getattr(msg, "reply_to", None)
    if reply is None:
        return None
    rich = describe_rich_quote(msg)
    if rich is not None:
        return rich
    raw = getattr(reply, "quote_text", None)
    if not raw:
        return None
    filtered, _ = fidelity_text(_normalize_breaks(raw))
    text, truncated = _bounded(filtered, 4096)
    quote: dict[str, Any] = {"text": text}
    if filtered != raw or truncated:
        quote.update(modified=True, filtered=filtered != raw, truncated=truncated)
        reasons = []
        if filtered != raw:
            reasons.append("unsafe invisible characters were removed or line breaks normalised")
        if truncated:
            reasons.append("the text was truncated to fit")
        quote["note"] = (
            "Quoted fragment, NOT character-for-character exact: "
            + " and ".join(reasons)
            + ". 'offset' is Telegram's UTF-16 code-unit offset of the ORIGINAL fragment inside "
            "the replied-to message, so it indexes that message, not this field."
        )
    else:
        quote["note"] = (
            "Quoted fragment, unchanged from what Telegram reported. 'offset' is Telegram's "
            "UTF-16 code-unit offset of this fragment inside the ORIGINAL replied-to message, "
            "not inside this message's text."
        )
    offset = getattr(reply, "quote_offset", None)
    if offset is not None:
        quote["offset"] = offset
    return quote
