"""Composing one outgoing secret message: its text, its reply, and what is lost.

Three steps every send through a secret chat shares, and which the timed sends
in :mod:`telegram_mcp.tools.secret_timed` need identically. They live here
rather than in the tool module because that module reached 700 lines -- the
point at which this project closes a file to new code -- and because a helper
cannot live in two places at once.

The reply step is the one worth reading. TDLib translates a reply's message id
into the wire id the encrypted protocol uses by looking the original up in THIS
device's database, and when it cannot find it, `create_message_to_send` resets
`input_reply_to` and sends an ordinary message instead. No error, no warning, a
reply that is not a reply. A secret chat's history is local-only and
permanently gappy, so a target this login never received is an ordinary
situation rather than an exotic one -- which is why the lookup happens up front.
"""

from typing import Optional

from telegram_mcp.secret_limits import dropped_entities, secret_chat_layer
from telegram_mcp.tdlib import TDLibError

__all__ = ["dropped_note", "formatted_text", "reply_to"]


async def formatted_text(client, message: str, parse_mode: Optional[str]) -> dict:
    """``message`` as a TDLib ``formattedText``, parsed when a mode was asked for.

    TDLib does the parsing, not this server: ``parseTextEntities`` is the same
    code path the official clients use, so a caller's markdown behaves here
    exactly as it does in Telegram.
    """
    if not parse_mode:
        return {"@type": "formattedText", "text": message}

    wanted = str(parse_mode).strip().lower()
    if wanted in ("markdown", "md", "markdownv2"):
        mode = {"@type": "textParseModeMarkdown", "version": 2}
    elif wanted == "html":
        mode = {"@type": "textParseModeHTML"}
    else:
        raise ValueError(
            f"parse_mode must be 'markdown' or 'html', not {parse_mode!r}. Leave it unset "
            "to send the text exactly as written. Nothing was sent."
        )
    return await client.request(
        {"@type": "parseTextEntities", "text": message, "parse_mode": mode}
    )


async def reply_to(client, chat_id: int, message_id: Optional[int]) -> Optional[dict]:
    """The ``reply_to`` for a send, after proving the target is really there.

    ``None`` when no reply was asked for. Raises ``ValueError`` when the target
    is not in this device's copy, rather than letting Telegram silently
    downgrade the reply to an ordinary message.
    """
    if message_id is None:
        return None
    try:
        await client.request(
            {"@type": "getMessage", "chat_id": int(chat_id), "message_id": int(message_id)}
        )
    except TDLibError:
        raise ValueError(
            f"Message {message_id} is not in this device's copy of that chat, so it cannot "
            "be replied to. A secret chat has no server-side history to fetch it from, and "
            "Telegram would silently send this as an ordinary message rather than a reply. "
            "read_secret_messages shows what this login actually received. Nothing was sent."
        )
    return {"@type": "inputMessageReplyToMessage", "message_id": int(message_id)}


async def dropped_note(client, chat_id: int, formatted: dict) -> list:
    """Which of the caller's formatting this chat will not carry.

    Costs nothing on an unformatted message: with no entities there is nothing
    to drop, so the chat's layer is never fetched.
    """
    entities = (formatted or {}).get("entities") or []
    if not entities:
        return []
    layer = await secret_chat_layer(client, int(chat_id))
    return dropped_entities(entities, layer)
