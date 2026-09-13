"""Warming a client's entity cache once, and letting everyone who needs it wait.

`StringSession` keeps no persistent entity cache, so the first lookup of a chat
raises `ValueError` until `get_dialogs()` has run. That warm is expensive and
must not run per lookup, which is what the TTL is for - but a TTL alone gets two
things wrong, and both were live:

* **The stamp was written before the call, not after.** A warm that failed or
  was cancelled still suppressed every retry for the whole window, so one
  transient error made every cold lookup fail for thirty seconds.
* **A concurrent caller saw that stamp and skipped.** Two cold lookups at once
  meant one warmed and the other was told a warm had just happened, declined to
  retry, and failed against a cache that was still cold.

So the in-flight warm is an owned task that waiters share, and the stamp is
written only on success. Waiting is `shield`ed: a tool call that gives up must
not cancel the warm the other waiters are relying on.

Shielding the WAIT, though, is not the same as owning the WORK, and the first
version did only the former. `get_dialogs()` had no deadline of its own, so a
warm that never returned was shared by every later caller for the life of the
process - and because nothing cancelled it, it outlived the retirement of the
very client it was warming. Both are closed here: the call is bounded, and
retirement and shutdown cancel and drain what they own.

The failure is kept too. A warm that failed used to leave the caller reporting
whatever it found next - usually "peer not found", which describes a cache that
was never filled rather than the timeout that stopped it being filled.
"""

import asyncio
import time
import weakref

# Per client, when its cache was last warmed SUCCESSFULLY. Weak, so a retired
# client's entry goes with it rather than pinning the client forever.
_dialog_warmed: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()

# Per client, the warm currently running. A plain dict, because the task holds a
# strong reference to the client and a weak key could never be collected; the
# entry is removed as soon as the task finishes.
_dialog_warms: dict = {}

_DIALOG_WARM_SECONDS = 30.0

# How long one warm may take. `get_dialogs()` is a network call with no deadline
# of its own, and an unbounded one is shared by every later caller forever.
_DIALOG_WARM_TIMEOUT = 60.0

# Per client, why its last warm failed. Kept so a caller can say "the cache
# could not be filled because X" instead of reporting the missing peer that is
# merely the consequence.
_dialog_warm_errors: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()


async def _warm_dialogs(client) -> None:
    try:
        await asyncio.wait_for(client.get_dialogs(), timeout=_DIALOG_WARM_TIMEOUT)
    except BaseException as error:
        _dialog_warm_errors[client] = error
        raise
    _dialog_warm_errors.pop(client, None)
    # AFTER it completed, never before.
    _dialog_warmed[client] = time.monotonic()


def last_warm_error(client):
    """Why this client's cache is still cold, when something is known."""
    return _dialog_warm_errors.get(client)


def cancel_warm(client) -> None:
    """Stop warming a client nobody serves from any more.

    A shielded wait protects the warm from ITS CALLERS, which is right, and left
    nothing able to stop it when the client itself was retired - so the task
    went on holding a reference to a disconnected client and calling into it.
    """
    task = _dialog_warms.pop(client, None)
    if task is not None and not task.done():
        task.cancel()


async def drain_warms(timeout: float = 5.0) -> int:
    """Cancel every warm still running and wait briefly; return how many remain."""
    pending = {t for t in _dialog_warms.values() if not t.done()}
    _dialog_warms.clear()
    if not pending:
        return 0
    for task in pending:
        task.cancel()
    _done, still_running = await asyncio.wait(pending, timeout=timeout)
    return len(still_running)


def _forget_warm(client, task) -> None:
    if _dialog_warms.get(client) is task:
        _dialog_warms.pop(client, None)


async def warm_dialogs_once(client) -> bool:
    """Warm the entity cache at most once per `_DIALOG_WARM_SECONDS` per client.

    Returns True when the cache was warmed - by this call or by one it waited on
    - so a caller knows a retry can see something new. False means the cache is
    unchanged and asking again would get the same answer.
    """
    last = _dialog_warmed.get(client)
    if last is not None and time.monotonic() - last < _DIALOG_WARM_SECONDS:
        return False

    task = _dialog_warms.get(client)
    if task is None or task.done():
        task = asyncio.ensure_future(_warm_dialogs(client))
        _dialog_warms[client] = task
        task.add_done_callback(lambda finished, c=client: _forget_warm(c, finished))

    try:
        await asyncio.shield(task)
    except asyncio.CancelledError:
        # This caller is going away; the warm is not. Propagate, never swallow.
        raise
    except Exception:
        # Failed, so nothing was stamped and the next caller tries again.
        return False
    return True


__all__ = [
    "_DIALOG_WARM_SECONDS",
    "_DIALOG_WARM_TIMEOUT",
    "_dialog_warm_errors",
    "_dialog_warmed",
    "_dialog_warms",
    "cancel_warm",
    "drain_warms",
    "last_warm_error",
    "warm_dialogs_once",
]
