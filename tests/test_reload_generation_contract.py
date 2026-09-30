"""Exercise revision changes through the actual file reader and refresh caller."""

import asyncio
from types import SimpleNamespace

import pytest

from telegram_mcp import account_config, account_lifecycle as life
from telegram_mcp import account_snapshot as snap, admission, connection as conn
from telegram_mcp.tools import events


class Client:
    def __init__(self, value):
        self.value = value
        self.handlers = []
        self.connected = False
        self.disconnected = False

    async def connect(self):
        self.connected = True

    async def is_user_authorized(self):
        return True

    async def disconnect(self):
        self.disconnected = True
        self.connected = False

    def add_event_handler(self, callback, *args):
        self.handlers.append(callback)

    def remove_event_handler(self, callback):
        if callback in self.handlers:
            self.handlers.remove(callback)


class Lock:
    def __init__(self, identity):
        self.identity = identity
        self.released = False

    def acquire(self, **kwargs):
        pass

    def release(self):
        self.released = True


@pytest.fixture
def reload_state(monkeypatch, tmp_path):
    file = tmp_path / ".env"
    registry = {"work": Client("A")}
    built = {}
    key = "TELEGRAM_SESSION_STRING_WORK"
    file.write_text(f"{key}=A\n")
    monkeypatch.setattr(account_config, "_env_file", lambda: str(file))
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.setattr(snap, "_accepted_path", None)
    initial = snap.read_snapshot()
    monkeypatch.setattr(conn, "clients", registry)
    monkeypatch.setattr(conn, "_env_stamp", initial.stamp)
    monkeypatch.setattr(conn, "_env_digests", initial.digests)
    monkeypatch.setattr(conn, "_registry_listeners", [events._on_clients_changed])
    monkeypatch.setattr(events, "clients", registry)
    monkeypatch.setattr(events, "_incoming_handlers", {})
    for name in ("_pending", "_tasks", "_deferred"):
        monkeypatch.setattr(life, name, {}, raising=False)
    monkeypatch.setattr(life, "_admissions", set())
    monkeypatch.setattr(life, "_activated", set())
    monkeypatch.setattr(life, "_rejection", None)
    for name in (
        "_active",
        "_retiring",
        "session_locks",
        "_awaiting_admission",
        "_admitting",
        "_claims",
    ):
        monkeypatch.setattr(admission, name, {}, raising=False)
    monkeypatch.setattr(admission, "_releasing", set())
    monkeypatch.setattr(admission, "_stopped", False)
    monkeypatch.setattr(admission, "SessionLock", Lock)
    monkeypatch.setattr(admission, "session_identity", lambda c: c.value)

    def discover(env, reuse=None):
        result = {}
        for name, value in env.items():
            if not name.startswith("TELEGRAM_SESSION_STRING_"):
                continue
            label = name.removeprefix("TELEGRAM_SESSION_STRING_").lower()
            result[label] = (reuse or {}).get(label) or Client(value)
            built[value] = result[label]
        return result

    monkeypatch.setattr(conn, "_discover_accounts", discover)
    events.register_incoming_handlers()

    def write(value, other=None):
        text = f"{key}={value}\n" if value is not None else ""
        if other is not None:
            text += f"TELEGRAM_SESSION_STRING_OTHER={other}\n"
        file.write_text(text)
        return conn.refresh_accounts()

    return SimpleNamespace(write=write, registry=registry, built=built, file=file, key=key)


async def barrier():
    # Run an actual scheduled marker, not a guessed delay.
    event = asyncio.Event()
    asyncio.get_running_loop().call_soon(event.set)
    await event.wait()


@pytest.mark.asyncio
@pytest.mark.parametrize("revision", ["C", "A", None])
async def test_late_candidate_cannot_overwrite_reversion_or_removal(
    reload_state, monkeypatch, revision
):
    entered, release = asyncio.Event(), asyncio.Event()
    old = reload_state.registry["work"]

    async def connect(label, client):
        if client.value == "B":
            entered.set()
            try:
                await release.wait()
            except asyncio.CancelledError:
                # The post-await generation check is necessary even if a
                # dependency finishes its connect despite caller cancellation.
                await release.wait()
        client.connected = True

    monkeypatch.setattr(life, "_connect_and_authorize", connect)
    reload_state.write("B")
    await asyncio.wait_for(entered.wait(), 2)
    reload_state.write(revision, other="X" if revision is None else None)
    await barrier()
    release.set()
    await life.drain(timeout=2)
    if revision is None:
        assert "work" not in reload_state.registry
    elif revision == "A":
        assert reload_state.registry["work"] is old
        assert not old.disconnected
    else:
        assert reload_state.registry["work"].value == "C"
    assert reload_state.built["B"].disconnected
    if revision is not None:
        assert conn._env_digests[reload_state.key] == snap.account_digest(revision)


@pytest.mark.asyncio
async def test_replacement_notifies_real_incoming_handler_at_commit(reload_state):
    previous = reload_state.registry["work"]
    reload_state.write("B")
    await life.drain(timeout=2)
    current = reload_state.registry["work"]
    assert current is not previous
    assert len(current.handlers) == 1
    assert not previous.handlers
    assert events._incoming_handlers["work"][0] is current


