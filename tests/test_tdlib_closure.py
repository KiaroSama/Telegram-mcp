"""Closed means TDLib said so, and a database is only moved once it is.

TDLib answers the `close` REQUEST with an acknowledgement that the request
arrived, and its own documentation names `authorizationStateClosed` as the
moment the work is finished. Taking the first for the second produced four
separate failures:

* a close that had not happened was reported as one;
* the dispatch registration went while the native client was still
  checkpointing, so the next caller started a second one against that database;
* `close_all` cleared the registry BEFORE awaiting the closes, which is the same
  window with the door held open, and a cancellation during the first close left
  every later client neither closed nor remembered;
* the recovery path then moved the database aside on the strength of a rename
  succeeding - which on POSIX it does immediately, open file descriptors and all.

The fifth is smaller and not about closing at all: `owner.json.tmp` was a
predictable name opened with ordinary semantics, so a symlink planted there in a
writable state directory was followed and its target overwritten.
"""

import asyncio
import json
import os
import sys

import pytest

from telegram_mcp import tdlib, tdlib_identity as identity, tdlib_registry as reg


class _Client:
    """A TDLib client whose close is under the test's control."""

    def __init__(self, account="acc", reach_closed=True, request_fails=False):
        self.account = account
        self.authorization_state = "authorizationStateReady"
        self._client_id = 1
        self._pending = {}
        self._closed = None
        self.reach_closed = reach_closed
        self.request_fails = request_fails
        self.requests = []

    async def request(self, obj, timeout=30.0):
        self.requests.append(obj["@type"])
        if self.request_fails:
            raise tdlib.TDLibError(500, "close refused")
        if obj["@type"] == "close" and self.reach_closed:
            # What TDLib does: answer the request, then report the state.
            self.authorization_state = "authorizationStateClosed"
            if self._closed is not None:
                self._closed.set()
        return {"@type": "ok"}

    def _closed_event(self):
        return tdlib.TDLibClient._closed_event(self)

    def _settle_pending(self, error):
        return tdlib.TDLibClient._settle_pending(self, error)

    async def close(self, timeout=0.2):
        return await tdlib.TDLibClient.close(self, timeout=timeout)


@pytest.fixture(autouse=True)
def _clean(monkeypatch, tmp_path):
    reg._by_account.clear()
    reg._verified_against.clear()
    monkeypatch.setattr(reg, "_by_account_lock", asyncio.Lock())
    monkeypatch.setattr(reg, "_closing", False)
    monkeypatch.setattr(tdlib, "database_dir_for", lambda label: tmp_path / "tdlib" / label)
    yield
    reg._by_account.clear()


# --- what "closed" means -------------------------------------------------------


@pytest.mark.asyncio
async def test_a_close_waits_for_the_documented_completion_signal():
    client = _Client()

    await client.close()

    assert "close" in client.requests
    assert client._client_id is None, "the registration outlived the close"


@pytest.mark.asyncio
async def test_an_ok_that_is_never_followed_by_closed_is_not_a_close():
    """The defect. `ok` says the request arrived; it says nothing about the
    database having been checkpointed."""
    client = _Client(reach_closed=False)

    with pytest.raises(TimeoutError, match="authorizationStateClosed"):
        await client.close()

    assert client._client_id is not None, "a client that never closed was unregistered"


@pytest.mark.asyncio
async def test_a_refused_close_request_still_waits_for_the_state():
    """TDLib may be closing regardless of what it answered, so the state is
    still the question worth asking - and still the one that decides."""
    client = _Client(reach_closed=False, request_fails=True)

    with pytest.raises(TimeoutError):
        await client.close()


@pytest.mark.asyncio
async def test_requests_in_flight_are_settled_rather_than_left_waiting():
    """A closed client can never answer, so a pending future is a caller waiting
    out its own timeout for a reply that was never coming."""
    client = _Client()
    stranded = asyncio.get_running_loop().create_future()
    client._pending["7"] = stranded

    await client.close()

    assert stranded.done()
    with pytest.raises(RuntimeError, match="closed while this was in flight"):
        stranded.result()


@pytest.mark.asyncio
async def test_closing_an_already_closed_client_is_harmless():
    client = _Client()
    await client.close()

    await client.close()


# --- closing all of them -------------------------------------------------------


@pytest.mark.asyncio
async def test_a_client_that_will_not_close_is_retained_not_forgotten():
    """Clearing the registry first left a window in which a new client started
    against a database still being checkpointed - and a client whose close
    failed was forgotten while it still held one."""
    stubborn = _Client(account="a", reach_closed=False)
    reg._by_account["a"] = stubborn

    failures = await reg.close_all(budget=2)

    assert [account for account, _ in failures] == ["a"]
    assert reg._by_account.get("a") is stubborn, "an unclosed client was dropped anyway"


@pytest.mark.asyncio
async def test_a_first_failure_does_not_abandon_the_rest():
    first = _Client(account="a", reach_closed=False)
    second = _Client(account="b")
    reg._by_account.update({"a": first, "b": second})

    failures = await reg.close_all(budget=3)

    assert [account for account, _ in failures] == ["a"]
    assert "b" not in reg._by_account, "the second account was never closed"
    assert second._client_id is None


@pytest.mark.asyncio
async def test_no_new_client_is_started_once_shutdown_has_begun(monkeypatch):
    """The window the old take-and-clear opened: a caller arriving mid-shutdown
    built a fresh native client against a database being flushed."""
    from telegram_mcp import connection as conn

    monkeypatch.setattr(conn, "clients", {"acc": object()})
    monkeypatch.setattr(conn, "refresh_accounts", lambda: [])
    built = []
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: built.append(account))
    reg._closing = True

    with pytest.raises(tdlib.NotSignedIn):
        await reg.secret_client("acc")

    assert built == [], "a native client was started against a database being flushed"


