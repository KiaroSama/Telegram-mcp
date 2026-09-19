"""A session lease is a lifetime, not a label.

The first version tracked only the label, and a label can mean a different
client than it did when the lock was taken. Four ways that came apart, each with
a test here that fails without the repair:

* **Two callers, two claims.** The pending entry was cleared only after the
  acquire returned, so simultaneous callers each started one. A POSIX lock
  belongs to an open file description rather than to a process, so the second
  acquire fails against the FIRST - the client failing against its own lease.
* **A late admission spoke for a client that was gone.** Its completion popped
  the label, which by then could belong to a replacement (erasing that
  replacement's pending entry) or to nothing (publishing a lock for a removed
  account).
* **A cancelled caller orphaned the acquire.** `asyncio.to_thread` cannot be
  cancelled: the thread finished, and the lock it produced was referenced by
  nothing - neither held nor released.
* **The lease was released before the socket was.** Exactly the window a second
  process needs to claim a session this one is still connected to.

The lock is the real `SessionLock` against a directory each test owns. Where
timing has to be controlled, the ACQUIRE is replaced by one that blocks on an
event - never the lock itself, because the lock is what is under test.
"""

import asyncio

import pytest

from telegram_mcp import admission as mod
from telegram_mcp.singleton import SessionLock


class _Client:
    def __init__(self, identity):
        self.identity = identity
        self.disconnects = 0

    async def disconnect(self):
        self.disconnects += 1


@pytest.fixture(autouse=True)
def _clean(monkeypatch, tmp_path):
    """Real locks, in a directory this test owns; state cleared in place."""
    mod.session_locks.clear()
    mod._leases.clear()
    mod._awaiting_admission.clear()
    mod._admitting.clear()
    mod._releasing.clear()
    monkeypatch.setattr(mod, "session_identity", lambda client: client.identity)
    real = SessionLock
    monkeypatch.setattr(
        mod, "SessionLock", lambda identity: real(identity, lock_dir=tmp_path / "locks")
    )
    yield
    mod.release_all()


def _acquires_count(monkeypatch):
    """Count acquires without changing what a lock does."""
    seen = []
    real = mod.SessionLock

    def _counting(identity):
        lock = real(identity)
        acquire = lock.acquire

        def _wrapped(**kwargs):
            seen.append(identity)
            return acquire(**kwargs)

        lock.acquire = _wrapped
        return lock

    monkeypatch.setattr(mod, "SessionLock", _counting)
    return seen


# --- how many claims -----------------------------------------------------------


@pytest.mark.asyncio
async def test_concurrent_first_calls_take_exactly_one_lease(monkeypatch):
    """The defect, against a REAL lock: the second acquire is refused by the
    first, so the client failed against its own lease."""
    acquires = _acquires_count(monkeypatch)
    client = _Client("one-session")
    mod.mark_awaiting_admission({"work": client})

    await asyncio.gather(*(mod.admit_if_pending(client) for _ in range(5)))

    assert acquires == ["one-session"], f"{len(acquires)} acquires for one session"
    assert "work" in mod.session_locks


@pytest.mark.asyncio
async def test_a_second_process_is_still_refused(tmp_path):
    """The lock has to keep doing its job: one session, one holder."""
    client = _Client("contended")
    mod.mark_awaiting_admission({"work": client})
    await mod.admit_if_pending(client)

    other_process = mod.SessionLock("contended")

    with pytest.raises(Exception):
        await asyncio.to_thread(other_process.acquire, grace_seconds=0.05)


@pytest.mark.asyncio
async def test_admitting_twice_in_sequence_does_not_take_a_second_lease(monkeypatch):
    acquires = _acquires_count(monkeypatch)
    client = _Client("one-session")
    mod.mark_awaiting_admission({"work": client})

    await mod.admit_if_pending(client)
    await mod.admit_if_pending(client)

    assert len(acquires) == 1


# --- who the lease is for ------------------------------------------------------


class _Blocking:
    """An acquire that finishes when the test says so."""

    def __init__(self):
        self.started = asyncio.Event()
        self.release_it = asyncio.Event()
        self.acquired = []
        self.released = []

    def install(self, monkeypatch, loop):
        test = self

        class _Lock:
            def __init__(self, identity):
                self.identity = identity

            # `**_` for the same reason the claim_session doubles carry it: a
            # double that pins the real signature breaks on its next optional
            # keyword.
            def acquire(self, grace_seconds=None, **_):
                loop.call_soon_threadsafe(test.started.set)
                asyncio.run_coroutine_threadsafe(test.release_it.wait(), loop).result(5)
                test.acquired.append(self.identity)

            def release(self):
                test.released.append(self.identity)

        monkeypatch.setattr(mod, "SessionLock", _Lock)


