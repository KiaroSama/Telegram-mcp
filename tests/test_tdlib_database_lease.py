"""A TDLib database belongs to the client that opened it, not to the label on it.

ADR 0001 one layer down, and the same reasoning applies: the only thing standing
between two processes and one native database was ``closed=True``, a boolean the
caller passed in, backed by ``tdlib._close_confirmed`` - a module-level dict keyed
by LABEL. Four things follow from that, and each has a test here:

1. Nothing refused another PROCESS. A second server, a standalone
   ``secret_chat_login.py`` or a recovery script could open, and then rename
   away, a database this process was still checkpointing.
2. A quarantine is a rename of a database whose secret-chat keys cannot be
   re-derived, and it was authorised by a value any earlier attempt on that
   label had left behind.
3. The rename retries for five seconds with ``time.sleep``, and it ran on the
   event loop - so every other account's shutdown waited behind it.
4. A close that could not be confirmed left nothing owning the database.

**Why a real second process.** A mock can record that a function was called; it
cannot be refused an operating-system lock, which is the entire question here.
The previous mock-lock suite for session leases missed all five of its defects
for exactly this reason - see ``docs/adr/0001`` and ``.ai/BUGS.md``.
"""

import asyncio
import contextlib
import json
import os
import stat
import subprocess
import sys
import textwrap

import pytest

from telegram_mcp import tdlib, tdlib_identity as identity, tdlib_lease as lease

# Tries once and reports. A probe that WAITED would turn a failing assertion
# into a hang.
_PROBE = textwrap.dedent("""
    import sys
    from pathlib import Path
    from telegram_mcp import tdlib_lease

    tdlib_lease.lock_dir = Path(sys.argv[2])
    try:
        tdlib_lease.hold(Path(sys.argv[1]), object())
    except tdlib_lease.DatabaseBusy:
        print("REFUSED")
    else:
        print("TAKEN")
    """)

# Takes the lease and holds it until its stdin closes. The self-destruct is not
# ceremony: nothing this suite starts may outlive it, whatever the parent does.
_HOLDER = textwrap.dedent("""
    import os
    import sys
    import threading
    from pathlib import Path
    from telegram_mcp import tdlib_lease

    threading.Timer(60, os._exit, [3]).start()
    tdlib_lease.lock_dir = Path(sys.argv[2])
    tdlib_lease.hold(Path(sys.argv[1]), object())
    print("HELD", flush=True)
    sys.stdin.readline()
    """)


