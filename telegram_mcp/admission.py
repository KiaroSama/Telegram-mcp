"""What a client must satisfy before this process serves from it, and who owns the lease.

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

Getting the checks right was not enough, because a lease is a lifetime and the
first version tracked only a label. Four ways that came apart:

* **Two callers, two claims.** The pending entry was cleared only after the
  acquire returned, so simultaneous callers both started one. A POSIX lock is
  per open file description, so the second `flock` on the same file fails *from
  the same process* - the client failed against its own lease.
* **A late admission spoke for a client that was gone.** The completion popped
  the label, which by then could belong to a REPLACEMENT - erasing its pending
  entry - or to nothing at all, publishing a lock for a removed account.
* **A cancelled caller orphaned the acquire.** `asyncio.to_thread` cannot be
  cancelled. The thread finished, the lock object was dropped on the floor, and
  nothing held it or released it.
* **The lease was released before the socket was.** `forget()` released while
  the old client's `disconnect()` was still running, which is precisely the
  window a second process needs to claim a session this one is still using.

So: one single-flight operation per client, whose task - never the caller - owns
the acquire's result and disposes of it; a re-check after the acquire that this
client is still the one the label means; and a release that waits for the socket
it protects to be down.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass
from typing import Dict, Optional

from telegram_mcp.safe_log import log_event
from telegram_mcp.settings import StartupMessage
from telegram_mcp.singleton import DEFAULT_GRACE_SECONDS, SessionLock, session_identity

# How long a lease release waits for the socket it protects to finish closing.
# Bounded: a disconnect that will not complete must not hold the lease for the
# life of the process either, and the release says so when it gives up.
_CLOSE_BEFORE_RELEASE_SECONDS: float = 10.0


@dataclass
class _Lease:
    """One session held by this process, and who it is held for."""

    label: str
    client: object
    lock: SessionLock
    identity: str


# Label -> the session lock this process holds for it. The published view, kept
# under this name because the runner and its tests read it.
session_locks: Dict[str, SessionLock] = {}

# The same leases with their owner attached, which the label-keyed view cannot
# carry: "release the lock for label X" is the wrong question once X can mean a
# different client than it did when the lock was taken.
_leases: Dict[str, _Lease] = {}

# Labels a reload published whose lease has not been taken yet.
_awaiting_admission: Dict[str, object] = {}

# client -> the one task admitting it. Single-flight: a second caller waits on
# the first task rather than starting a competing acquire.
_admitting: Dict[int, asyncio.Task] = {}

# Releases waiting on a socket to finish closing, so shutdown can wait for them.
_releasing: set = set()


class AdmissionSuperseded(StartupMessage):
    """This client stopped being what its label means while it was being admitted.

    A `StartupMessage` so the readable-error path prints it: the caller is
    holding a client the configuration has moved on from, and retrying is the
    answer rather than a traceback.
    """


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


def _publish(label: str, client, lock: SessionLock, identity: str) -> None:
    session_locks[label] = lock
    _leases[label] = _Lease(label=label, client=client, lock=lock, identity=identity)


def _drop(label: str) -> Optional[_Lease]:
    session_locks.pop(label, None)
    return _leases.pop(label, None)


def _release_lock(lock: SessionLock, why: str) -> None:
    try:
        lock.release()
    except Exception as error:
        log_event(logging.WARNING, f"failed to release a session lock after {why}", error=error)


def mark_awaiting_admission(added: Dict[str, object]) -> None:
    """Record clients a reload published, for admission before they are served from."""
    _awaiting_admission.update(added)


def begin_admission(added: Dict[str, object]) -> None:
    """Start admitting the given clients NOW, if there is a loop to do it on.

    Waiting for the first async API call was enough for a server that answers
    tool calls and not for one that only waits for incoming events: nothing ever
    touched the new client, so nothing ever took its lease, and the account sat
    published and unadmitted. Starting here means admission is under way from the
    moment of publication; `admit_if_pending` still gates SERVING on it, so
    beginning early never means serving early.
    """
    mark_awaiting_admission(added)
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return  # synchronous context: the first async use starts it instead
    for client in added.values():
        _start_admission(client)


def _pending_label_of(client) -> Optional[str]:
    return next((name for name, pending in _awaiting_admission.items() if pending is client), None)


def _start_admission(client, grace_seconds: Optional[float] = None) -> Optional[asyncio.Task]:
    """The one task admitting this client, created if it does not exist yet."""
    existing = _admitting.get(id(client))
    if existing is not None and not existing.done():
        return existing
    label = _pending_label_of(client)
    if label is None:
        return None
    task = asyncio.ensure_future(_admit(label, client, grace_seconds))
    _admitting[id(client)] = task
    task.add_done_callback(lambda _t: _admitting.pop(id(client), None))
    return task


async def _admit(label: str, client, grace_seconds: Optional[float]) -> None:
    """Take the lease, then decide whether it may still be published.

    This coroutine owns the acquire's result. `asyncio.to_thread` cannot be
    cancelled, so whatever happened to the callers waiting on it, the lock that
    comes back is published here or released here - never dropped.
    """
    identity = session_identity(client)
    lock = SessionLock(identity)
    await asyncio.to_thread(
        lock.acquire,
        grace_seconds=DEFAULT_GRACE_SECONDS if grace_seconds is None else grace_seconds,
    )
    # Re-checked AFTER the acquire, which is the only moment it can be checked
    # honestly: the configuration was free to move while the thread blocked.
    if _awaiting_admission.get(label) is not client:
        _release_lock(lock, "the account was reconfigured mid-admission")
        raise AdmissionSuperseded(
            f"Account '{label}' was reconfigured while its session lock was being "
            "taken, so this client is no longer the one that label names. Nothing "
            "was published and the lock was released. Retry the call."
        )
    _awaiting_admission.pop(label, None)
    _publish(label, client, lock, identity)


async def claim_session(label: str, client, grace_seconds: Optional[float] = None) -> None:
    """Take this process's exclusive lock on the client's session. Startup's path.

    Unconditional on purpose. A short-circuit on "this label already has a lock"
    reads like harmless idempotence and is not: the same label handed a SECOND
    client for the same session is exactly the duplicate-connection case the lock
    exists to refuse, and skipping the acquire let it through.
    """
    identity = session_identity(client)
    lock = SessionLock(identity)
    await asyncio.to_thread(
        lock.acquire,
        grace_seconds=DEFAULT_GRACE_SECONDS if grace_seconds is None else grace_seconds,
    )
    _publish(label, client, lock, identity)


async def admit_if_pending(client) -> None:
    """Admit a client before anything is served from it.

    Single-flight: concurrent callers wait on the one task rather than each
    starting an acquire - two acquires on one session fail against each other
    even inside a single process, because a POSIX lock belongs to an open file
    description rather than to a process.

    Shielded: a cancelled CALLER must not cancel the admission, whose thread
    cannot be cancelled anyway and whose lock would then have no owner.
    """
    if not _awaiting_admission:
        return
    task = _start_admission(client)
    if task is None:
        return
    # Left pending on failure, on purpose: the next use tries again, and until
    # then nothing has been served from an unlocked session.
    await asyncio.shield(task)


def transfer_lease(from_label: str, to_label: str, client) -> bool:
    """Move a lease with its client when the same session changes label.

    Releasing and re-acquiring would be a gap in which another process can take
    a session this one is still connected to - for a change that did not touch
    the session at all.
    """
    lease = _leases.get(from_label)
    if lease is None or lease.client is not client:
        return False
    _drop(from_label)
    _publish(to_label, client, lease.lock, lease.identity)
    _awaiting_admission.pop(to_label, None)
    return True


def forget(label: str, closing=None) -> None:
    """Retire a label's lease: stop admitting it, and release once its socket is down.

    ``closing`` is whatever the retirement handed back - an awaitable while the
    disconnect is in flight, or ``None`` when it has already finished. Releasing
    before the socket is down is the window a second process needs to claim a
    session this one is still using, which Telegram answers by invalidating the
    key for both.
    """
    _awaiting_admission.pop(label, None)
    lease = _drop(label)
    if lease is None:
        return
    if closing is None:
        _release_lock(lease.lock, "retirement")
        return
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        _release_lock(lease.lock, "retirement with no loop to wait on")
        return
    task = asyncio.ensure_future(_release_when_closed(lease, closing))
    _releasing.add(task)
    task.add_done_callback(_releasing.discard)


async def _release_when_closed(lease: _Lease, closing) -> None:
    try:
        await asyncio.wait_for(asyncio.shield(_as_future(closing)), _CLOSE_BEFORE_RELEASE_SECONDS)
    except Exception as error:
        # Released anyway, and said out loud. Holding it for the life of the
        # process would lock the operator out of their own account; the risk
        # being accepted is named rather than hidden.
        log_event(
            logging.WARNING,
            "releasing a session lease before its socket confirmed it closed",
            account=lease.label,
            error=error,
        )
    _release_lock(lease.lock, "a confirmed close")


def _as_future(closing):
    if asyncio.isfuture(closing) or asyncio.iscoroutine(closing):
        return asyncio.ensure_future(closing)

    async def _wrap():
        await closing

    return asyncio.ensure_future(_wrap())


async def drain_releases(timeout: float = _CLOSE_BEFORE_RELEASE_SECONDS) -> int:
    """Wait for pending lease releases; return how many did not finish."""
    pending = {t for t in _releasing if not t.done()}
    if not pending:
        return 0
    _done, still_running = await asyncio.wait(pending, timeout=timeout)
    return len(still_running)


def release_all() -> None:
    """Drop every lock this process holds. Shutdown's last step."""
    for lease in list(_leases.values()):
        _release_lock(lease.lock, "shutdown")
    for label, lock in list(session_locks.items()):
        if label not in _leases:
            _release_lock(lock, "shutdown")
    session_locks.clear()
    _leases.clear()
    _awaiting_admission.clear()
    _admitting.clear()


__all__ = [
    "AdmissionSuperseded",
    "_awaiting_admission",
    "admit_if_pending",
    "begin_admission",
    "claim_session",
    "drain_releases",
    "forget",
    "mark_awaiting_admission",
    "reject_duplicate_sessions",
    "release_all",
    "session_locks",
    "transfer_lease",
]
