"""A TDLib database belongs to an account, and is never deleted to tidy up.

Two defects with one shape: the database outlives `.env`, and nothing tied the
two together.

* A label reused for a different person kept the PREVIOUS user's TDLib login.
  `_attempt_login` saw `authorizationStateReady`, asked nothing further, and
  every secret-chat call made through that client ran as them.
* The recovery for a dead authorisation was
  `shutil.rmtree(..., ignore_errors=True)`: no backup, no ownership check, no
  confirmation the client had closed, and a locked directory reported as a
  success. A real temporary fixture lost its bytes to it during the audit.

Nothing here starts a native TDLib client. `getMe` and `get_me()` are the two
answers being compared, so the tests supply those two answers.
"""

import asyncio
import json
import sys

import pytest

from telegram_mcp import tdlib, tdlib_identity as identity


@pytest.fixture(autouse=True)
def _state(monkeypatch, tmp_path):
    """Every database under a directory this test owns."""
    # ONE seam: `tdlib_identity.database_dir_for` calls through to `tdlib` at call
    # time, so redirecting the owner is enough - and is the only redirection that
    # actually protects the owner's real state directory.
    monkeypatch.setattr(tdlib, "database_dir_for", lambda label: tmp_path / "tdlib" / label)
    return tmp_path


class _TDLib:
    """A TDLib client that answers `getMe` and nothing else."""

    def __init__(self, user_id):
        self.user_id = user_id
        self.closed = False

    async def request(self, obj, timeout=30.0):
        assert obj["@type"] == "getMe"
        return {"@type": "user", "id": self.user_id}

    async def close(self):
        self.closed = True


class _Telethon:
    def __init__(self, user_id):
        self.user_id = user_id

    async def get_me(self):
        return type("Me", (), {"id": self.user_id})()


def _database(tmp_path, label, contents=b"the secret chat keys"):
    directory = identity.database_dir_for(label)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "td.binlog").write_bytes(contents)
    return directory


# --- whose database is this ---------------------------------------------------


@pytest.mark.asyncio
async def test_matching_identities_pass_and_are_recorded(_state):
    _database(_state, "work")

    found = await identity.verify_owner("work", _TDLib(777), _Telethon(777))

    assert found == 777
    assert identity.read_identity("work") == 777


@pytest.mark.asyncio
async def test_a_reused_label_is_refused_before_anything_is_done(_state):
    """The defect. The label now names user 222; the database is still signed in
    as 111, and every call through it would run as 111."""
    directory = _database(_state, "work")

    with pytest.raises(identity.IdentityMismatch) as raised:
        await identity.verify_owner("work", _TDLib(111), _Telethon(222))

    said = str(raised.value)
    assert "111" in said and "222" in said
    assert directory.exists(), "a mismatch deleted the evidence instead of reporting it"


@pytest.mark.asyncio
async def test_a_mismatch_names_both_accounts_and_the_directory(_state):
    _database(_state, "work")

    with pytest.raises(identity.IdentityMismatch) as raised:
        await identity.verify_owner("work", _TDLib(111), _Telethon(222))

    assert str(identity.database_dir_for("work")) in str(raised.value)


@pytest.mark.asyncio
async def test_an_existing_database_gets_its_binding_written_on_first_use(_state):
    """The migration, and the whole of it: no guessing from a filename, no
    deletion for lacking a file that did not exist when the database was made."""
    _database(_state, "legacy")
    assert identity.read_identity("legacy") is None

    await identity.verify_owner("legacy", _TDLib(500), _Telethon(500))

    assert identity.read_identity("legacy") == 500


@pytest.mark.asyncio
async def test_the_same_user_with_a_new_session_still_passes(_state):
    """A re-login is a new session for the SAME person, which is supported."""
    _database(_state, "work")
    await identity.verify_owner("work", _TDLib(900), _Telethon(900))

    assert await identity.verify_owner("work", _TDLib(900), _Telethon(900)) == 900


