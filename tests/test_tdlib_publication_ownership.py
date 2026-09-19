"""Regression checks for TDLib's final publication and native-close ownership."""

import asyncio
from types import SimpleNamespace

import pytest

from telegram_mcp import tdlib_registry as reg


@pytest.fixture
def native_env(monkeypatch):
    from telegram_mcp import connection, tdlib, tdlib_identity

    active = {"work": object()}
    made = []

    class Native:
        def __init__(self, account):
            self.account = account
            self._client_id = 1
            self.authorization_state = "authorizationStateReady"
            self.closed = False
            self.close_calls = 0
            self.close_gate = None
            self.close_entered = asyncio.Event()
            self.start_error = None
            self.close_error = None
            made.append(self)

        async def start(self):
            if self.start_error:
                raise self.start_error
            return self.authorization_state

        async def close(self):
            self.close_calls += 1
            self.close_entered.set()
            if self.close_gate is not None:
                await self.close_gate.wait()
            if self.close_error:
                raise self.close_error
            self.closed = True
            self._client_id = None

    async def verify(account, client, telethon):
        return 111

    monkeypatch.setattr(reg, "_by_account", {})
    monkeypatch.setattr(reg, "_verified_against", {})
    monkeypatch.setattr(reg, "_by_account_lock", asyncio.Lock())
    monkeypatch.setattr(reg, "_closing", False)
    if hasattr(reg, "_close_tasks"):
        monkeypatch.setattr(reg, "_close_tasks", {})
    monkeypatch.setattr(connection, "get_client", lambda account: active[account])
    monkeypatch.setattr(tdlib, "TDLibClient", Native)
    monkeypatch.setattr(tdlib_identity, "verify_owner", verify)
    return SimpleNamespace(active=active, made=made, Native=Native, identity=tdlib_identity)


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["generation", "shutdown", "authorization", "removal"])
async def test_final_verification_cannot_publish_an_obsolete_client(
    native_env, monkeypatch, change
):
    entered, release = asyncio.Event(), asyncio.Event()

    async def verify(account, client, telethon):
        entered.set()
        await release.wait()

    monkeypatch.setattr(native_env.identity, "verify_owner", verify)
    task = asyncio.create_task(reg.secret_client("work"))
    await asyncio.wait_for(entered.wait(), 2)
    if change == "generation":
        native_env.active["work"] = object()
    elif change == "shutdown":
        reg._closing = True
    elif change == "authorization":
        native_env.made[0].authorization_state = "authorizationStateWaitPassword"
    else:
        native_env.active.pop("work")
    release.set()
    with pytest.raises(Exception):
        await asyncio.wait_for(task, 2)
    assert native_env.made[0].closed
    assert "work" not in reg._by_account
    assert "work" not in reg._verified_against


@pytest.mark.asyncio
async def test_cancellation_during_old_close_is_not_relabelled_as_login_failure(native_env):
    old = native_env.Native("work")
    old.close_gate = asyncio.Event()
    reg._by_account["work"] = old
    reg._verified_against["work"] = object()
    task = asyncio.create_task(reg.secret_client("work"))
    await asyncio.wait_for(old.close_entered.wait(), 2)
    task.cancel()
    try:
        with pytest.raises(asyncio.CancelledError):
            await task
        assert reg._by_account["work"] is old
    finally:
        old.close_gate.set()
        await asyncio.sleep(0)
        await asyncio.sleep(0)


@pytest.mark.asyncio
@pytest.mark.parametrize("start_kind", ["error", "incomplete"])
async def test_failed_new_owner_is_retained_when_cleanup_fails(
    native_env, monkeypatch, start_kind
):
    async def start(self):
        self.close_error = RuntimeError("synthetic close failure")
        if start_kind == "error":
            raise RuntimeError("synthetic start failure")
        self.authorization_state = "authorizationStateWaitCode"
        return self.authorization_state

    monkeypatch.setattr(native_env.Native, "start", start)
    with pytest.raises(Exception):
        await reg.secret_client("work")
    old = native_env.made[0]
    assert reg._by_account.get("work") is old
    assert "work" not in reg._verified_against
    with pytest.raises(Exception, match="did not close"):
        await reg.secret_client("work")
    assert native_env.made == [old]


@pytest.mark.asyncio
async def test_removal_during_start_still_closes_the_new_native_owner(native_env, monkeypatch):
    async def start(self):
        native_env.active.pop("work")
        return self.authorization_state

    monkeypatch.setattr(native_env.Native, "start", start)
    with pytest.raises(Exception):
        await reg.secret_client("work")
    assert native_env.made[0].closed


@pytest.mark.asyncio
async def test_repeated_shutdown_reuses_and_reaps_a_late_close(native_env):
    old = native_env.Native("work")
    old.close_gate = asyncio.Event()
    reg._by_account["work"] = old
    reg._verified_against["work"] = native_env.active["work"]
    try:
        first = await reg.close_all(budget=0.02)
        second = await reg.close_all(budget=0.02)
        assert [name for name, _ in first] == ["work"]
        assert [name for name, _ in second] == ["work"]
        assert old.close_calls == 1
    finally:
        old.close_gate.set()
        await asyncio.sleep(0)
        await asyncio.sleep(0)
    assert "work" not in reg._by_account
    assert "work" not in reg._verified_against


@pytest.mark.asyncio
async def test_unchanged_generation_is_cached_without_recreating_it(native_env):
    first = await reg.secret_client("work")
    second = await reg.secret_client("work")
    assert first is second
    assert len(native_env.made) == 1
    assert await reg.close_all() == []
    assert first.closed
