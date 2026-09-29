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
import os
import tempfile
import threading
from pathlib import Path
from typing import Optional

from telegram_mcp.alias_store import restrict_to_owner
from telegram_mcp.settings import state_dir

__all__ = ["drop_chat", "load", "remember"]

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
    except ValueError:
        # Unparseable references are unusable references; starting empty costs the
        # after-restart save of those files, never the chat.
        return {}
    return state if isinstance(state, dict) else {}


def _write(account: str, state: dict) -> None:
    path = _path(account)
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(state, fh, sort_keys=True)
        restrict_to_owner(Path(temporary))
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
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