def second_process_sees(database_dir, lock_dir) -> str:
    """What another process makes of this database: ``TAKEN`` or ``REFUSED``."""
    done = subprocess.run(
        [sys.executable, "-c", _PROBE, str(database_dir), str(lock_dir)],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert done.returncode == 0, f"probe failed: {done.stderr}"
    return done.stdout.strip()


@contextlib.contextmanager
def another_process_holding(database_dir, lock_dir):
    """A real second process owning this database for the body of the block."""
    child = subprocess.Popen(
        [sys.executable, "-c", _HOLDER, str(database_dir), str(lock_dir)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        ready = child.stdout.readline().strip()
        assert ready == "HELD", f"the holder never took the lease ({ready!r})"
        yield child
    finally:
        with contextlib.suppress(OSError, ValueError):
            child.stdin.close()
        try:
            child.wait(timeout=60)
        except subprocess.TimeoutExpired:  # pragma: no cover - the timer above beat us to it
            child.kill()
            child.wait(timeout=60)


class FakeTdjson:
    """Enough of the native module for `start` and `close`, and no binary."""

    def __init__(self):
        self.sent = []

    def td_create_client_id(self):
        return 11

    def td_send(self, client_id, payload):
        self.sent.append(json.loads(payload.decode()))

    def td_receive(self, timeout):  # pragma: no cover - no reader thread is run
        return None

    def td_execute(self, payload):
        return json.dumps({"@type": "ok"}).encode()


@pytest.fixture(autouse=True)
def leases(monkeypatch, tmp_path):
    """An isolated lease table over a real lock directory this test owns."""
    monkeypatch.setattr(lease, "_held", {})
    monkeypatch.setattr(lease, "lock_dir", tmp_path / "locks")
    monkeypatch.setattr(tdlib, "database_dir_for", lambda label: tmp_path / "tdlib" / label)
    yield tmp_path
    # Handles, not just entries: the lock IS the open file, so a test that left
    # one behind would keep a lock the next one has no name for.
    for held in list(lease._held.values()):
        held.handle.close()
    lease._held.clear()


def _database(root, label="work", contents=b"secret chat keys"):
    directory = tdlib.database_dir_for(label)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "td.binlog").write_bytes(contents)
    return directory


@pytest.fixture
def fake_tdjson(monkeypatch):
    module = FakeTdjson()
    monkeypatch.setattr(tdlib, "_tdjson", lambda: module)
    monkeypatch.setattr(tdlib, "_ensure_reader", lambda: None)
    monkeypatch.setattr(tdlib, "_clients", {})
    return module


async def _ready_client(label="work"):
    """A client past `start`, with TDLib's own first update delivered."""
    client = tdlib.TDLibClient(label)
    task = asyncio.ensure_future(client.start())
    await asyncio.sleep(0)
    client._handle_on_loop(
        {
            "@type": "updateAuthorizationState",
            "authorization_state": {"@type": "authorizationStateReady"},
        }
    )
    assert await task == "authorizationStateReady"
    return client


# --- the lease is an operating-system lock, not a Python object ----------------


def test_a_database_nobody_holds_is_free_to_another_process(leases):
    """The control. Without it `REFUSED` might be the only word the probe knows."""
    assert second_process_sees(tdlib.database_dir_for("work"), lease.lock_dir) == "TAKEN"


def test_a_second_process_cannot_take_a_database_this_one_holds(leases):
    database = tdlib.database_dir_for("work")
    lease.hold(database, object())

    assert second_process_sees(database, lease.lock_dir) == "REFUSED"


def test_the_lease_is_the_path_so_two_labels_on_one_database_are_one_lease(leases):
    """The label is not the key. Two names for one directory are one database."""
    database = tdlib.database_dir_for("work")
    lease.hold(database, object())

    with pytest.raises(lease.DatabaseBusy):
        lease.hold(str(database), object())


def test_only_the_owner_can_end_a_lease(leases):
    database = tdlib.database_dir_for("work")
    owner = object()
    lease.hold(database, owner)

    assert lease.release(database, object()) is False
    assert second_process_sees(database, lease.lock_dir) == "REFUSED"
    assert lease.release(database, owner) is True
    assert second_process_sees(database, lease.lock_dir) == "TAKEN"


# --- the quarantine ------------------------------------------------------------


def test_a_quarantine_is_refused_while_another_process_holds_the_database(leases):
    """The defect this closes. `closed=True` is a statement about THIS process;
    the database was renamed out from under a live client in another one."""
    database = _database(leases)

    with another_process_holding(database, lease.lock_dir):
        with pytest.raises(identity.QuarantineFailed, match="another process"):
            identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED", closed=True)

    assert (database / "td.binlog").read_bytes() == b"secret chat keys"


def test_the_owner_of_the_lease_may_quarantine(leases):
    """The guard must not refuse the one caller entitled to do this."""
    database = _database(leases)
    attempt = lease.LoginAttempt("work", database)
    lease.hold(database, attempt)

    kept = identity.quarantine_database("work", why="dead", closed=True, owner=attempt)

    assert not database.exists()
    assert (kept / "td.binlog").read_bytes() == b"secret chat keys"


def test_a_quarantine_leaves_the_database_free_for_the_next_login(leases):
    """The probe it takes to check ownership must be given back."""
    database = _database(leases)

    identity.quarantine_database("work", why="dead", closed=True)

    assert lease.owner_of(database) is None
    assert second_process_sees(database, lease.lock_dir) == "TAKEN"


@pytest.mark.skipif(os.name == "nt", reason="POSIX modes; Windows carries ACLs instead")
def test_a_quarantine_keeps_the_database_as_private_as_it_found_it(leases):
    """A rename preserves the mode; a copy-and-delete would not have.

    What is moved aside still holds secret-chat keys, so a quarantine that
    widened it would publish exactly what the original was protecting.
    """
    database = _database(leases)
    os.chmod(database, 0o700)

    kept = identity.quarantine_database("work", why="dead", closed=True)

    assert stat.S_IMODE(kept.stat().st_mode) == 0o700


@pytest.mark.asyncio
async def test_a_receipt_from_another_attempt_cannot_authorise_a_quarantine(monkeypatch, leases):
    """A close confirmation belongs to the attempt that watched it happen.

    Red before the fix: the confirmation lived in `tdlib._close_confirmed`, keyed
    by LABEL, so a `True` any earlier login for this name left behind was what
    authorised this one's rename - and this attempt's client confirmed nothing.
    """
    _database(leases)
    # Whatever an earlier login for this label left behind, in the form the code
    # of the day keeps it.
    getattr(tdlib, "_close_confirmed", {})["work"] = True

    async def _dead(*_args, **_kwargs):
        raise tdlib.TDLibError(401, "AUTH_KEY_UNREGISTERED")

    monkeypatch.setattr(tdlib, "_attempt_login", _dead)

    with pytest.raises(identity.QuarantineFailed, match="not confirmed it closed"):
        await tdlib.complete_login("work", object())

    assert list((leases / "tdlib").glob("work.quarantined-*")) == []


@pytest.mark.asyncio
async def test_the_rename_retry_does_not_run_on_the_event_loop(monkeypatch, leases):
    """`_rename_when_released` polls with `time.sleep` for up to five seconds.

    On the loop that is five seconds in which no other account disconnects, no
    feed stops and no other database is flushed.
    """
    _database(leases)
    where = []

    def _watching(*_args, **_kwargs):
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            where.append("off the loop")
        else:
            where.append("on the loop")
        return leases / "kept-aside"

    monkeypatch.setattr(identity, "quarantine_database", _watching)

    calls = []

    async def _dead_then_ready(*_args, **_kwargs):
        calls.append(1)
        if len(calls) == 1:
            raise tdlib.TDLibError(401, "AUTH_KEY_UNREGISTERED")
        return "authorizationStateReady"

    monkeypatch.setattr(tdlib, "_attempt_login", _dead_then_ready)

    assert await tdlib.complete_login("work", object()) == "authorizationStateReady"
    assert where == ["off the loop"]


# --- a client holds its database for as long as it has one ---------------------


@pytest.mark.asyncio
async def test_a_started_client_owns_its_database(fake_tdjson, leases):
    client = await _ready_client()

    assert lease.owner_of(client.database_dir) is client
    assert second_process_sees(client.database_dir, lease.lock_dir) == "REFUSED"


@pytest.mark.asyncio
async def test_a_close_that_never_confirms_keeps_the_database(fake_tdjson, leases):
    """The ADR's rule, one layer down: an unconfirmed close keeps the lock. The
    client may still be checkpointing, and a database released here is one
    another process may open mid-write."""
    client = await _ready_client()

    with pytest.raises(TimeoutError):
        await client.close(timeout=0.05)

    assert lease.owner_of(client.database_dir) is client
    assert second_process_sees(client.database_dir, lease.lock_dir) == "REFUSED"


@pytest.mark.asyncio
async def test_a_confirmed_close_hands_the_database_back(fake_tdjson, leases):
    """Held too long costs one account until the next restart; that is still a
    cost, and a client that really closed is holding nothing."""
    client = await _ready_client()
    closing = asyncio.ensure_future(client.close(timeout=5))
    await asyncio.sleep(0)
    # Closed, and no answer to the `close` request itself - which is what TDLib
    # does when the authorisation ends underneath it. The state IS the
    # completion signal; the request's own reply is only an acknowledgement.
    client._handle_on_loop(
        {
            "@type": "updateAuthorizationState",
            "authorization_state": {"@type": "authorizationStateClosed"},
        }
    )
    await closing

    assert lease.owner_of(client.database_dir) is None
    assert second_process_sees(client.database_dir, lease.lock_dir) == "TAKEN"


@pytest.mark.asyncio
async def test_a_login_sequence_keeps_one_lease_across_its_client_and_its_retry(
    fake_tdjson, leases
):
    """The client a login makes must not be refused by its own sequence's lease,
    and the lease must outlive it: the quarantine happens after the close."""
    database = tdlib.database_dir_for("work")
    attempt = lease.LoginAttempt("work", database)
    lease.hold(database, attempt)

    first = tdlib.TDLibClient("work", lease_owner=attempt)
    task = asyncio.ensure_future(first.start())
    await asyncio.sleep(0)
    first._handle_on_loop(
        {
            "@type": "updateAuthorizationState",
            "authorization_state": {"@type": "authorizationStateClosed"},
        }
    )
    await task
    await attempt.close(first)

    assert attempt.closed_confirmed is True
    assert lease.owner_of(database) is attempt, "the close took the sequence's lease with it"
    assert second_process_sees(database, lease.lock_dir) == "REFUSED"
