"""Admitting a reconfigured account as one transaction, or not at all.

`refresh_accounts()` is synchronous and everything admission needs is not:
taking the session lease blocks on a file lock, and connecting and proving the
session is authorized are round trips to Telegram. The first version resolved
that by doing the synchronous half immediately - retire the old client, publish
the new one - and queueing the rest. So a reload that could not complete had
already destroyed the generation that worked:

* a replacement whose session lock was held by another process replaced a
  working client with one that could never connect;
* a replacement that failed authorization did the same;
* duplicate-session validation raising AFTER the clients were built left those
  clients constructed and unowned, with nothing to close them;
* an event-only account added while the server ran was published and never
  connected at all, because admission took its lease and stopped there - there
  is no "first tool call" on a server that only waits for incoming updates.

So the swap happens HERE, at the end, and only when the new client has a lease,
a connection and an authorized session. Until then the previous client keeps
serving. On failure the staged client is disposed and the previous generation is
untouched, which is the property every acceptance case in this area asks for.

This module deliberately does not import `runner` or `connection`: the registry
is handed to it, and the connect/authorize step is plain Telethon. That is what
keeps `connection` (already at its size ceiling) free of it and leaves one owner
for the transaction.
"""

from __future__ import annotations

import asyncio
import time
import logging
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional

from telegram_mcp import admission as _admission
from telegram_mcp.retirement import closure_failed, retire
from telegram_mcp.safe_log import log_event
from telegram_mcp.settings import StartupMessage

# One budget over lease, connect and authorization for a single staged account.
# A reload must not be able to hold a client's replacement open indefinitely.
ADMIT_PHASE_SECONDS: float = 60.0


@dataclass
class Staged:
    """A client built for a label, not yet serving.

    ``previous`` is whatever that label currently means - the client that keeps
    answering until this one is admitted, and that is retired only once it is.
    ``None`` when the account is new.
    """

    label: str
    client: object
    previous: Optional[object] = None
    is_current: Optional[Callable[[], bool]] = None
    on_publish: Optional[Callable[[], None]] = None
    on_failure: Optional[Callable[[BaseException], None]] = None


@dataclass
class Outcome:
    """What one transaction did, for the caller that has to report it."""

    admitted: List[str] = field(default_factory=list)
    rejected: Dict[str, str] = field(default_factory=dict)


# Staged transactions in flight, so shutdown can wait for them and a test can
# see that one was started at all.
_admissions: set = set()
# Deferred (no-loop) and running admissions share this same owned operation.
_pending: dict = {}
_tasks: dict = {}


def _check(staged: Staged) -> None:
    if _admission._stopped or (staged.is_current is not None and not staged.is_current()):
        raise _admission.AdmissionSuperseded(
            f"Account '{staged.label}' activation was superseded or shutdown began"
        )


def supersede(registry: dict, keep=None, is_current=None) -> None:
    """Cancel only candidates invalidated by the caller's new revision token."""
    for key, (one, target, _connect, _grace) in list(_pending.items()):
        if target is not registry:
            continue
        if (keep or {}).get(one.label) is one.client:
            # A cold unchanged candidate is reused, never disconnected merely
            # because another label or a comment changed in the same file.
            one.is_current = is_current
            continue
        if one.is_current is None or one.is_current():
            continue
        task = _tasks.get(key)
        if task is not None and not task.done():
            task.cancel()
        elif task is None:
            _pending.pop(key, None)
            dispose(one.label, one.client, "the desired configuration changed")


def pending_labels(registry: dict) -> set:
    return {
        one.label
        for one, target, _connect, _grace in _pending.values()
        if target is registry and (one.is_current is None or one.is_current())
    }


async def wait_for_client(client) -> bool:
    """Wait for the FULL existing activation, not a competing lease-only claim.

    `asyncio.wait`, not `shield`: a superseded activation is cancelled, and shield
    handed that cancellation to this waiter as if the waiter had been cancelled -
    the tool call then ended with no answer at all (review, 2026-09-30).
    """
    key = id(client)
    operation = _pending.get(key)
    if operation is None:
        return False
    resume(operation[1])
    task = _tasks.get(key)
    if task is None:
        return True
    await asyncio.wait((task,))
    if task.cancelled():
        raise _admission.AdmissionSuperseded(
            f"Account '{operation[0].label}' activation was superseded"
        )
    if not task.result():
        raise StartupMessage(f"Account '{operation[0].label}' could not be activated")
    return True


