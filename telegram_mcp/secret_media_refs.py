"""Received secret-chat files' references, kept on disk so a file outlives a restart.

A secret chat's file key travels INSIDE the message that carried it, so once the
process that received it is gone the bytes are unreachable - unless the key was kept.
The package hands out a ``MediaReference`` for exactly this and leaves where to keep it
to the application. The owner decided (2026-09-29): on disk, beside the chat keys
themselves, readable only by the owner, and dropped with the chat.

The entries are plain JSON produced by ``MediaReference.to_dict``; turning them back
into objects is :mod:`telegram_mcp.secret_backend`'s job, the only module allowed to
import the package. A reference holds a file key, so nothing here ever logs one.
"""

import json
import logging
import os
import tempfile
import threading
from pathlib import Path
from typing import Optional

from telegram_mcp.owner_only import restrict_to_owner_strict
from telegram_mcp.safe_log import log_event
from telegram_mcp.settings import state_dir

__all__ = ["drop_chat", "forget", "load", "remember"]

#: Per chat, like the message history beside it: the package keeps no more either.
_PER_CHAT_LIMIT = 500

_lock = threading.RLock()


def _path(account: str) -> Path:
    return state_dir() / "secret-chats" / f"{account}-media.json"


def _read(account: str) -> dict:
    try:
        state = json.loads(_path(account).read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    # Corrupt data may still hold recoverable file keys. It is not an empty
    # store, and no later arrival may overwrite it under that assumption.
    valid = isinstance(state, dict) and all(
        isinstance(chat, dict)
        and all(
            isinstance(item, dict)
            and isinstance(item.get("reference"), dict)
            and type(item.get("ttl", 0)) is int
            and item.get("ttl", 0) >= 0
            for item in chat.values()
        )
        for chat in state.values()
    )
    if not valid:
        raise ValueError(
            "The secret media reference file has an invalid structure; it was preserved"
        )
    return state


def _write(account: str, state: dict) -> None:
    path = _path(account)
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as fh:
            if not restrict_to_owner_strict(temporary):
                raise OSError("Temporary secret state could not be made owner-only")
            # Insertion order is the arrival order; random Telegram IDs are not
            # chronological. Sorting keys changes which record is evicted next.
            json.dump(state, fh)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            Path(temporary).unlink(missing_ok=True)
        except OSError as error:
            log_event(logging.WARNING, "could not remove temporary media references", error=error)
        raise


def remember(account: str, chat_id: int, message_id: int, reference: dict, ttl: int) -> None:
    """Keep one received file's reference, with the timer it arrived under."""
    with _lock:
        state = _read(account)
        chat = state.setdefault(str(int(chat_id)), {})
        chat[str(int(message_id))] = {"reference": reference, "ttl": int(ttl or 0)}
        while len(chat) > _PER_CHAT_LIMIT:
            chat.pop(next(iter(chat)))
        _write(account, state)


def load(account: str, chat_id: int, message_id: int) -> Optional[dict]:
    """``{"reference": ..., "ttl": ...}`` for one file, or ``None``."""
    with _lock:
        return _read(account).get(str(int(chat_id)), {}).get(str(int(message_id)))


def drop_chat(account: str, chat_id: int) -> None:
    """Forget every reference of one chat: its keys go when the chat does."""
    with _lock:
        state = _read(account)
        if state.pop(str(int(chat_id)), None) is not None:
            _write(account, state)


def forget(account: str, chat_id: int, message_ids) -> None:
    """Remove the decryption references for exactly the erased messages."""
    wanted = {str(int(message_id)) for message_id in message_ids}
    with _lock:
        state = _read(account)
        key = str(int(chat_id))
        chat = state.get(key, {})
        if not wanted.intersection(chat):
            return
        for message_id in wanted:
            chat.pop(message_id, None)
        if not chat:
            state.pop(key, None)
        _write(account, state)