@pytest.mark.asyncio
async def test_a_replacement_during_acquisition_is_not_erased(monkeypatch):
    """The late admission popped the label - and the label by then belonged to
    the REPLACEMENT, whose pending entry went with it. The replacement was then
    published, serving, and holding no lease at all."""
    blocking = _Blocking()
    blocking.install(monkeypatch, asyncio.get_running_loop())
    old = _Client("session-A")
    mod.mark_awaiting_admission({"work": old})

    waiting = asyncio.ensure_future(mod.admit_if_pending(old))
    await asyncio.wait_for(blocking.started.wait(), timeout=1)

    replacement = _Client("session-B")
    mod._awaiting_admission["work"] = replacement
    blocking.release_it.set()
    with pytest.raises(mod.AdmissionSuperseded):
        await waiting

    assert mod._awaiting_admission.get("work") is replacement, "the replacement was erased"
    assert "work" not in mod.session_locks, "the old client's lock was published anyway"
    assert blocking.released == ["session-A"], "the superseded lock was not released"


@pytest.mark.asyncio
async def test_a_removal_during_acquisition_publishes_nothing(monkeypatch):
    blocking = _Blocking()
    blocking.install(monkeypatch, asyncio.get_running_loop())
    client = _Client("session-A")
    mod.mark_awaiting_admission({"work": client})

    waiting = asyncio.ensure_future(mod.admit_if_pending(client))
    await asyncio.wait_for(blocking.started.wait(), timeout=1)

    mod.forget("work")
    blocking.release_it.set()
    with pytest.raises(mod.AdmissionSuperseded):
        await waiting

    assert mod.session_locks == {}, "a lock was published for a removed account"
    assert blocking.released == ["session-A"]


@pytest.mark.asyncio
async def test_a_cancelled_caller_does_not_orphan_the_acquire(monkeypatch):
    """`to_thread` cannot be cancelled. The thread finishes either way, so the
    lock it produces must still have an owner that publishes or releases it."""
    blocking = _Blocking()
    blocking.install(monkeypatch, asyncio.get_running_loop())
    client = _Client("session-A")
    mod.mark_awaiting_admission({"work": client})

    caller = asyncio.ensure_future(mod.admit_if_pending(client))
    await asyncio.wait_for(blocking.started.wait(), timeout=1)
    caller.cancel()
    with pytest.raises(asyncio.CancelledError):
        await caller

    blocking.release_it.set()
    for _ in range(200):
        if blocking.acquired or blocking.released:
            break
        await asyncio.sleep(0.01)

    assert blocking.acquired == ["session-A"], "the acquire never completed"
    assert "work" in mod.session_locks, "the lock it produced was dropped on the floor"


# --- when the lease goes -------------------------------------------------------


@pytest.mark.asyncio
async def test_a_lease_is_held_until_the_socket_is_down():
    """Releasing while the old socket is still closing is the window another
    process needs to claim a session this one is still connected to."""
    client = _Client("session-A")
    mod.mark_awaiting_admission({"work": client})
    await mod.admit_if_pending(client)
    lock = mod.session_locks["work"]
    released = []
    lock.release = lambda: released.append(True)

    closing = asyncio.get_running_loop().create_future()
    mod.forget("work", closing=closing)
    await asyncio.sleep(0)

    assert released == [], "the lease went before the socket did"

    closing.set_result(None)
    await mod.drain_releases(timeout=2)

    assert released == [True]


@pytest.mark.asyncio
async def test_a_disconnect_that_never_finishes_keeps_the_lease(monkeypatch):
    """INVERTED 2026-09-18. This used to assert that the lease was released once
    the wait timed out, on the reasoning that holding it forever locks the
    operator out of their own account.

    That trade is the wrong way round. An unfinished disconnect means the socket
    MAY STILL BE OPEN, and handing the session to whoever asks next is a session
    connected twice - which Telegram answers by invalidating the auth key for
    both ends. Losing the account is not recoverable; a session this process will
    not reuse until it restarts is. So the lock is kept and the label is recorded,
    which is also what lets shutdown say what it could not account for.
    """
    monkeypatch.setattr(mod, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.05)
    client = _Client("session-A")
    mod.mark_awaiting_admission({"work": client})
    await mod.admit_if_pending(client)
    released = []
    mod.session_locks["work"].release = lambda: released.append(True)

    mod.forget("work", closing=asyncio.get_running_loop().create_future())
    await mod.drain_releases(timeout=2)

    assert released == [], "the session was given away over a socket that never closed"
    assert "work" in mod.unreleased_leases


def test_a_retirement_with_nothing_left_to_wait_for_releases_at_once(monkeypatch, tmp_path):
    lock = mod.SessionLock("session-A")
    lock.acquire(grace_seconds=0.01)
    released = []
    lock.release = lambda: released.append(True)
    mod._publish("work", _Client("session-A"), lock, "session-A")

    mod.forget("work", closing=None)

    assert released == [True]


@pytest.mark.asyncio
async def test_the_same_session_moving_label_keeps_its_lease(monkeypatch):
    """A label rename never touched the session, so releasing and re-taking the
    lock would open a gap for no reason at all."""
    acquires = _acquires_count(monkeypatch)
    client = _Client("session-A")
    mod.mark_awaiting_admission({"work": client})
    await mod.admit_if_pending(client)
    held = mod.session_locks["work"]

    assert mod.transfer_lease("work", "office", client) is True

    assert mod.session_locks["office"] is held
    assert "work" not in mod.session_locks
    assert len(acquires) == 1, "the lease was re-taken for a rename"


