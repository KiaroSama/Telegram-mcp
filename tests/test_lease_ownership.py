"""A session lease is held until the socket under it is confirmed down.

The lease is what stops a SECOND process connecting a session this one still
holds, and Telegram answers a session connected twice by invalidating the auth
key for both. So every path that gives the lease up has to be able to tell
"closed" from "I do not know", and only the first may release.

Three ways that came apart, each with its own counterexample below:

* **`retire()` reported a refused disconnect as `None`** - the same value it
  returns for "already closed". The caller released the lease on a socket that
  was never closed.
* **The release fired anyway after its own timeout.** A disconnect that had not
  finished in ten seconds still ended with `lock.release()`, which is the exact
  window the lease exists to close.
* **A late acquire published after shutdown.** `release_all()` cleared the
  table, and an admission still blocked in its thread then published a lock
  nothing would ever release.
"""

import asyncio

import pytest

from telegram_mcp import admission, retirement


class _Lock:
    """A stand-in for the real POSIX lock, which records what happened to it."""

    def __init__(self):
        self.released = 0

    def release(self):
        self.released += 1


@pytest.fixture(autouse=True)
def _clean():
    admission.release_all()
    admission._stopped = False
    admission.unreleased_leases.clear()
    yield
    for task in list(admission._releasing):
        task.cancel()
    admission._releasing.clear()
    admission.release_all()
    admission._stopped = False
    admission.unreleased_leases.clear()


def _lease(label="work", lock=None):
    lock = lock or _Lock()
    lease = admission._Lease(label=label, client=object(), lock=lock, identity="id:" + label)
    admission.session_locks[label] = lock
    admission._leases[label] = lease
    return lease


# --- retire() has to say which of the three things happened -------------------


def test_a_disconnect_that_refuses_is_not_reported_as_closed():
    """`return None` meant "nothing left to wait for", and a refusal is the
    opposite: the socket is still up and the lease must not be given up."""

    class _Refuses:
        def disconnect(self):
            raise OSError("the transport refused to close")

    outcome = retirement.retire(_Refuses())

    assert outcome is not None, "a refused disconnect looked exactly like a completed one"
    assert retirement.closure_failed(outcome) is True


def test_a_client_already_closed_is_reported_as_nothing_to_wait_for():
    class _AlreadyClosed:
        def disconnect(self):
            return None

    assert retirement.retire(_AlreadyClosed()) is None


@pytest.mark.asyncio
async def test_a_disconnect_in_flight_is_handed_back_to_be_waited_on():
    class _Slow:
        def disconnect(self):
            return asyncio.sleep(0)

    outcome = retirement.retire(_Slow())

    assert outcome is not None and not retirement.closure_failed(outcome)
    await asyncio.wait_for(asyncio.shield(outcome), timeout=1)


# --- the release itself -------------------------------------------------------


@pytest.mark.asyncio
async def test_a_lease_is_not_released_when_the_close_never_finishes(monkeypatch):
    """The defect: the release ran in the `finally` sense - timeout included -
    so a socket that never closed still gave its session away."""
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.05)
    lock = _Lock()
    lease = _lease(lock=lock)

    await admission._release_when_closed(lease, asyncio.get_running_loop().create_future())

    assert lock.released == 0, "the lease was released over a socket that never closed"
    assert "work" in admission.unreleased_leases


@pytest.mark.asyncio
async def test_a_lease_is_not_released_when_the_close_failed(monkeypatch):
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 1)
    lock = _Lock()
    lease = _lease(lock=lock)
    failed = asyncio.get_running_loop().create_future()
    failed.set_exception(OSError("the transport refused to close"))

    await admission._release_when_closed(lease, failed)

    assert lock.released == 0
    assert "work" in admission.unreleased_leases


@pytest.mark.asyncio
async def test_a_confirmed_close_does_release_the_lease(monkeypatch):
    """The other half: holding it forever would lock the operator out of their
    own account, so a close that DID finish must still give the session back."""
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 1)
    lock = _Lock()
    lease = _lease(lock=lock)
    closed = asyncio.get_running_loop().create_future()
    closed.set_result(None)

    await admission._release_when_closed(lease, closed)

    assert lock.released == 1
    assert admission.unreleased_leases == {}


@pytest.mark.asyncio
async def test_a_refused_retirement_reaches_forget_as_a_failure(monkeypatch):
    """End to end through the caller `refresh_accounts` uses."""
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.05)

    class _Refuses:
        def disconnect(self):
            raise OSError("the transport refused to close")

    lock = _Lock()
    _lease(lock=lock)

    admission.forget("work", closing=retirement.retire(_Refuses()))
    await admission.drain_releases(timeout=1)

    assert lock.released == 0, "a refused disconnect still handed the session away"
    assert "work" in admission.unreleased_leases


# --- the stop boundary --------------------------------------------------------


def test_a_lease_cannot_be_published_after_shutdown():
    """`release_all()` is the boundary. An acquire still blocked in its thread
    published afterwards, and nothing was left to release it."""
    admission.release_all()
    lock = _Lock()

    admission._publish("late", object(), lock, "id:late")

    assert "late" not in admission.session_locks
    assert lock.released == 1, "the late lock was neither published nor released"


def test_publication_works_again_once_the_process_is_serving():
    admission.release_all()
    admission._stopped = False
    lock = _Lock()

    admission._publish("work", object(), lock, "id:work")

    assert admission.session_locks["work"] is lock
    assert lock.released == 0
