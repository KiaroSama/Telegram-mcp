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

Those three were not enough either, because the store underneath them was still
keyed by label. Five more ways it came apart, all of them reachable:

* **A replacement overwrote its predecessor's lease.** The entry was replaced and
  the old `_Lease` was the only reference to its `SessionLock`, so the previous
  client - still connected - lost its lock to the next collection.
* **An unconfirmed close recorded a string and dropped the lease.** The comment
  said the lock was still held. It was held until the collector ran.
* **`forget(label)` released whatever sat at the label**, so a failed replacement
  retired the client that was still working.
* **A refusal at the stop boundary returned normally**, and the caller connected
  believing it held a lease.
* **A cancelled caller left the acquire unowned**, so a real lock came back with
  nothing holding or releasing it.

What holds now: a lease's identity is the CLIENT that took it, never the label.
`_active` is what each label currently means; `_retiring` holds every lease that
is no longer active - one closing, one whose close never confirmed - and it is
the strong reference, because `SessionLock` wraps an open file handle and a lease
nothing refers to is a lock the operating system has already given back. Two
leases under one label is the normal state of a replacement, not a bug: the
predecessor keeps its own lock until its own socket is confirmed down.
"""

from __future__ import annotations

import asyncio
import functools
import logging
from dataclasses import dataclass
from typing import Callable, Dict, Optional

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

# Label -> the lease currently SERVING that label. One per label.
_active: Dict[str, _Lease] = {}

# Every lease that is no longer the active one: a client that is closing, and a
# client whose close never confirmed. Keyed by `id(lease)` because one label can
# have several at once - a replacement's predecessor sits here while the
# replacement serves.
#
# **This dict is the strong reference, and that is its whole job.** `SessionLock`
# holds a plain open file object, and both `flock` and `msvcrt.locking` release
# when the handle closes. A lease nothing refers to is therefore a lock the
# operating system has already given back, whatever the code around it says. The
# previous version recorded an error STRING and dropped the lease, under a comment
# claiming the lock was still held; it was held until the next collection.
_retiring: Dict[int, _Lease] = {}

# Labels a reload published whose lease has not been taken yet.
_awaiting_admission: Dict[str, object] = {}

# client -> the one task admitting it. Single-flight: a second caller waits on
# the first task rather than starting a competing acquire.
_admitting: Dict[int, asyncio.Task] = {}

# Releases waiting on a socket to finish closing, so shutdown can wait for them.
_releasing: set = set()

# Leases whose socket never confirmed it closed. The lock is STILL HELD - giving
# it back would let a second process connect a session this one may still have
# open, which Telegram answers by invalidating the key for both. Kept as an owned
# record so shutdown can report what it could not account for, keyed by label
# with the reason.
unreleased_leases: Dict[str, str] = {}

# Set by `release_all()`. An acquire blocked in its thread when shutdown ran must
# not publish afterwards: nothing would be left to release it, and the session
# would stay claimed for the life of the process.
_stopped: bool = False


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


def _publish(label: str, client, lock: SessionLock, identity: str) -> _Lease:
    """Make this client's lease the one `label` means, and RETURN it.

    Raises rather than returning quietly when shutdown has begun: the previous
    version released the lock and returned `None`, so `claim_session` returned
    normally and its caller went on to connect believing it held a lease.
    """
    if _stopped:
        # Shutdown already ran. `asyncio.to_thread` cannot be cancelled, so an
        # acquire begun before the boundary still returns a real lock afterwards;
        # publishing it here would leave a held session with no owner to release.
        _release_lock(lock, "an admission that finished after shutdown")
        raise AdmissionSuperseded(
            f"Account '{label}' finished taking its session lock after this process "
            "began shutting down, so it was not published and the lock was released. "
            "Nothing connected."
        )
    previous = _active.get(label)
    if previous is not None and previous.client is not client:
        # NOT overwritten. The previous client may still be connected - during a
        # replacement it certainly is - and dropping the last reference to its
        # lease hands its session to whoever asks next. It retires instead, and
        # its own retirement releases it once its socket is confirmed down.
        _retire_lease(previous)
    session_locks[label] = lock
    lease = _Lease(label=label, client=client, lock=lock, identity=identity)
    _active[label] = lease
    return lease


def _drop(label: str) -> Optional[_Lease]:
    """Unpublish the active lease at this label. The lease itself is returned, not
    discarded - the caller decides whether it retires or is released."""
    session_locks.pop(label, None)
    return _active.pop(label, None)


def _retire_lease(lease: _Lease) -> None:
    """Move a lease out of service while keeping it - and its lock - owned."""
    _retiring[id(lease)] = lease


def _lease_of(label: str, client) -> Optional[_Lease]:
    """The lease this client holds under this label, active or retiring.

    Resolved by OWNER. `forget("work")` used to mean "whatever lease is at work",
    which is the wrong question once a label can mean a different client than it
    did when the lock was taken - a failed replacement retired the client that was
    still serving.
    """
    active = _active.get(label)
    if active is not None and active.client is client:
        return active
    for lease in _retiring.values():
        if lease.client is client and lease.label == label:
            return lease
    return None


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


async def claim_session(
    label: str,
    client,
    grace_seconds: Optional[float] = None,
    on_wait: Optional[Callable[[float], None]] = None,
) -> None:
    """Take this process's exclusive lock on the client's session. Startup's path.

    Unconditional on purpose. A short-circuit on "this label already has a lock"
    reads like harmless idempotence and is not: the same label handed a SECOND
    client for the same session is exactly the duplicate-connection case the lock
    exists to refuse, and skipping the acquire let it through.

    ``on_wait`` is handed straight to the lock: it fires only when this actually
    has to wait for another instance, which is the one case startup owes the
    operator a sentence about.
    """

    async def owned() -> None:
        # The acquire's result belongs to THIS coroutine, never to the caller.
        # `asyncio.to_thread` cannot be cancelled, so a cancelled caller used to
        # leave a real lock with nothing holding or releasing it; here the lock
        # is published or released before this returns, whatever happened above.
        identity = session_identity(client)
        lock = SessionLock(identity)
        acquired = False
        try:
            await asyncio.to_thread(
                functools.partial(
                    lock.acquire,
                    grace_seconds=(
                        DEFAULT_GRACE_SECONDS if grace_seconds is None else grace_seconds
                    ),
                    on_wait=on_wait,
                )
            )
            acquired = True
            _publish(label, client, lock, identity)
        except BaseException:
            if acquired:
                _release_lock(lock, "a claim that could not be published")
            raise

    task = asyncio.ensure_future(owned())
    # Shielded: cancelling the caller must not cancel the owner, or the lock the
    # thread is about to return has no one to dispose of it.
    await asyncio.shield(task)


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
    lease = _active.get(from_label)
    if lease is None or lease.client is not client:
        return False
    if to_label in _active and _active[to_label].client is not client:
        # Another client already means the destination name. Taking its place
        # here would release its lock through the same gap this function exists
        # to avoid, so the rename is refused rather than half-applied.
        return False
    _drop(from_label)
    _publish(to_label, client, lease.lock, lease.identity)
    _awaiting_admission.pop(to_label, None)
    return True


def forget(label: str, closing=None, *, client=None) -> bool:
    """Retire ONE client's lease: stop admitting it, release once its socket is down.

    ``client`` names whose lease this is. Without it the active lease at the label
    is taken, which is only correct when the caller genuinely means "whatever is
    serving here" - every caller that knows which client it is retiring passes it,
    because "release the lock for label X" is the wrong question once X can mean a
    different client than it did when the lock was taken.

    ``closing`` is whatever the retirement handed back - an awaitable while the
    disconnect is in flight, or ``None`` when it has already finished. Releasing
    before the socket is down is the window a second process needs to claim a
    session this one is still using, which Telegram answers by invalidating the
    key for both.

    Returns whether a lease was actually retired, so a caller can see that its
    retirement did not apply rather than assuming it did.
    """
    _awaiting_admission.pop(label, None)
    if client is None:
        lease = _drop(label)
    else:
        lease = _lease_of(label, client)
        if lease is not None and _active.get(label) is lease:
            _drop(label)
    if lease is None:
        return False
    _retiring.pop(id(lease), None)
    if closing is None:
        _release_lock(lease.lock, "retirement")
        unreleased_leases.pop(lease.label, None)
        return True
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        _release_lock(lease.lock, "retirement with no loop to wait on")
        unreleased_leases.pop(lease.label, None)
        return True
    # Owned while it waits: the lease is back in `_retiring` for the whole of the
    # close, so nothing can collect the lock out from under a socket that is
    # still open.
    _retire_lease(lease)
    task = asyncio.ensure_future(_release_when_closed(lease, closing))
    _releasing.add(task)
    task.add_done_callback(_releasing.discard)
    return True


async def _release_when_closed(lease: _Lease, closing) -> None:
    """Release the lease only once the socket it protects is CONFIRMED down.

    The earlier version released on every path, timeout included. That is the
    precise window the lease exists to close: a disconnect that had not finished
    handed the session to whoever asked next, while this process may still have
    had it open - and a session connected twice is an auth key Telegram
    invalidates for both ends.

    So an unconfirmed close keeps the lock and records the label instead. The
    cost is a session this process will not reuse until it restarts, which is
    recoverable; the alternative is not.
    """
    try:
        await asyncio.wait_for(asyncio.shield(_as_future(closing)), _CLOSE_BEFORE_RELEASE_SECONDS)
    except Exception as error:
        # The lease STAYS in `_retiring`. That is what keeps the lock: recording
        # the reason is for the operator, and a string in a dict holds nothing.
        _retire_lease(lease)
        unreleased_leases[lease.label] = f"{type(error).__name__}: {error}"
        log_event(
            logging.WARNING,
            "keeping a session lease: its socket never confirmed it closed",
            account=lease.label,
            error=error,
        )
        return
    _retiring.pop(id(lease), None)
    unreleased_leases.pop(lease.label, None)
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


def begin_serving() -> None:
    """Open the door `release_all` closed. Startup's first step.

    The stop boundary is sticky on purpose - a shutdown must not be undone by a
    task that was merely slow - so something has to state that this process is
    serving again. Only startup does, which is why no admission path calls it.
    """
    global _stopped
    _stopped = False


def release_all() -> None:
    """Drop every lock this process holds, and close the door behind it.

    Shutdown's last step. The `_stopped` flag is the half that was missing: an
    acquire already blocked in its thread cannot be cancelled, so without a
    boundary it published a lock after this ran and nothing was left to release
    it.
    """
    global _stopped
    _stopped = True
    for lease in list(_active.values()):
        _release_lock(lease.lock, "shutdown")
    for label, lock in list(session_locks.items()):
        if label not in _active:
            _release_lock(lock, "shutdown")
    # The retiring ones last, and their reasons are left standing: a lease whose
    # close never confirmed is what `unreleased_leases` names, and the report is
    # the only place an operator learns which sessions this process still had.
    # The handles go with the process either way; the record is what survives.
    for lease in list(_retiring.values()):
        _release_lock(lease.lock, "shutdown")
    session_locks.clear()
    _active.clear()
    _retiring.clear()
    _awaiting_admission.clear()
    _admitting.clear()


__all__ = [
    "AdmissionSuperseded",
    "_awaiting_admission",
    "admit_if_pending",
    "begin_admission",
    "begin_serving",
    "claim_session",
    "drain_releases",
    "forget",
    "mark_awaiting_admission",
    "reject_duplicate_sessions",
    "release_all",
    "session_locks",
    "transfer_lease",
    "unreleased_leases",
]