@pytest.mark.asyncio
async def test_with_no_session_to_compare_the_recorded_binding_is_used(_state):
    """The cached-client path: `secret_client` has a database and no Telethon
    session, so the recorded id is what it checks against."""
    _database(_state, "work")
    identity.record_identity("work", 111)

    with pytest.raises(identity.IdentityMismatch, match="recorded earlier"):
        await identity.verify_owner("work", _TDLib(222), None)


@pytest.mark.asyncio
async def test_with_neither_a_session_nor_a_binding_nothing_is_accused(_state):
    _database(_state, "legacy")

    assert await identity.verify_owner("legacy", _TDLib(42), None) == 42


def test_an_unreadable_binding_is_not_evidence_of_anything(_state):
    directory = _database(_state, "work")
    (directory / "owner.json").write_text("not json", encoding="utf-8")

    assert identity.read_identity("work") is None


def test_a_binding_is_written_atomically_and_leaves_no_temporary(_state):
    _database(_state, "work")

    identity.record_identity("work", 4242)

    directory = identity.database_dir_for("work")
    assert json.loads((directory / "owner.json").read_text(encoding="utf-8"))["user_id"] == 4242
    assert not list(directory.glob("*.tmp")), "a partial write was left behind"


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX file mode")
def test_a_binding_is_owner_only(_state):
    _database(_state, "work")
    identity.record_identity("work", 1)

    assert (identity.identity_path("work").stat().st_mode & 0o077) == 0


# --- what happens to a dead database ------------------------------------------


def test_a_dead_database_is_moved_aside_not_deleted(_state):
    """The defect: `rmtree(..., ignore_errors=True)`. The bytes may hold
    secret-chat keys that cannot be re-derived, and the diagnosis that led here
    is a guess about Telegram's answer, not a fact about the file."""
    directory = _database(_state, "work", b"keys worth keeping")

    kept = identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED", closed=True)

    assert not directory.exists(), "the active name was not freed for a fresh database"
    assert (kept / "td.binlog").read_bytes() == b"keys worth keeping"


def test_a_locked_database_is_reported_not_silently_left(_state, monkeypatch):
    """`ignore_errors=True` reported success having deleted nothing, and the
    caller then started a fresh login on top of the same stale database."""
    _database(_state, "work")

    def _refuse(src, dst):
        raise OSError("the directory is open in another process")

    monkeypatch.setattr(identity.os, "replace", _refuse)
    monkeypatch.setattr(identity, "_RELEASE_DEADLINE_SECONDS", 0.01)
    monkeypatch.setattr(identity, "_RELEASE_POLL_SECONDS", 0.001)

    with pytest.raises(identity.QuarantineFailed, match="still holds the database open"):
        identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED", closed=True)

    assert identity.database_dir_for("work").exists(), "it was destroyed after all"


def test_quarantining_nothing_is_an_error(_state):
    with pytest.raises(identity.QuarantineFailed, match="no TDLib database"):
        identity.quarantine_database("absent", why="AUTH_KEY_UNREGISTERED", closed=True)


def test_two_quarantines_in_the_same_second_both_survive(_state):
    _database(_state, "work", b"first")
    first = identity.quarantine_database("work", why="one", closed=True)
    _database(_state, "work", b"second")

    second = identity.quarantine_database("work", why="two", closed=True)

    assert first != second
    assert (first / "td.binlog").read_bytes() == b"first"
    assert (second / "td.binlog").read_bytes() == b"second"


# --- the login path -----------------------------------------------------------


def _login_with(monkeypatch, attempts):
    """Drive `complete_login` with a scripted sequence of attempt outcomes."""
    calls = []

    async def _attempt(label, telethon_client, password, ask_password):
        calls.append(label)
        outcome = attempts[len(calls) - 1]
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    monkeypatch.setattr(tdlib, "_attempt_login", _attempt)
    return calls


