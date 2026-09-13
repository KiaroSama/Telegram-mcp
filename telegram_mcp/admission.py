"""What a client must satisfy before this process serves from it.

Startup had these checks and a hot reload did not, so the same server enforced
two different rules depending on whether an account was present at boot or added
while it ran. The checks live here so both paths run the same ones.

What admission is for, in the order the damage happens:

* **Two labels, one session.** One session string is one auth key, and Telegram
  permanently invalidates a key used from two places at once. Caught before any
  socket opens it is a sentence naming both labels; caught afterwards it is a
  burned login for both.
* **Two processes, one session.** The per-session file lock is what stops a
  second instance of this server connecting a session the first already holds.
  A client added by a reload never took one, so exactly the case the lock exists
  for came back through the side door.

Rollback is part of the contract: a client that fails admission releases
whatever it took, and never replaces a client that is working.
"""

import asyncio
import logging
from typing import Dict, Optional

from telegram_mcp.safe_log import log_event
from telegram_mcp.settings import StartupMessage
from telegram_mcp.singleton import DEFAULT_GRACE_SECONDS, SessionLock, session_identity

# Label -> the session lock this process holds for it. Shared by startup and by
# reload admission, so a label that moves from one to the other does not end up
# holding two locks or none.
session_locks: Dict[str, SessionLock] = {}

# Labels a reload published that have not been admitted yet. Admission needs an
# event loop and `refresh_accounts` is synchronous, so the work is deferred to
# the first async use of the client - `ensure_connected` - rather than skipped.
_awaiting_admission: Dict[str, object] = {}


def reject_duplicate_sessions(configured: dict) -> None:
    """Refuse a configuration where two labels name the SAME Telegram session.

    Not a precedence question: one session is one auth key, and connecting it
    twice makes Telegram invalidate it for both labels. Cheap and offline -
    reading the session costs nothing - so it runs on every path that publishes
    clients, not only at startup.
    """
    by_identity: Dict[str, str] = {}
    for label, client in configured.items():
        try:
            identity = session_identity(client)
        except Exception:
            # An identity that cannot be read is not evidence of a duplicate;
            # the connect path reports whatever is actually wrong with it.
            continue
        first = by_identity.setdefault(identity, label)
        if first != label:
            raise StartupMessage(
                f"Accounts '{first}' and '{label}' are configured with the same "
                "Telegram session. One session is one auth key, and connecting it "
                "twice makes Telegram invalidate it for both. Generate a separate "
                "session per account with `uv run session_string_generator.py`."
            )


def mark_awaiting_admission(added: Dict[str, object]) -> None:
    """Record clients a reload published, for admission on their first use."""
    _awaiting_admission.update(added)


def forget(label: str) -> None:
    """Release and drop whatever a retired label was holding."""
    _awaiting_admission.pop(label, None)
    lock = session_locks.pop(label, None)
    if lock is not None:
        try:
            lock.release()
        except Exception as error:
            log_event(logging.WARNING, "failed to release a session lock", error=error)


async def claim_session(label: str, client, grace_seconds: Optional[float] = None) -> None:
    """Take this process's exclusive lock on the client's session.

    Unconditional on purpose. A short-circuit on "this label already has a
    lock" reads like harmless idempotence and is not: the same label handed a
    SECOND client for the same session is exactly the duplicate-connection case
    the lock exists to refuse, and skipping the acquire let it through.
    Not-twice-for-the-same-client is `admit_if_pending`'s job, which it does by
    forgetting the label once it succeeds.
    """
    lock = SessionLock(session_identity(client))
    await asyncio.to_thread(
        lock.acquire,
        grace_seconds=DEFAULT_GRACE_SECONDS if grace_seconds is None else grace_seconds,
    )
    session_locks[label] = lock


async def admit_if_pending(client) -> None:
    """Admit a reload-published client the first time anything uses it.

    Called from the connection path, which is the first place a hot-added client
    is touched from inside the event loop. Failure leaves the label pending and
    raises, so the caller sees why rather than talking to an unlocked session.
    """
    if not _awaiting_admission:
        return
    label = next(
        (name for name, pending in _awaiting_admission.items() if pending is client), None
    )
    if label is None:
        return
    try:
        await claim_session(label, client)
    except Exception:
        # Left pending on purpose: the next use tries again, and until then no
        # unlocked session has been served from.
        raise
    _awaiting_admission.pop(label, None)


def release_all() -> None:
    """Drop every lock this process holds. Shutdown's last step."""
    for lock in session_locks.values():
        try:
            lock.release()
        except Exception as error:
            log_event(logging.WARNING, "failed to release a session lock", error=error)
    session_locks.clear()
    _awaiting_admission.clear()


__all__ = [
    "_awaiting_admission",
    "admit_if_pending",
    "claim_session",
    "forget",
    "mark_awaiting_admission",
    "reject_duplicate_sessions",
    "release_all",
    "session_locks",
]