def _waited_client(registry: dict, label: str):
    if label in registry:
        return registry[label]
    for one, target, _connect, _grace in list(_pending.values()):
        if target is registry and one.label == label:
            return one.client
    return None


async def wait_for_label(registry: dict, label: str) -> None:
    """Wait for whatever activation the label has NOW, following a superseded one."""
    # ponytail: three re-edits of one label during a single wait end in an error.
    for _ in range(3):
        client = _waited_client(registry, label)
        if client is None:
            return
        try:
            await wait_for_client(client)
            return
        except _admission.AdmissionSuperseded:
            continue
    raise StartupMessage(f"Account '{label}' kept changing while this call waited; try again.")


#: How long a label whose activation failed waits before a refresh stages it again,
#: so a broken account is not reconnected on every tool call.
RETRY_SECONDS = 30.0
_retry = {"stamp": None, "not_before": 0.0}


def note_failed(stamp) -> None:
    """A staged label failed: the next refresh after the backoff stages it again."""
    _retry["stamp"], _retry["not_before"] = stamp, time.monotonic() + RETRY_SECONDS


def holding_off(stamp) -> bool:
    return stamp == _retry["stamp"] and time.monotonic() < _retry["not_before"]


# Labels whose admission REACHED the registry in the current transaction. The
# caller advances its record of the active configuration from this and from
# nothing else: a desired revision is not an active one, and recording it before
# a candidate connected is what left a failed account believing it had already
# been replaced - so the next reload compared the file against itself and never
# retried it.
_activated: set = set()


def activated_labels() -> set:
    """The labels that actually became the client their label means."""
    return set(_activated)


# The last configuration revision that was READ and REFUSED, as (stamp, reason).
# Recorded rather than only logged: a reload that quietly did nothing looks
# exactly like one that worked, and an operator who edited `.env` needs to be
# able to ask which it was. The reason is a type and a message - never a value
# from the file, which is where the session strings are.
_rejection: Optional[tuple] = None


def record_rejection(stamp, reason: str) -> None:
    global _rejection
    _rejection = (stamp, reason)


def clear_rejection() -> None:
    global _rejection
    _rejection = None


def last_rejection() -> Optional[tuple]:
    """The revision this process refused, or ``None`` if the last one applied."""
    return _rejection


async def _connect_and_authorize(label: str, client) -> None:
    """Prove the session works before anything is served from it."""
    await client.connect()
    if not await client.is_user_authorized():
        raise StartupMessage(
            f"Account '{label}' is configured but its session is not authorized, so it "
            "was not activated. The account that was serving under that label is "
            "unchanged. Generate a session string with "
            "`uv run session_string_generator.py` and reload."
        )


# How long the transaction waits for a rejected client's socket to go down
# before it reports and moves on. Short: this runs on the failure path and must
# not turn one unreachable account into a stalled reload.
_DISPOSE_SECONDS: float = 5.0


async def _settled(closing) -> None:
    """Wait, bounded, for a disposal to finish. Never raises."""
    if closing is None:
        return
    try:
        await asyncio.wait_for(asyncio.shield(_as_future(closing)), _DISPOSE_SECONDS)
    except BaseException:
        pass


def _as_future(closing):
    if asyncio.isfuture(closing):
        return closing

    async def _wrap():
        await closing

    return asyncio.ensure_future(_wrap())


def dispose(label: str, client, why: str) -> None:
    """Close a staged client this process will not serve from.

    The staged client may already hold things worth releasing even when it never
    became the published one, and a construction that is merely dropped leaks a
    socket and, on a file session, an open SQLite handle.
    """
    outcome = retire(client)
    if closure_failed(outcome):
        log_event(
            logging.WARNING,
            "a staged account could not be closed after it was rejected",
            account=label,
            reason=why,
        )


