"""Own initialization, identity proof and closure over real package managers."""

import asyncio
import json
from types import SimpleNamespace

import pytest
import pytest_asyncio

from telegram_mcp import secret_backend as backend
from telegram_mcp.singleton import SessionLock, SessionLockError
from test_secret_backend import _Client


@pytest_asyncio.fixture
async def state(monkeypatch, tmp_path):
    from telethon_secret_chat import MemoryStorage

    active = {"acct": _Client(user_id=111)}
    for name in (
        "_by_account",
        "_verified_against",
        "_store_locks",
        "_starts",
        "_stops",
        "_acquisitions",
    ):
        monkeypatch.setattr(backend, name, {}, raising=False)
    monkeypatch.setattr(backend, "_lock", asyncio.Lock())
    monkeypatch.setattr(backend, "_closing", False)
    monkeypatch.setattr(backend, "_telethon_client", lambda account: active[account])
    monkeypatch.setattr(backend, "_storage_for", lambda account: MemoryStorage())
    monkeypatch.setattr(backend, "_owner_path", lambda account: tmp_path / f"{account}.owner.json")
    monkeypatch.setattr(backend, "_store_lock_dir", lambda: tmp_path)
    yield SimpleNamespace(active=active, root=tmp_path)
    # Unconditional fixture cleanup is not a production success assertion.
    for name in ("_starts", "_stops", "_acquisitions"):
        tasks = [t for t in getattr(backend, name, {}).values() if isinstance(t, asyncio.Future)]
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
    for lock in list(backend._store_locks.values()):
        lock.release()


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["binding", "start"])
@pytest.mark.parametrize("change", ["replace", "remove", "shutdown"])
async def test_no_obsolete_manager_published_after_await(state, monkeypatch, phase, change):
    entered, release = asyncio.Event(), asyncio.Event()
    original = backend.SecretChatManager.start

    if phase == "binding":

        async def get_me(input_peer=False):
            entered.set()
            await release.wait()
            return SimpleNamespace(user_id=111)

        monkeypatch.setattr(state.active["acct"], "get_me", get_me)
    else:

        async def start(manager):
            await original(manager)
            entered.set()
            await release.wait()

        monkeypatch.setattr(backend.SecretChatManager, "start", start)
    client = state.active["acct"]
    pending = asyncio.create_task(backend.secret_manager("acct"))
    await asyncio.wait_for(entered.wait(), 2)
    if change == "replace":
        state.active["acct"] = _Client(user_id=222)
    elif change == "remove":
        del state.active["acct"]
    else:
        backend._closing = True
    release.set()
    with pytest.raises((backend.SecretChatUnavailable, KeyError)):
        await asyncio.wait_for(pending, 2)
    assert not client.handlers
    assert not backend._by_account
    assert not backend._store_locks


@pytest.mark.asyncio
async def test_failed_start_cleans_real_subscription_and_store(state, monkeypatch):
    original = backend.SecretChatManager.start

    async def start(manager):
        await original(manager)
        raise OSError("synthetic startup failure after subscription")

    monkeypatch.setattr(backend.SecretChatManager, "start", start)
    with pytest.raises(OSError):
        await backend.secret_manager("acct")
    assert not state.active["acct"].handlers
    assert not backend._by_account
    assert not backend._store_locks


@pytest.mark.asyncio
async def test_stop_failure_retains_owner_and_retry_releases_it(state, monkeypatch):
    manager = await backend.secret_manager("acct")
    original = manager.stop

    async def refuse():
        raise OSError("synthetic flush failure")

    monkeypatch.setattr(manager, "stop", refuse)
    failures = await backend.close_all()
    assert [label for label, _ in failures] == ["acct"]
    assert backend._by_account.get("acct") is manager
    competitor = SessionLock(backend._store_identity("acct"), lock_dir=state.root)
    with pytest.raises(SessionLockError):
        competitor.acquire(grace_seconds=0, poll_interval=0.01)
    monkeypatch.setattr(manager, "stop", original)
    assert await backend.close_all() == []
    assert not backend._by_account
    assert not backend._store_locks
    competitor.acquire(grace_seconds=0, poll_interval=0.01)
    competitor.release()