def test_a_dead_authorisation_quarantines_once_and_retries(_state, monkeypatch):
    _database(_state, "work")
    calls = _login_with(
        monkeypatch,
        [tdlib.TDLibError(401, "AUTH_KEY_UNREGISTERED"), "authorizationStateReady"],
    )

    state = asyncio.run(tdlib.complete_login("work", _Telethon(1)))

    assert state == "authorizationStateReady"
    assert len(calls) == 2
    quarantined = list((_state / "tdlib").glob("work.quarantined-*"))
    assert len(quarantined) == 1, "the database was not kept"


def test_a_dead_authorisation_that_survives_a_fresh_database_stops(_state, monkeypatch):
    """No endless recreation of authorisations: each attempt spends a real login,
    and a second quarantine would only learn the same thing again."""
    _database(_state, "work")
    _login_with(
        monkeypatch,
        [
            tdlib.TDLibError(401, "AUTH_KEY_UNREGISTERED"),
            tdlib.TDLibError(401, "SESSION_REVOKED"),
            "authorizationStateReady",
        ],
    )

    with pytest.raises(RuntimeError) as raised:
        asyncio.run(tdlib.complete_login("work", _Telethon(1)))

    said = str(raised.value)
    assert "still refuses" in said
    assert "quarantined-" in said, "the operator was not told where their database is"
    assert len(list((_state / "tdlib").glob("work.quarantined-*"))) == 1


def test_an_unrelated_failure_resets_nothing(_state, monkeypatch):
    """A 401 that is not one of the three dead-authorisation names - a login in
    progress, for instance - must not cost anyone their database."""
    directory = _database(_state, "work")
    _login_with(monkeypatch, [tdlib.TDLibError(420, "FLOOD_WAIT_30")])

    with pytest.raises(tdlib.TDLibError, match="FLOOD_WAIT"):
        asyncio.run(tdlib.complete_login("work", _Telethon(1)))

    assert (directory / "td.binlog").exists()
    assert not list((_state / "tdlib").glob("work.quarantined-*"))


def test_a_mismatch_is_never_recovered_from_automatically(_state, monkeypatch):
    """Both accounts are real; only the operator knows which the label meant."""
    directory = _database(_state, "work")
    _login_with(monkeypatch, [identity.IdentityMismatch("111 is not 222")])

    with pytest.raises(identity.IdentityMismatch):
        asyncio.run(tdlib.complete_login("work", _Telethon(222)))

    assert (directory / "td.binlog").exists()
    assert not list((_state / "tdlib").glob("work.quarantined-*")), "it quarantined a live account"


class _StartsReady:
    """A TDLib client that comes up already signed in, as `user_id`."""

    def __init__(self, user_id, account="acc"):
        self.user_id = user_id
        self.closed = 0
        self._client_id = 1
        self.authorization_state = "authorizationStateReady"

    async def start(self):
        return "authorizationStateReady"

    async def request(self, obj, timeout=30.0):
        assert obj["@type"] == "getMe"
        return {"@type": "user", "id": self.user_id}

    async def close(self):
        self.closed += 1
        self._client_id = None


def test_the_login_path_asks_whose_database_it_is(_state, monkeypatch):
    """The wiring, not the module. `_attempt_login` returned a ready client
    without asking, so a reused label handed back the previous owner's login."""
    _database(_state, "work")
    previous = _StartsReady(111)
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: previous)

    with pytest.raises(identity.IdentityMismatch):
        asyncio.run(tdlib._attempt_login("work", _Telethon(222), None, None))

    assert previous.closed == 1, "the wrong account's client was left open"


def test_the_login_path_records_the_binding_it_proved(_state, monkeypatch):
    _database(_state, "work")
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: _StartsReady(555))

    state = asyncio.run(tdlib._attempt_login("work", _Telethon(555), None, None))

    assert state == "authorizationStateReady"
    assert identity.read_identity("work") == 555