@pytest.mark.asyncio
async def test_new_account_is_not_active_before_authorization(reload_state, monkeypatch):
    entered, release = asyncio.Event(), asyncio.Event()

    async def connect(label, client):
        entered.set()
        await release.wait()

    monkeypatch.setattr(life, "_connect_and_authorize", connect)
    reload_state.write("A", other="X")
    await asyncio.wait_for(entered.wait(), 2)
    try:
        assert "other" not in reload_state.registry
        assert "TELEGRAM_SESSION_STRING_OTHER" not in conn._env_digests
    finally:
        release.set()
        await life.drain(timeout=2)
    assert reload_state.registry["other"] is reload_state.built["X"]


@pytest.mark.asyncio
async def test_first_use_waits_for_existing_full_admission(reload_state, monkeypatch):
    entered, release = asyncio.Event(), asyncio.Event()

    async def connect(label, client):
        entered.set()
        await release.wait()

    monkeypatch.setattr(life, "_connect_and_authorize", connect)
    reload_state.write("A", other="X")
    await asyncio.wait_for(entered.wait(), 2)
    pending = asyncio.create_task(admission.admit_if_pending(reload_state.built["X"]))
    await barrier()
    try:
        assert not pending.done(), "lease acquired is not authorization complete"
    finally:
        release.set()
        await asyncio.wait_for(pending, 2)
        await life.drain(timeout=2)


def test_replaced_source_key_does_not_leave_a_phantom_active_digest(monkeypatch):
    monkeypatch.setattr(conn, "_env_digests", {"TELEGRAM_SESSION_NAME_WORK": "old"})
    conn.record_activated({"work": object()}, {"TELEGRAM_SESSION_STRING_WORK": "new"}, {"work"})
    assert conn._env_digests == {"TELEGRAM_SESSION_STRING_WORK": "new"}


@pytest.mark.asyncio
async def test_shutdown_boundary_prevents_a_finished_connect_publishing(reload_state, monkeypatch):
    entered, release = asyncio.Event(), asyncio.Event()
    old = reload_state.registry["work"]

    async def connect(label, client):
        entered.set()
        await release.wait()

    monkeypatch.setattr(life, "_connect_and_authorize", connect)
    reload_state.write("B")
    await asyncio.wait_for(entered.wait(), 2)
    admission._stopped = True
    release.set()
    await life.drain(timeout=2)
    assert reload_state.registry["work"] is old
    assert reload_state.built["B"].disconnected


@pytest.mark.asyncio
async def test_disposed_candidate_cannot_publish_a_late_threaded_claim(reload_state, monkeypatch):
    import threading

    entered, release = threading.Event(), threading.Event()

    def acquire(self, **kwargs):
        entered.set()
        if not release.wait(2):
            raise TimeoutError("test failed to release acquisition")

    monkeypatch.setattr(Lock, "acquire", acquire)
    candidate = Client("delayed")
    task = asyncio.create_task(life.admit(life.Staged("new", candidate), reload_state.registry))
    assert await asyncio.to_thread(entered.wait, 1)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert candidate.disconnected
    release.set()
    # Await the actual owned operation when available, not only the caller.
    owners = list(getattr(admission, "_claims", {}).values())
    if owners:
        await asyncio.gather(*owners, return_exceptions=True)
    else:
        await asyncio.sleep(0.1)
    assert all(lease.client is not candidate for lease in admission._active.values())
    assert all(lease.client is not candidate for lease in admission._retiring.values())


@pytest.mark.asyncio
async def test_a_close_confirmed_after_its_deadline_is_reaped(reload_state, monkeypatch):
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.01)
    client = Client("late-close")
    await admission.claim_session("work", client)
    lease = admission._active["work"]
    closed = asyncio.get_running_loop().create_future()
    admission.forget("work", closing=closed, client=client)
    await admission.drain_releases(timeout=1)
    assert not lease.lock.released
    closed.set_result(None)
    await barrier()
    assert lease.lock.released
    assert id(lease) not in admission._retiring


def test_forgetting_old_client_does_not_erase_new_pending_owner(reload_state):
    old, fresh = Client("old"), Client("fresh")
    admission.mark_awaiting_admission({"work": fresh})
    admission.forget("work", client=old)
    assert admission._awaiting_admission["work"] is fresh


def test_synchronous_retirement_refusal_retains_the_actual_lease(reload_state):
    from telegram_mcp import retirement

    class Refuses(Client):
        def disconnect(self):
            raise OSError("synthetic synchronous close refusal")

    client = Refuses("sync-refusal")
    asyncio.run(admission.claim_session("work", client))
    lease = admission._active["work"]
    admission.forget("work", client=client, closing=retirement.retire(client))
    assert not lease.lock.released
    assert admission._retiring[id(lease)] is lease


def test_cold_discovery_resumes_one_full_activation_on_the_serving_loop(reload_state):
    reload_state.write("A", other="X")
    candidate = reload_state.registry["other"]
    assert not candidate.connected

    async def use():
        await admission.admit_if_pending(candidate)
        await life.drain(timeout=2)
        assert candidate.connected
        assert admission._active["other"].client is candidate
        assert events._incoming_handlers["other"][0] is candidate

    asyncio.run(use())


@pytest.mark.asyncio
async def test_shutdown_releases_only_positively_confirmed_clients(reload_state):
    first, second = Client("confirmed"), Client("unconfirmed")
    await admission.claim_session("first", first)
    await admission.claim_session("second", second)
    first_lease, second_lease = admission._active["first"], admission._active["second"]
    admission.release_all(confirmed_clients={id(first)})
    assert first_lease.lock.released
    assert not second_lease.lock.released
    assert admission._active["second"] is second_lease
    assert "second" in admission.unreleased_leases