@pytest.mark.asyncio
async def test_cancelled_waiter_does_not_orphan_shared_start(state, monkeypatch):
    entered, release = asyncio.Event(), asyncio.Event()
    original = backend.SecretChatManager.start
    calls = []

    async def start(manager):
        calls.append(manager)
        await original(manager)
        entered.set()
        await release.wait()

    monkeypatch.setattr(backend.SecretChatManager, "start", start)
    first = asyncio.create_task(backend.secret_manager("acct"))
    await asyncio.wait_for(entered.wait(), 2)
    second = asyncio.create_task(backend.secret_manager("acct"))
    first.cancel()
    with pytest.raises(asyncio.CancelledError):
        await first
    release.set()
    manager = await asyncio.wait_for(second, 2)
    assert calls == [manager]
    assert len(state.active["acct"].handlers) == 1
    await backend.close_all()


@pytest.mark.asyncio
@pytest.mark.parametrize("identity", [None, 0, -1, True])
async def test_unknown_identity_does_not_adopt_store(state, identity):
    state.active["acct"].user_id = identity
    with pytest.raises(backend.SecretChatUnavailable):
        await backend.secret_manager("acct")
    assert not backend._owner_path("acct").exists()
    assert not backend._store_locks


@pytest.mark.asyncio
@pytest.mark.parametrize("payload", [b"broken", b"[]", b'{"user_id":null}', b'{"user_id":true}'])
async def test_invalid_owner_is_preserved_not_adopted(state, payload):
    path = backend._owner_path("acct")
    path.write_bytes(payload)
    with pytest.raises(backend.SecretChatUnavailable):
        await backend.secret_manager("acct")
    assert path.read_bytes() == payload
    assert not backend._store_locks


@pytest.mark.asyncio
async def test_failing_owner_read_preserves_bytes_and_releases_unused_lease(state, monkeypatch):
    from pathlib import Path

    path = backend._owner_path("acct")
    payload = json.dumps({"user_id": 111})
    path.write_text(payload)
    original = Path.read_text

    def unreadable(self, *args, **kwargs):
        if self == path:
            raise PermissionError("synthetic owner read refusal")
        return original(self, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", unreadable)
    with pytest.raises((OSError, backend.SecretChatUnavailable)):
        await backend.secret_manager("acct")
    assert path.read_bytes() == payload.encode()
    assert not backend._store_locks


@pytest.mark.asyncio
async def test_shutdown_attempts_every_manager_under_one_budget(state, monkeypatch):
    state.active["other"] = _Client(user_id=222)
    first = await backend.secret_manager("acct")
    other = await backend.secret_manager("other")
    gate = asyncio.Event()
    original = first.stop
    calls = []

    async def slow_stop():
        calls.append(1)
        await gate.wait()
        await original()

    monkeypatch.setattr(first, "stop", slow_stop)
    try:
        failures = await asyncio.wait_for(backend.close_all(budget=0.05), 1)
        assert [label for label, _ in failures] == ["acct"]
        assert not other._running
        assert backend._by_account["acct"] is first
        await backend.close_all(budget=0.05)
        assert len(calls) == 1
    finally:
        gate.set()
    assert await backend.close_all(budget=1) == []
    assert not backend._store_locks


@pytest.mark.asyncio
async def test_owner_permission_refusal_releases_unused_store(state, monkeypatch):
    monkeypatch.setattr(backend, "restrict_to_owner_strict", lambda path: False)
    with pytest.raises(OSError, match="owner-only"):
        await backend.secret_manager("acct")
    assert not backend._owner_path("acct").exists()
    assert not backend._store_locks
    assert not list(state.root.glob("*.owner.tmp"))
