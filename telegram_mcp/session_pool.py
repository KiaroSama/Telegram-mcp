"""Claiming one of several interchangeable sessions for the same account.

Telegram forbids one auth key being used from two IPs at once, and on a
dual-stack or VPN host two local clients can egress via different source
addresses and collide -- which Telegram answers with
``AuthKeyDuplicatedError``, burning the session for both. The fix is one
authorized session per concurrent client, listed in
``TELEGRAM_SESSION_STRINGS``.

Which slot this process gets is decided here and nowhere else: an advisory
file lock per session, held for the life of the process, so two clients
deterministically pick different slots and a crash releases the claim. Keeping
it in its own module keeps that decision in one place -- `connection` builds
clients, this decides which session one of them is built on.
"""

from __future__ import annotations

import hashlib
import os
import re
import sys
import tempfile
from typing import List, Optional

from telegram_mcp.settings import StartupMessage
from telegram_mcp.singleton import try_lock_exclusive

__all__ = ["_parse_session_pool", "_acquire_session", "_SESSION_LOCKS", "_CLAIMED_SESSION"]


# --- Session pool ------------------------------------------------------------
# A POOL of interchangeable authorized sessions for the SAME account lets
# several concurrent MCP clients (e.g. the desktop app AND a terminal CLI) run
# against one Telegram account without tripping AuthKeyDuplicatedError.
#
# Telegram forbids one auth key (one StringSession) being used from two IPs at
# once; on a dual-stack / VPN host two local clients can egress via different
# source IPs and collide. The fix is one authorized session PER concurrent
# client (Telegram allows one account on many "devices"). Generate extra
# sessions with `uv run session_string_generator.py` and list them in
# TELEGRAM_SESSION_STRINGS (whitespace/comma/semicolon separated). Each process
# claims the first session not already locked by a live process via an advisory
# flock, so clients deterministically pick distinct slots; the OS releases the
# lock if a process dies.

# Acquired lock handles are held for the process lifetime so the advisory locks
# stay held until exit (or crash, when the OS releases them).
_SESSION_LOCKS: list = []

# The pooled session this process claimed, if any. Held so a rebuild hands back
# the slot already locked instead of taking another client's.
_CLAIMED_SESSION: Optional[str] = None


def _parse_session_pool(env: Optional[dict] = None) -> List[str]:
    """Parse TELEGRAM_SESSION_STRINGS into a de-duplicated list of sessions.

    Takes the SNAPSHOT its caller is working from. Reading `os.environ` here
    while `_discover_accounts` had been handed a freshly parsed environment
    meant the two disagreed inside one call: the accounts came from the new
    file and the pool from whatever the process happened to still hold.
    """
    raw = (os.environ if env is None else env).get("TELEGRAM_SESSION_STRINGS")
    if not raw:
        return []
    pool: List[str] = []
    for tok in re.split(r"[\s,;]+", raw.strip()):
        if tok and tok not in pool:
            pool.append(tok)
    return pool


def _acquire_session(pool: List[str]) -> str:
    """Claim the first free session in the pool via an advisory file lock.

    A slot this process already holds is returned again rather than re-claimed.
    Rebuilding an unchanged pool - which a hot reload does whenever anything
    else in `.env` moves - otherwise walked past its own locked slot, found it
    taken, and claimed the NEXT one, quietly consuming a slot that belonged to
    another live client.
    """
    global _CLAIMED_SESSION
    if _CLAIMED_SESSION is not None and _CLAIMED_SESSION in pool:
        return _CLAIMED_SESSION
    lock_dir = os.path.join(tempfile.gettempdir(), "telegram-mcp-session-locks")
    try:
        os.makedirs(lock_dir, exist_ok=True)
    except OSError:
        lock_dir = tempfile.gettempdir()
    for idx, session in enumerate(pool):
        # sha256, matching `singleton.SessionLock`, which derives its own lock
        # name from a session the same way. Two derivations of one concept using
        # two algorithms is a decision nobody made; this was the outlier, and it
        # is the one a security scan objects to.
        #
        # The name therefore changes. For one restart an old instance holds the
        # sha1 name and a new one the sha256 name, so both can claim the same
        # pool slot - and then `SessionLock`, which is keyed on the session
        # itself and unchanged, refuses the second. The window costs a failed
        # start, not a burned auth key.
        digest = hashlib.sha256(session.encode("utf-8")).hexdigest()[:16]
        lock_path = os.path.join(lock_dir, f"session-{digest}.lock")
        try:
            # "a+", not "w": on Windows the lock covers the first byte, and
            # truncating a file another live client holds is refused.
            fh = open(lock_path, "a+")
        except OSError:
            continue
        if not try_lock_exclusive(fh):
            # Locked by another live client — try the next session.
            try:
                fh.close()
            except Exception:
                pass
            continue
        _SESSION_LOCKS.append(fh)
        try:
            fh.seek(0)
            fh.truncate()
            fh.write(f"pid={os.getpid()}\n")
            fh.flush()
        except OSError:
            pass
        print(f"Using Telegram session slot {idx + 1}/{len(pool)}.", file=sys.stderr)
        _CLAIMED_SESSION = session
        return session
    # Handing out an already-claimed session here would make Telegram burn it
    # with AuthKeyDuplicatedError — losing the slot for the client that owns it
    # too. Refusing to start is recoverable; a burned session is not.
    raise StartupMessage(
        f"All {len(pool)} pooled Telegram session(s) are already claimed by other "
        "live clients, so this one has no session to use. Add another session to "
        "TELEGRAM_SESSION_STRINGS (generate it with "
        "`uv run session_string_generator.py`) — one slot per concurrent client — "
        "or stop one of the other clients."
    )
