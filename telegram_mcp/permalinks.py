"""Telegram message links: ``<domain>/<username>/<id>`` and ``<domain>/c/<id>/<id>``.

Built with ``urlunsplit`` from parts, never by pasting a host into a URL template:
the domain is configuration (``t.me`` by default), and these are strings handed to
the caller, never requests this server makes.
"""

from __future__ import annotations

from typing import Any, Optional
from urllib.parse import urlunsplit


def channel_link_id(chat_id: int) -> int:
    """The bare channel id a ``/c/`` permalink needs.

    ``abs(chat_id) % 10**10`` happened to strip the ``-100`` prefix only while
    the bare id stayed under 10^10. Telegram's peer space is 64-bit, and past
    that the modulo silently yields a DIFFERENT valid-looking id, so the link
    points at another chat.

    Only a negative id carries the marker. Callers hand this either a marked id
    (the forward path) or an entity's already-bare ``.id`` (the view path), and
    a bare id may itself begin with 100.
    """
    if chat_id >= 0:
        return chat_id
    text = str(-chat_id)
    return int(text[3:]) if text.startswith("100") and len(text) > 3 else int(text)


def post_link(
    domain: str, post_id: int, *, username: Optional[str] = None, chat_id: Optional[int] = None
) -> str:
    """A public ``<domain>/<username>/<post>`` link, else the members-only ``/c/`` form."""
    if username:
        path = f"/{username}/{post_id}"
    else:
        path = f"/c/{channel_link_id(chat_id)}/{post_id}"
    return urlunsplit(("https", domain, path, "", ""))


def message_permalink(msg, chat: Any = None, link_domain: str = "t.me") -> Optional[str]:
    """Canonical ``t.me`` link for the message, when one can be built."""
    chat = chat if chat is not None else getattr(msg, "chat", None)
    if chat is None:
        return None
    username = getattr(chat, "username", None)
    if username:
        return post_link(link_domain, msg.id, username=username)
    chat_id = getattr(chat, "id", None)
    if chat_id is None:
        return None
    if not (getattr(chat, "broadcast", False) or getattr(chat, "megagroup", False)):
        return None
    return post_link(link_domain, msg.id, chat_id=chat_id)