@pytest.mark.asyncio
async def test_the_budget_covers_all_of_them_together(monkeypatch):
    reg._by_account.update({"a": _Client(account="a", reach_closed=False)})
    monkeypatch.setattr(reg, "_CLOSE_ALL_BUDGET", 0.5)

    failures = await reg.close_all(budget=0.01)

    assert failures, "a budget that had already run out reported success"


# --- moving a database aside ---------------------------------------------------


def _database(tmp_path, label="work", contents=b"secret chat keys"):
    directory = identity.database_dir_for(label)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "td.binlog").write_bytes(contents)
    return directory


def test_a_quarantine_is_refused_without_a_confirmed_close(tmp_path):
    """A rename succeeding is not proof the database was released: on POSIX it
    succeeds immediately with the file descriptors still open."""
    directory = _database(tmp_path)

    with pytest.raises(identity.QuarantineFailed, match="not confirmed it closed"):
        identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED")

    assert (directory / "td.binlog").exists()


def test_a_confirmed_close_may_quarantine(tmp_path):
    directory = _database(tmp_path)

    kept = identity.quarantine_database("work", why="AUTH_KEY_UNREGISTERED", closed=True)

    assert not directory.exists()
    assert (kept / "td.binlog").read_bytes() == b"secret chat keys"


@pytest.mark.asyncio
async def test_the_recovery_path_only_quarantines_what_it_saw_close(monkeypatch, tmp_path):
    """The wiring: the recovery branch must carry the close confirmation from
    the attempt that produced the failure, not assume one."""
    _database(tmp_path)
    tdlib._close_confirmed.pop("work", None)

    async def _dead(label, telethon_client, password, ask_password):
        raise tdlib.TDLibError(401, "AUTH_KEY_UNREGISTERED")

    monkeypatch.setattr(tdlib, "_attempt_login", _dead)

    with pytest.raises(identity.QuarantineFailed, match="not confirmed it closed"):
        await tdlib.complete_login("work", object())

    assert list((tmp_path / "tdlib").glob("work.quarantined-*")) == []


@pytest.mark.asyncio
async def test_the_recovery_path_proceeds_when_the_close_was_confirmed(monkeypatch, tmp_path):
    _database(tmp_path)
    attempts = []

    async def _then_ready(label, telethon_client, password, ask_password):
        attempts.append(label)
        tdlib._close_confirmed[label] = True
        if len(attempts) == 1:
            raise tdlib.TDLibError(401, "AUTH_KEY_UNREGISTERED")
        return "authorizationStateReady"

    monkeypatch.setattr(tdlib, "_attempt_login", _then_ready)

    assert await tdlib.complete_login("work", object()) == "authorizationStateReady"
    assert len(list((tmp_path / "tdlib").glob("work.quarantined-*"))) == 1


# --- the metadata write --------------------------------------------------------


def test_the_owner_file_is_written_and_readable(tmp_path):
    _database(tmp_path)

    identity.record_identity("work", 4242)

    written = json.loads(identity.identity_path("work").read_text(encoding="utf-8"))
    assert written["user_id"] == 4242
    assert not list(identity.database_dir_for("work").glob("*.tmp")), "a partial write survived"


def test_the_temporary_name_is_not_predictable(tmp_path):
    """A fixed `owner.json.tmp` is a name an attacker can occupy in advance."""
    _database(tmp_path)
    seen = set()

    real_open = os.open

    def _watching(path, flags, mode=0o777, **kwargs):
        seen.add(str(path))
        return real_open(path, flags, mode, **kwargs)

    import telegram_mcp.tdlib_identity as mod

    original = mod.os.open
    mod.os.open = _watching
    try:
        identity.record_identity("work", 1)
        identity.record_identity("work", 2)
    finally:
        mod.os.open = original

    temporaries = {name for name in seen if name.endswith(".tmp")}
    assert len(temporaries) == 2, f"the temporary name repeated: {temporaries}"


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX symlink semantics")
def test_a_planted_link_is_not_followed(tmp_path):
    """The write is exclusive and does not follow links, so a name someone else
    put there fails the open rather than redirecting the contents."""
    _database(tmp_path)
    victim = tmp_path / "victim.txt"
    victim.write_text("do not overwrite me", encoding="utf-8")
    planted = identity.identity_path("work").with_name("owner.json.planted.tmp")
    planted.symlink_to(victim)

    identity.record_identity("work", 99)

    assert victim.read_text(encoding="utf-8") == "do not overwrite me"


def test_an_existing_name_never_has_its_contents_replaced(tmp_path):
    """O_EXCL: the open fails outright if anything is already at that path, so a
    file this process did not create is never written through."""
    _database(tmp_path)
    occupied = identity.database_dir_for("work") / "squatter.tmp"
    occupied.write_text("someone else's file", encoding="utf-8")

    with pytest.raises(FileExistsError):
        handle = os.open(
            occupied, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600
        )
        os.close(handle)

    assert occupied.read_text(encoding="utf-8") == "someone else's file"


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX file mode")
def test_the_owner_file_is_owner_only(tmp_path):
    _database(tmp_path)

    identity.record_identity("work", 1)

    assert (identity.identity_path("work").stat().st_mode & 0o077) == 0