def test_the_cached_client_path_asks_too(_state, monkeypatch):
    """`secret_client` never goes through the login tool, so a label reused for
    another account reaches a signed-in client by this route alone."""
    import asyncio as _asyncio

    from telegram_mcp import tdlib_registry as reg

    from telegram_mcp import connection as conn

    _database(_state, "work")
    identity.record_identity("work", 111)
    monkeypatch.setattr(reg, "_by_account", {})
    monkeypatch.setattr(reg, "_verified_against", {})
    monkeypatch.setattr(reg, "_by_account_lock", _asyncio.Lock())
    # The label now names user 111 on the Telethon side; the database is 222.
    monkeypatch.setattr(conn, "clients", {"work": _Telethon(111)})
    monkeypatch.setattr(conn, "refresh_accounts", lambda: [])
    wrong = _StartsReady(222)
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: wrong)

    with pytest.raises(identity.IdentityMismatch):
        asyncio.run(reg.secret_client("work"))

    assert wrong.closed == 1, "the wrong account's client was cached or left open"
    assert "work" not in reg._by_account


def test_the_real_state_directory_is_never_written_to_by_accident(tmp_path, monkeypatch):
    """This one is not hypothetical.

    `tdlib_identity` used to `from telegram_mcp.tdlib import database_dir_for`,
    which binds a SECOND name. A test that redirected `tdlib.database_dir_for`
    therefore left this module resolving the real path, and a fixture's
    `user_id: 7` was recorded against two live TDLib databases under
    `~/.local/state/telegram-mcp/tdlib` - locking one of them out with an
    IdentityMismatch until the file was removed by hand.

    Two guards, because either alone was enough to prevent it: the seam is the
    owning module's, and a binding is never written where there is no database.
    """
    monkeypatch.setattr(tdlib, "database_dir_for", lambda label: tmp_path / "redirected" / label)

    assert identity.database_dir_for("work") == tmp_path / "redirected" / "work"

    identity.record_identity("work", 7)

    assert not (
        tmp_path / "redirected"
    ).exists(), "recording a binding created a database directory that did not exist"
    assert identity.read_identity("work") is None


def test_a_rename_waits_for_the_previous_holder_to_let_go(_state, monkeypatch):
    """TDLib answers `close` and THEN finishes its checkpoint on its own thread.

    Measured against a live 40 MB database: `close_all()` reported clean, and the
    quarantine rename that followed succeeded on one run and returned WinError 5
    on the next - telling the operator to close a server that had already closed.
    So the rename waits for the handle to go, bounded, and the thing it waits on
    is the rename itself succeeding rather than a guessed interval.
    """
    _database(_state, "work")
    attempts = []
    real = identity.os.replace

    def _busy_at_first(src, dst):
        attempts.append(1)
        if len(attempts) < 3:
            raise PermissionError(13, "Access is denied")
        return real(src, dst)

    monkeypatch.setattr(identity.os, "replace", _busy_at_first)
    monkeypatch.setattr(identity, "_RELEASE_POLL_SECONDS", 0.001)

    kept = identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED", closed=True)

    assert len(attempts) == 3, "it gave up on the first refusal"
    assert (kept / "td.binlog").exists()


def test_a_holder_that_never_lets_go_is_still_reported(_state, monkeypatch):
    """Bounded, so a directory genuinely held open fails rather than hanging."""
    _database(_state, "work")

    def _always_busy(src, dst):
        raise PermissionError(13, "Access is denied")

    monkeypatch.setattr(identity.os, "replace", _always_busy)
    monkeypatch.setattr(identity, "_RELEASE_DEADLINE_SECONDS", 0.01)
    monkeypatch.setattr(identity, "_RELEASE_POLL_SECONDS", 0.001)

    with pytest.raises(identity.QuarantineFailed, match="still holds the database open"):
        identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED", closed=True)

    assert identity.database_dir_for("work").exists(), "it was destroyed after all"
