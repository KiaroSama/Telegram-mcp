"""Every public Telegram link this server hands out, built in one place.

Message links (``<domain>/<username>/<id>``, ``<domain>/c/<id>/<id>``) and plain ones
(``<domain>/<username>``, ``<domain>/addstickers/<set>``) all use ``LINK_DOMAIN``, read
once from ``TELEGRAM_LINK_DOMAIN``. They are built with ``urlunsplit`` from parts, never
by pasting a host into a URL template: these are strings handed to the caller, never
requests this server makes.
"""

from __future__ import annotations

import os
from typing import Any, Optional
from urllib.parse import urlunsplit

_DEFAULT_DOMAIN = "t.me"
_SCHEMES = ("https://", "http://", "")


def normalize_domain(raw: Optional[str]) -> str:
    """The bare host of a configured link domain: no scheme, no slashes, no spaces."""
    host = (raw or "").strip()
    for scheme in _SCHEMES[:2]:
        if host.lower().startswith(scheme):
            host = host[len(scheme) :]
    return host.strip("/ ") or _DEFAULT_DOMAIN


LINK_DOMAIN = normalize_domain(os.getenv("TELEGRAM_LINK_DOMAIN"))


def public_link(*path: str) -> str:
    """``https://<LINK_DOMAIN>/<path...>`` - a username, a sticker set, a post."""
    return urlunsplit(("https", LINK_DOMAIN, "/" + "/".join(str(p) for p in path), "", ""))


def username_prefixes() -> tuple:
    """Every link prefix a username may arrive with: Telegram's own domain and the
    configured one, each with and without a scheme."""
    domains = dict.fromkeys((_DEFAULT_DOMAIN, LINK_DOMAIN))
    return tuple(f"{scheme}{d}/" for d in domains for scheme in _SCHEMES)


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
