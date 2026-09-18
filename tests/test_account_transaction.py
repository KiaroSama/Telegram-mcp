"""A reconfigured account is admitted as one transaction, or not at all.

The generation that works keeps working until a replacement has a lease, a
socket and an authorized session. Everything below is a way that used not to
hold, and each one costs the same thing: an account that stops answering because
a reload could not finish.

Note what is NOT doubled here. The registry, the staging, the lease bookkeeping
and the ordering are the real modules; only Telegram itself and the POSIX lock
are stood in for, because neither can be reached from a test and neither is what
these cases are about.
"""

import asyncio

import pytest

from telegram_mcp import account_lifecycle as lifecycle
from telegram_mcp import admission


class _Client:
    def __init__(self, name):
        self.name = name
        self.connected = False
        self.disconnected = False

    async def connect(self):
        self.connected = True

    async def is_user_authorized(self):
        return True

    async def disconnect(self):
        self.disconnected = True


async def _claims(label, client, grace_seconds=None):
    admission.session_locks[label] = object()
    return None


@pytest.fixture(autouse=True)
def _clean(monkeypatch):
    admission.begin_serving()
    admission.session_locks.clear()
    admission._leases.clear()
    admission._awaiting_admission.clear()
    monkeypatch.setattr(admission, "claim_session", _claims)
    yield
    lifecycle._admissions.clear()
    admission.session_locks.clear()
    admission._leases.clear()


@pytest.mark.asyncio
async def test_an_admitted_replacement_takes_over_and_the_old_client_is_retired():
    registry = {"work": _Client("old")}
    fresh = _Client("new")

    ok = await lifecycle.admit(
        lifecycle.Staged("work", fresh, previous=registry["work"]), registry
    )

    assert ok is True
    assert registry["work"] is fresh
    assert fresh.connected, "the replacement was published without ever connecting"


@pytest.mark.asyncio
async def test_a_replacement_that_cannot_connect_changes_nothing():
    old = _Client("old")
    registry = {"work": old}
    fresh = _Client("new")

    async def _refuses(label, client):
        raise OSError("connect refused")

    ok = await lifecycle.admit(
        lifecycle.Staged("work", fresh, previous=old), registry, connect=_refuses
    )

    assert ok is False
    assert registry["work"] is old, "a client that never connected replaced a working one"
    assert not old.disconnected
    assert fresh.disconnected, "the staged client was left holding its socket"
    assert "work" not in admission.session_locks, "a rejected client kept the session lease"


@pytest.mark.asyncio
async def test_a_replacement_whose_session_is_not_authorized_changes_nothing():
    """Different failure, same requirement: the lease is taken, and the session
    then turns out to be one Telegram no longer accepts."""
    old = _Client("old")
    registry = {"work": old}
    fresh = _Client("new")
    fresh.is_user_authorized = lambda: _false()

    ok = await lifecycle.admit(lifecycle.Staged("work", fresh, previous=old), registry)

    assert ok is False
    assert registry["work"] is old
    assert "work" not in admission.session_locks


async def _false():
    return False


@pytest.mark.asyncio
async def test_a_held_lease_does_not_take_the_working_client_down():
    """The acceptance case named in the audit: another process holds the lock."""
    old = _Client("old")
    registry = {"work": old}
    fresh = _Client("new")

    async def _held(label, client, grace_seconds=None):
        raise TimeoutError("another process holds this session")

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(admission, "claim_session", _held)
        ok = await lifecycle.admit(lifecycle.Staged("work", fresh, previous=old), registry)

    assert ok is False
    assert registry["work"] is old
    assert not old.disconnected


@pytest.mark.asyncio
async def test_one_account_failing_does_not_block_another():
    registry = {}
    good, bad = _Client("good"), _Client("bad")

    async def _connect(label, client):
        if client is bad:
            raise OSError("no")

    outcome = await lifecycle.admit_all(
        [lifecycle.Staged("good", good), lifecycle.Staged("bad", bad)],
        registry,
        connect=_connect,
    )

    assert outcome.admitted == ["good"]
    assert "bad" in outcome.rejected
    assert registry == {"good": good}


@pytest.mark.asyncio
async def test_an_admission_is_bounded(monkeypatch):
    """A reload must not be able to hold a replacement open for ever."""
    monkeypatch.setattr(lifecycle, "ADMIT_PHASE_SECONDS", 0.05)
    old = _Client("old")
    registry = {"work": old}

    async def _never(label, client):
        await asyncio.get_running_loop().create_future()

    ok = await asyncio.wait_for(
        lifecycle.admit(lifecycle.Staged("work", _Client("new"), previous=old), registry, _never),
        timeout=3,
    )

    assert ok is False
    assert registry["work"] is old


@pytest.mark.asyncio
async def test_a_cancelled_transaction_still_disposes_what_it_staged():
    old = _Client("old")
    registry = {"work": old}
    fresh = _Client("new")
    started = asyncio.Event()

    async def _hangs(label, client):
        started.set()
        await asyncio.get_running_loop().create_future()

    task = asyncio.ensure_future(
        lifecycle.admit(lifecycle.Staged("work", fresh, previous=old), registry, _hangs)
    )
    await asyncio.wait_for(started.wait(), timeout=2)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task

    assert registry["work"] is old
    assert fresh.disconnected, "a cancelled admission left its staged client connected"


def test_nothing_is_started_without_a_loop():
    """`refresh_accounts` is synchronous. With no loop the safe answer is to
    leave the previous generation serving, never to publish a half-admitted one."""
    registry = {"work": _Client("old")}

    assert lifecycle.begin([lifecycle.Staged("work", _Client("new"))], registry) is None
    assert registry["work"].name == "old"
