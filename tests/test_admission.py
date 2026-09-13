"""One admission path, whether an account was present at boot or added later.

Startup rejected two labels sharing a session and took a per-session file lock
before connecting. A hot reload did neither, so the same server enforced two
different rules depending on when an account appeared - and the side door led
exactly where the front door was guarded: one auth key used from two places,
which Telegram answers by invalidating it for both.

Nothing here opens a socket; the lock is the real `SessionLock` against a
temporary directory.
"""

import time

import pytest

from telegram_mcp import admission as mod
from telegram_mcp import connection as conn
from telegram_mcp.settings import StartupMessage


class _Client:
    def __init__(self, identity):
        self.identity = identity
        self.disconnected = False

    async def disconnect(self):
        self.disconnected = True


@pytest.fixture(autouse=True)
def _clean(monkeypatch, tmp_path):
    # Cleared in place, never rebound: the runner holds a second name for this
    # very dict, and rebinding would quietly hand the two halves of the server
    # separate registries - the defect this module exists to close.
    mod.session_locks.clear()
    mod._awaiting_admission.clear()
    monkeypatch.setattr(mod, "session_identity", lambda client: client.identity)
    # The real SessionLock, against a directory this test owns: the lock is the
    # behaviour under test, so a fake one would prove nothing.
    real = mod.SessionLock
    monkeypatch.setattr(
        mod, "SessionLock", lambda identity: real(identity, lock_dir=tmp_path / "locks")
    )
    yield
    mod.release_all()


def test_two_labels_sharing_one_session_are_refused():
    """One session is one auth key; connecting it twice makes Telegram
    invalidate it for both labels."""
    with pytest.raises(StartupMessage) as raised:
        mod.reject_duplicate_sessions({"work": _Client("same"), "home": _Client("same")})

    said = str(raised.value)
    assert "work" in said and "home" in said


def test_distinct_sessions_pass():
    mod.reject_duplicate_sessions({"work": _Client("a"), "home": _Client("b")})


def test_an_identity_that_cannot_be_read_is_not_called_a_duplicate(monkeypatch):
    """Absence of evidence is not a duplicate; the connect path reports whatever
    is actually wrong with such a session."""

    def _unreadable(client):
        raise ValueError("not a session this can identify")

    monkeypatch.setattr(mod, "session_identity", _unreadable)

    mod.reject_duplicate_sessions({"one": _Client("x"), "two": _Client("x")})