async def admit(
    staged: Staged,
    registry: Dict[str, object],
    connect: Optional[Callable] = None,
    grace_seconds: Optional[float] = None,
) -> bool:
    """Lease, connect, authorize, then publish - or leave everything as it was.

    Returns whether the staged client is now the one its label means.
    """
    connect = connect or _connect_and_authorize
    label, client = staged.label, staged.client
    _activated.discard(label)
    try:
        async with asyncio.timeout(ADMIT_PHASE_SECONDS):
            _check(staged)
            await _admission.claim_session(label, client, grace_seconds=grace_seconds)
            _check(staged)
            await connect(label, client)
            _check(staged)
    except BaseException as error:
        # The lease first: an acquire that succeeded before the connect failed
        # would otherwise keep a session claimed for a client nobody serves.
        closing = retire(client)
        _admission.forget(label, closing=closing, client=client)
        # AWAITED, bounded, as part of the transaction. Scheduling the disconnect
        # and returning leaves the caller unable to say whether the staged client
        # was disposed of - and "dispose every staged resource" is the property
        # this path exists for, not a task queued behind the answer.
        await _settled(closing)
        if staged.previous is None and registry.get(label) is client:
            registry.pop(label, None)
        if registry.get(label) is staged.previous:
            _admission.restore_active(label, staged.previous)
        if staged.on_failure is not None:
            staged.on_failure(error)
        if isinstance(error, asyncio.CancelledError):
            raise
        log_event(
            logging.WARNING,
            "account not activated; the previous client for this label is unchanged",
            account=label,
            error=error,
        )
        return False

    # Published last, and only now. Everything above can fail; from here the new
    # client has a lease, a socket and an authorized session.
    registry[label] = client
    _activated.add(label)
    if staged.on_publish is not None:
        staged.on_publish()
    if staged.previous is not None and staged.previous is not client:
        # The old one goes only once its replacement is actually serving. Its
        # lease is NOT gone: publishing the replacement retired it rather than
        # overwriting it, so it is still held and still owned, and handing the
        # close to `forget` is what releases it - once the socket it protects is
        # confirmed down, and not before.
        outcome = retire(staged.previous)
        _admission.forget(label, closing=outcome, client=staged.previous)
        if closure_failed(outcome):
            log_event(
                logging.WARNING,
                "the replaced client for this account did not close",
                account=label,
            )
    return True


async def admit_all(
    staged: List[Staged],
    registry: Dict[str, object],
    connect: Optional[Callable] = None,
    grace_seconds: Optional[float] = None,
) -> Outcome:
    """Admit each staged account independently. One failure never blocks another."""
    outcome = Outcome()
    results = await asyncio.gather(
        *(admit(one, registry, connect, grace_seconds) for one in staged),
        return_exceptions=True,
    )
    for one, result in zip(staged, results):
        if result is True:
            outcome.admitted.append(one.label)
        else:
            outcome.rejected[one.label] = (
                f"{type(result).__name__}: {result}"
                if isinstance(result, BaseException)
                else "not admitted"
            )
    return outcome


def _launch(key: int) -> asyncio.Task:
    existing = _tasks.get(key)
    if existing is not None:
        return existing
    operation = _pending[key]
    one, registry, connect, grace = operation
    task = asyncio.create_task(admit(one, registry, connect, grace))
    _tasks[key] = task

    def done(completed):
        if _pending.get(key) is operation:
            _pending.pop(key, None)
        if _tasks.get(key) is completed:
            _tasks.pop(key, None)
        if not completed.cancelled() and completed.exception() is not None:
            log_event(logging.ERROR, "account activation failed", error=completed.exception())

    task.add_done_callback(done)
    return task


def _collect(tasks):
    async def collect():
        return await asyncio.gather(*tasks, return_exceptions=True)

    task = asyncio.create_task(collect())
    _admissions.add(task)
    task.add_done_callback(_admissions.discard)
    return task


def resume(registry: dict) -> None:
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return
    new = [key for key, op in _pending.items() if op[1] is registry and key not in _tasks]
    if new:
        _collect([_launch(key) for key in new])


def begin(staged, registry, connect=None, grace_seconds=None) -> Optional[asyncio.Task]:
    """Own candidates now; resume deferred cold discovery on the serving loop."""
    if not staged:
        return None
    for one in staged:
        _activated.discard(one.label)
        _pending[id(one.client)] = (one, registry, connect, grace_seconds)
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return None
    return _collect([_launch(id(one.client)) for one in staged])


def stop(registry: dict) -> None:
    """Stop admission before any serving resource is torn down."""
    for key, (one, target, _connect, _grace) in list(_pending.items()):
        if target is not registry:
            continue
        task = _tasks.get(key)
        if task is None:
            _pending.pop(key, None)
            dispose(one.label, one.client, "shutdown")
        elif not task.done():
            task.cancel()


async def drain(timeout: float = ADMIT_PHASE_SECONDS) -> int:
    """Wait for staged admissions to settle; return how many did not."""
    pending = {t for t in _admissions if not t.done()}
    if not pending:
        return 0
    _done, still_running = await asyncio.wait(pending, timeout=timeout)
    return len(still_running)


__all__ = [
    "ADMIT_PHASE_SECONDS",
    "Outcome",
    "Staged",
    "admit",
    "admit_all",
    "begin",
    "dispose",
    "drain",
]
