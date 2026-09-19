"""A reload records as ACTIVE only what actually became active, and asks once.

Two defects, both reachable on the ordinary hot-reload path, both about the gap
between "the operator wants this" and "this is now serving".

**The digests moved too early.** `refresh_accounts` set `_env_digests` to the
DESIRED revision on the line before it started the transaction that might fail.
`_replaced()` compares against those digests, so after a failed admission the
server believes the new session is in force while the old client is the one
answering - and the next reload, comparing the file against itself, finds nothing
to do. The account that failed never retries. Nothing surfaces it: the client
still works, it is simply the wrong one, and the reload that should have noticed
is the thing that was told not to.

**The same client was handed to two acquisition routes.** A newly added account
is published into the registry AND staged, so `account_lifecycle.admit` calls
`claim_session` for it while `mark_awaiting_admission` leaves it pending for
`admit_if_pending` to admit again. Two acquires on one session conflict inside a
single process - a POSIX lock belongs to an open file description, which is the
reason `admission`'s own docstring gives for single-flighting - so the second one
waits out the grace period and fails, on whichever tool call happens to be next.
"""

import asyncio

import pytest

from telegram_mcp import account_lifecycle as lifecycle
from telegram_mcp import admission, connection


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


class _Lock:
    def __init__(self):
        self.released = 0

    def release(self):
        self.released += 1


@pytest.fixture(autouse=True)
def _clean(monkeypatch):
    admission.begin_serving()
    admission.session_locks.clear()
    admission._active.clear()
    admission._retiring.clear()
    admission._awaiting_admission.clear()
    admission._admitting.clear()
    yield
    lifecycle._admissions.clear()
    admission.session_locks.clear()
    admission._active.clear()
    admission._retiring.clear()
    admission._awaiting_admission.clear()


# --- the digests must not move ahead of what activated ------------------------


@pytest.mark.asyncio
async def test_a_failed_admission_does_not_record_its_session_as_active(monkeypatch):
    """The whole point. If the digest moves anyway, `_replaced` compares the file
    against itself on the next reload and the failed account is never retried."""
    old = _Client("old")
    registry = {"work": old}
    fresh = _Client("new")

    async def refuses(label, client):
        raise OSError("connect refused")

    async def claims(label, client, grace_seconds=None, **_):
        admission._publish(label, client, _Lock(), f"id:{label}")

    monkeypatch.setattr(admission, "claim_session", claims)

    ok = await lifecycle.admit(
        lifecycle.Staged("work", fresh, previous=old), registry, connect=refuses
    )

    assert ok is False
    assert registry["work"] is old
    assert "work" not in lifecycle.activated_labels(), (
        "a client that never connected was reported as activated, so the caller "
        "would record its session as the one in force"
    )


@pytest.mark.asyncio
async def test_a_successful_admission_is_reported_as_activated(monkeypatch):
    old = _Client("old")
    registry = {"work": old}
    fresh = _Client("new")

    async def connects(label, client):
        await client.connect()

    async def claims(label, client, grace_seconds=None, **_):
        admission._publish(label, client, _Lock(), f"id:{label}")

    monkeypatch.setattr(admission, "claim_session", claims)

    ok = await lifecycle.admit(
        lifecycle.Staged("work", fresh, previous=old), registry, connect=connects
    )

    assert ok is True
    assert registry["work"] is fresh
    assert "work" in lifecycle.activated_labels()


def test_the_reload_keeps_the_serving_digest_until_a_candidate_activates(monkeypatch):
    """`_replaced` reads `_env_digests`. Until the candidate is serving, the
    digest that belongs there is the one the SERVING client was built from."""
    monkeypatch.setattr(connection, "_env_digests", {"TELEGRAM_SESSION_STRING_WORK": "old"})

    connection.record_activated({}, {"TELEGRAM_SESSION_STRING_WORK": "new"}, set())

    assert connection._env_digests == {"TELEGRAM_SESSION_STRING_WORK": "old"}


def test_an_activated_label_moves_its_digest_and_only_its_own(monkeypatch):
    monkeypatch.setattr(
        connection,
        "_env_digests",
        {"TELEGRAM_SESSION_STRING_WORK": "old", "TELEGRAM_SESSION_STRING_SPARE": "keep"},
    )

    connection.record_activated(
        {},
        {"TELEGRAM_SESSION_STRING_WORK": "new", "TELEGRAM_SESSION_STRING_SPARE": "moved"},
        {"work"},
    )

    assert connection._env_digests["TELEGRAM_SESSION_STRING_WORK"] == "new"
    assert (
        connection._env_digests["TELEGRAM_SESSION_STRING_SPARE"] == "keep"
    ), "an account nobody admitted had its digest advanced"


# --- one client, one acquisition route ----------------------------------------


@pytest.mark.asyncio
async def test_a_claimed_client_is_no_longer_pending_admission(monkeypatch):
    """Two acquires on one session conflict inside a single process, so a client
    that `claim_session` already took the lease for must not still be queued for
    `admit_if_pending` to take it again."""
    client = _Client("new")
    admission.mark_awaiting_admission({"work": client})

    monkeypatch.setattr(admission, "SessionLock", lambda identity: _Lock())
    monkeypatch.setattr(admission, "session_identity", lambda c: f"id:{id(c)}")
    monkeypatch.setattr(_Lock, "acquire", lambda self, **kw: None, raising=False)

    await admission.claim_session("work", client)

    assert "work" in admission.session_locks
    assert (
        admission._awaiting_admission.get("work") is not client
    ), "the client still waits for a second acquisition of a lease it already holds"


@pytest.mark.asyncio
async def test_admitting_afterwards_is_a_no_op_rather_than_a_second_acquire(monkeypatch):
    client = _Client("new")
    admission.mark_awaiting_admission({"work": client})
    monkeypatch.setattr(admission, "SessionLock", lambda identity: _Lock())
    monkeypatch.setattr(admission, "session_identity", lambda c: f"id:{id(c)}")
    monkeypatch.setattr(_Lock, "acquire", lambda self, **kw: None, raising=False)
    await admission.claim_session("work", client)

    # Whatever a later tool call does, it must not start a second acquire.
    await asyncio.wait_for(admission.admit_if_pending(client), timeout=2)

    assert "work" in admission.session_locks