def test_a_reload_runs_the_same_duplicate_check(monkeypatch, tmp_path):
    """The side door. `refresh_accounts` could publish the configuration startup
    refuses, and the first use burned the key for both labels."""
    path = tmp_path / ".env"
    path.write_text("TELEGRAM_SESSION_STRING_ONE=s\n", encoding="utf-8")
    from telegram_mcp import account_config as cfg

    from telegram_mcp import account_snapshot as snapshot

    monkeypatch.setattr(cfg, "_env_file", lambda: str(path))
    monkeypatch.setattr(conn, "_env_file", lambda: str(path))
    monkeypatch.setattr(snapshot, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.setattr(conn, "read_snapshot", lambda p=None: snapshot.read_snapshot(str(path)))
    monkeypatch.setattr(conn, "clients", {"one": _Client("kept")})
    monkeypatch.setattr(conn, "_env_stamp", ())
    monkeypatch.setattr(conn, "_env_digests", {"TELEGRAM_SESSION_STRING_ONE": "old"})
    monkeypatch.setattr(
        conn,
        "_discover_accounts",
        lambda env=None, reuse=None: {"one": _Client("same"), "two": _Client("same")},
    )

    changed = conn.refresh_accounts()

    assert changed == [], "the duplicate configuration was published"
    assert set(conn.clients) == {"one"}, "and it replaced the working client"


@pytest.mark.asyncio
async def test_a_hot_added_client_takes_its_session_lock_on_first_use():
    """Admission needs an event loop and `refresh_accounts` is synchronous, so
    the work is deferred to the first async touch - never skipped."""
    client = _Client("hot-added-identity")
    mod.mark_awaiting_admission({"fresh": client})
    assert mod.session_locks == {}

    await mod.admit_if_pending(client)

    assert "fresh" in mod.session_locks, "the hot-added client never took a lock"
    assert mod._awaiting_admission == {}


@pytest.mark.asyncio
async def test_admission_is_idempotent():
    client = _Client("identity")
    mod.mark_awaiting_admission({"fresh": client})

    await mod.admit_if_pending(client)
    held = mod.session_locks["fresh"]
    await mod.admit_if_pending(client)

    assert mod.session_locks["fresh"] is held, "it queued behind its own lock"


@pytest.mark.asyncio
async def test_a_client_nobody_is_waiting_on_is_left_alone():
    await mod.admit_if_pending(_Client("unknown"))

    assert mod.session_locks == {}


@pytest.mark.asyncio
async def test_a_failed_admission_stays_pending_and_raises(monkeypatch):
    """Until it succeeds, nothing has been served from an unlocked session."""
    client = _Client("identity")
    mod.mark_awaiting_admission({"fresh": client})

    class _Refusing:
        def __init__(self, identity):
            pass

        def acquire(self, grace_seconds=None):
            raise OSError("the lock file could not be opened")

    monkeypatch.setattr(mod, "SessionLock", _Refusing)

    with pytest.raises(OSError):
        await mod.admit_if_pending(client)

    assert "fresh" in mod._awaiting_admission, "the failure was forgotten"


def test_retiring_a_label_releases_what_it_held():
    released = []

    class _Lock:
        def release(self):
            released.append(True)

    lock = _Lock()
    client = _Client("x")
    mod._publish("gone", client, lock, "x")
    mod._awaiting_admission["gone"] = client

    mod.forget("gone")

    assert released == [True]
    assert "gone" not in mod.session_locks
    assert "gone" not in mod._awaiting_admission


def test_the_runner_and_the_reload_share_one_lock_registry():
    """Two registries would mean a label moving between them holds two locks or
    none - which is the same defect wearing a different hat."""
    from telegram_mcp import runner

    assert runner._session_locks is mod.session_locks
    assert runner._reject_duplicate_sessions is mod.reject_duplicate_sessions


def _env_pointing_at(monkeypatch, tmp_path, text):
    """A `.env` the reload reads, with the module-level caches reset around it."""
    path = tmp_path / ".env"
    path.write_text(text, encoding="utf-8")
    from telegram_mcp import account_config as cfg

    from telegram_mcp import account_snapshot as snapshot

    monkeypatch.setattr(cfg, "_env_file", lambda: str(path))
    monkeypatch.setattr(conn, "_env_file", lambda: str(path))
    monkeypatch.setattr(snapshot, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.setattr(conn, "read_snapshot", lambda p=None: snapshot.read_snapshot(str(path)))
    monkeypatch.setattr(conn, "_env_stamp", ())
    return path


def test_a_reload_queues_its_new_client_for_admission(monkeypatch, tmp_path):
    """The wiring, not the module: `refresh_accounts` is synchronous and cannot
    take a lock, so what it must do is leave the work where the first async use
    finds it. Skipping that is how a reload connected an unlocked session."""
    _env_pointing_at(monkeypatch, tmp_path, "TELEGRAM_SESSION_STRING_TWO=s\n")
    fresh = _Client("brand-new")
    monkeypatch.setattr(conn, "clients", {})
    monkeypatch.setattr(conn, "_env_digests", {})
    monkeypatch.setattr(conn, "_discover_accounts", lambda env=None, reuse=None: {"two": fresh})

    conn.refresh_accounts()

    assert mod._awaiting_admission.get("two") is fresh, "the new client was published unadmitted"


@pytest.mark.asyncio
async def test_the_connection_path_admits_before_it_serves(monkeypatch):
    """`ensure_connected` is the first thing inside the loop to touch a client,
    so it is the seam that turns a queued admission into a held lock."""
    from telegram_mcp import reconnect

    client = _Client("served-identity")
    client.is_connected = lambda: True
    mod.mark_awaiting_admission({"late": client})
    monkeypatch.setattr(reconnect, "_last_conn_verified", {id(client): time.time()})

    await reconnect.ensure_connected(client)

    assert "late" in mod.session_locks, "a session was served from without its lock"
