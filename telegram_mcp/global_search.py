"""Bounded native global search: message IDs are local to each peer.

Adapted from chigwell/telegram-mcp a64be89 (Apache-2.0), with strict cursor
validation and this server's existing page/filter budgets.
"""

import base64
import binascii
import hashlib
import json
from dataclasses import dataclass

from telethon import functions, types, utils


class CursorError(ValueError):
    """A continuation that cannot safely describe this search."""


@dataclass(frozen=True)
class Offset:
    rate: int = 0
    peer: tuple = ()
    message: int = 0

    def input_peer(self):
        if not self.peer:
            return types.InputPeerEmpty()
        kind, identifier, access_hash = self.peer
        if kind == "user":
            return types.InputPeerUser(identifier, access_hash)
        if kind == "channel":
            return types.InputPeerChannel(identifier, access_hash)
        return types.InputPeerChat(identifier)


def _scope(query, chosen, account):
    raw = json.dumps([query, type(chosen).__name__, account], ensure_ascii=False)
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


def encode(offset, scope):
    raw = json.dumps([1, scope, offset.rate, offset.peer, offset.message], separators=(",", ":"))
    return base64.urlsafe_b64encode(raw.encode("utf-8")).decode("ascii")


def decode(cursor, scope):
    try:
        if not isinstance(cursor, str) or not 0 < len(cursor) <= 2048:
            raise ValueError
        raw = base64.b64decode(cursor.encode("ascii"), altchars=b"-_", validate=True)
        version, bound, rate, peer, message = json.loads(raw.decode("utf-8"))
        if type(version) is not int or version != 1 or bound != scope:
            raise ValueError
        if any(type(value) is not int or not 0 <= value < 2**31 for value in (rate, message)):
            raise ValueError
        if not isinstance(peer, list) or len(peer) not in (0, 3):
            raise ValueError
        if peer:
            kind, identifier, access_hash = peer
            if kind not in ("user", "channel", "chat"):
                raise ValueError
            if type(identifier) is not int or not 0 < identifier < 2**63:
                raise ValueError
            if type(access_hash) is not int or not -(2**63) <= access_hash < 2**63:
                raise ValueError
        return Offset(rate, tuple(peer), message)
    except (ValueError, TypeError, UnicodeError, binascii.Error, RecursionError) as error:
        raise CursorError(
            "cursor is invalid or belongs to another query, filter or account."
        ) from error


def _peer(message, entities):
    marked = utils.get_peer_id(message.peer_id)
    entity = entities.get(marked)
    try:
        peer = utils.get_input_peer(entity) if entity is not None else None
    except (TypeError, ValueError):
        peer = None
    if isinstance(peer, types.InputPeerUser):
        return ("user", peer.user_id, peer.access_hash)
    if isinstance(peer, types.InputPeerChannel):
        return ("channel", peer.channel_id, peer.access_hash)
    if isinstance(message.peer_id, types.PeerChat):
        return ("chat", message.peer_id.chat_id, 0)
    raise CursorError("Telegram omitted the peer needed for safe search continuation.")


async def page(client, query, chosen, account, size, skip=0, cursor=None, connect=None):
    """Return messages/entities, continuation and explicit exhaustion/budget metadata."""
    chosen = chosen or types.InputMessagesFilterEmpty()
    scope = _scope(query, chosen, account)
    offset = decode(cursor, scope) if cursor is not None else Offset()
    if connect is not None:
        await connect(client)
    records, seen, empty = [], set(), 0
    remaining = skip
    # A legacy deep page can cost at most 25 chunks plus bounded empty responses.
    for _ in range(28 if skip else 3):
        response = await client(
            functions.messages.SearchGlobalRequest(
                q=query,
                filter=chosen,
                min_date=None,
                max_date=None,
                offset_rate=offset.rate,
                offset_peer=offset.input_peer(),
                offset_id=offset.message,
                limit=min(100, remaining if remaining else size),
            )
        )
        messages = list(getattr(response, "messages", None) or [])
        entities = {
            utils.get_peer_id(e): e
            for e in list(getattr(response, "users", None) or [])
            + list(getattr(response, "chats", None) or [])
        }
        real = [m for m in messages if not isinstance(m, types.MessageEmpty)]
        whole = isinstance(response, types.messages.Messages)
        if not real:
            empty += 1
            if whole:
                return records, None, True, False
            # No peer/message position exists: retain rate progress, not a fabricated peer.
            following = Offset(
                getattr(response, "next_rate", None) or 0, offset.peer, offset.message
            )
        else:
            empty = 0
            if remaining:
                remaining -= len(real)
            else:
                records.extend((m, entities) for m in real)
            if whole:
                return records, None, True, False
            last = real[-1]
            following = Offset(
                getattr(response, "next_rate", None) or 0, _peer(last, entities), last.id
            )
        if following == offset or following in seen:
            exhausted = not real and not getattr(response, "next_rate", None)
            return records, None, exhausted, not exhausted
        seen.add(offset)
        offset = following
        if records:
            return records, encode(offset, scope), False, False
        if empty >= 3:
            break
    return records, encode(offset, scope) if not remaining else None, False, True