@pytest.mark.asyncio
async def test_a_transfer_refuses_a_client_that_does_not_own_the_lease():
    client = _Client("session-A")
    mod.mark_awaiting_admission({"work": client})
    await mod.admit_if_pending(client)

    assert mod.transfer_lease("work", "office", _Client("session-B")) is False
    assert "work" in mod.session_locks


@pytest.mark.asyncio
async def test_shutdown_releases_every_lease():
    for label, identity in (("a", "s-a"), ("b", "s-b")):
        client = _Client(identity)
        mod.mark_awaiting_admission({label: client})
        await mod.admit_if_pending(client)
    released = []
    for lock in mod.session_locks.values():
        lock.release = lambda: released.append(True)

    mod.release_all()

    assert len(released) == 2
    assert mod.session_locks == {} and mod._leases == {}


# --- what the reload path does with all of it ----------------------------------


@pytest.mark.asyncio
async def test_a_hot_added_account_is_admitted_without_any_tool_call(monkeypatch):
    """A server that only waits for incoming events never makes an API call, so
    an account added while it ran sat published and unadmitted indefinitely."""
    client = _Client("fresh-session")

    mod.begin_admission({"fresh": client})
    for _ in range(200):
        if "fresh" in mod.session_locks:
            break
        await asyncio.sleep(0.01)

    assert "fresh" in mod.session_locks, "nothing started the admission"


def test_begin_admission_without_a_loop_defers_rather_than_failing():
    client = _Client("fresh-session")

    mod.begin_admission({"fresh": client})

    assert mod._awaiting_admission.get("fresh") is client
    assert mod.session_locks == {}


# --- the reload and routing paths themselves -----------------------------------


def test_a_failed_discovery_disposes_what_it_had_already_built(monkeypatch):
    """A failure part-way - an unusable label further down the list, or a pool
    with no free slot - left every client built before it holding its session
    file open with nothing referencing it, once per reload."""
    from telegram_mcp import connection as conn

    built = []

    class _Built:
        def __init__(self, label):
            self.label = label
            self.disconnected = False

        def disconnect(self):
            self.disconnected = True
            return None

    def _make(session, label):
        if label == "boom":
            raise ValueError("this one cannot be built")
        client = _Built(label)
        built.append(client)
        return client

    env = {
        "TELEGRAM_SESSION_STRING_AAA": "s1",
        "TELEGRAM_SESSION_STRING_BOOM": "s2",
    }
    # A pool entry is not a real StringSession here and does not need to be:
    # what is under test is what happens to clients already constructed.
    monkeypatch.setattr(conn, "StringSession", lambda value: value)
    monkeypatch.setattr(conn, "_build_client", _make)

    with pytest.raises(ValueError):
        conn._discover_accounts(env)

    assert built, "the test never built anything, so it proves nothing"
    assert all(client.disconnected for client in built), "an abandoned client was left open"


def test_a_reused_client_is_not_disposed_by_a_failure(monkeypatch):
    """Rollback covers what THIS call constructed. A client the caller owns and
    is keeping must survive the failure untouched."""
    from telegram_mcp import connection as conn

    class _Kept:
        def __init__(self):
            self.disconnected = False

        def disconnect(self):
            self.disconnected = True
            return None

    kept = _Kept()
    env = {"TELEGRAM_SESSION_STRING_KEEP": "s1", "TELEGRAM_SESSION_STRING_BOOM": "s2"}
    monkeypatch.setattr(conn, "StringSession", lambda value: value)
    monkeypatch.setattr(
        conn, "_build_client", lambda session, label: (_ for _ in ()).throw(ValueError("no"))
    )

    with pytest.raises(ValueError):
        conn._discover_accounts(env, reuse={"keep": kept})

    assert not kept.disconnected, "rollback closed a client it did not build"


@pytest.mark.asyncio
async def test_with_account_refreshes_before_it_decides_single_or_multi(monkeypatch):
    """`is_multi_mode()` reads the registry, and the registry only moves when
    something refreshes it - so a second account added while the server ran was
    invisible here, and a write with no `account` was refused for being
    single-mode until some unrelated call happened to refresh first."""
    from telegram_mcp import connection as conn

    registry = {"one": _Client("s1")}
    monkeypatch.setattr(conn, "clients", registry)

    def _refresh():
        registry["two"] = _Client("s2")
        return ["two"]

    monkeypatch.setattr(conn, "refresh_accounts", _refresh)

    @conn.with_account(readonly=False)
    async def _write(account: str = None):
        return "served"

    answer = await _write()

    assert "served" != answer, "a multi-account write with no account must be refused"
    assert "account" in str(answer).lower()


@pytest.mark.asyncio
async def test_a_single_account_still_serves_without_an_account_argument(monkeypatch):
    from telegram_mcp import connection as conn

    monkeypatch.setattr(conn, "clients", {"only": _Client("s1")})
    monkeypatch.setattr(conn, "refresh_accounts", lambda: [])

    @conn.with_account(readonly=False)
    async def _write(account: str = None):
        return "served"

    assert await _write() == "served"
