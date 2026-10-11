"""Authorized activation without a secret tool call."""

import pytest


@pytest.fixture(autouse=True)
def activation_state(monkeypatch):
    from telegram_mcp import secret_activation

    monkeypatch.setattr(secret_activation, "_closing", False)
    monkeypatch.setattr(secret_activation, "_failures", {})
    monkeypatch.setattr(secret_activation, "_tasks", set())
    monkeypatch.setattr(secret_activation, "_sweep", None)


@pytest.mark.asyncio
async def test_activation_only_starts_current_authorized_clients(monkeypatch):
    from telegram_mcp import secret_activation as activation, connection, secret_backend

    client = object()
    monkeypatch.setattr(connection, "clients", {"acct": client})
    observed = []

    async def start(account):
        observed.append(account)
        return object()

    monkeypatch.setattr(secret_backend, "secret_manager", start)
    await activation.activate("acct", client)
    await activation.activate("retired", object())
    assert observed == ["acct"]
    await activation.stop()


@pytest.mark.asyncio
async def test_idle_sweep_pauses_aged_custody_without_deleting_it(tmp_path, monkeypatch):
    import time
    from telegram_mcp import secret_activation as activation, secret_backend
    from test_secret_custody import seed_store, build_manager, CHAT, WIRE_SEQ

    store = seed_store(tmp_path / "protocol.json")
    manager = build_manager(store)
    await manager.start()
    # Advance operational time only after healthy startup, so the idle sweep
    # (not startup admission) is the first operation observing expiry.
    monkeypatch.setattr(time, "time", lambda now=time.time(): now + 4000)
    monkeypatch.setattr(secret_backend, "_by_account", {"acct": manager})
    monkeypatch.setattr(secret_backend, "_stops", {})
    monkeypatch.setattr(activation, "_closing", False)

    async def finish_sweep(seconds):
        activation._closing = True

    monkeypatch.setattr(activation.asyncio, "sleep", finish_sweep)
    try:
        await activation._custody_sweep()
        status = manager.pause_status(CHAT)
        assert status.paused and status.reason == "age"
        assert [item.seq_no for item in manager.pending_deliveries(CHAT)] == [WIRE_SEQ]
        assert store.list() == [CHAT]
    finally:
        await manager.stop()


@pytest.mark.asyncio
async def test_registry_change_starts_only_current_generation_and_stops_owned_tasks(monkeypatch):
    import asyncio
    from telegram_mcp import secret_activation as activation, connection, secret_backend

    old, new = object(), object()
    monkeypatch.setattr(connection, "clients", {"acct": new})
    observed = []
    ready = asyncio.Event()

    async def start(account):
        observed.append(connection.clients[account])
        ready.set()
        return object()

    monkeypatch.setattr(secret_backend, "secret_manager", start)
    activation.changed({"acct"}, set())
    await asyncio.wait_for(ready.wait(), 1)
    await activation.activate("acct", old)
    assert observed == [new]
    await activation.stop()
    assert not activation._tasks


@pytest.mark.asyncio
async def test_activation_failure_is_reported_without_removing_ordinary_account(monkeypatch):
    from telegram_mcp import secret_activation as activation, connection, secret_backend

    client = object()
    monkeypatch.setattr(connection, "clients", {"acct": client})

    async def fail(account):
        raise PermissionError("synthetic archive refusal")

    monkeypatch.setattr(secret_backend, "secret_manager", fail)
    await activation.activate("acct", client)
    assert activation._failures == {"acct": "PermissionError"}
    assert connection.clients == {"acct": client}


@pytest.mark.asyncio
async def test_removed_generation_retirement_cannot_stop_readded_manager(monkeypatch):
    import asyncio
    from telegram_mcp import (
        secret_activation as activation,
        secret_backend as backend,
        secret_control,
    )

    old, new = object(), object()
    blocker = asyncio.create_task(asyncio.Event().wait())
    monkeypatch.setattr(backend, "_by_account", {"acct": new})
    monkeypatch.setattr(secret_control, "_locks", {})
    stopped = []

    def stop(account, manager):
        stopped.append(manager)
        return asyncio.create_task(asyncio.sleep(0))

    monkeypatch.setattr(backend, "_stop_owned", stop)
    await activation._retire("acct", old, blocker)
    assert stopped == [] and backend._by_account["acct"] is new


@pytest.mark.asyncio
async def test_client_disconnect_waits_for_confirmed_secret_manager_stop(monkeypatch):
    import asyncio
    from telegram_mcp import retirement, secret_backend as backend

    actions = []

    class Client:
        async def disconnect(self):
            actions.append("disconnect")

    client = Client()
    manager = object()
    monkeypatch.setattr(backend, "_by_account", {"acct": manager})
    monkeypatch.setattr(backend, "_manager_clients", {id(manager): client})

    async def stop():
        actions.append("manager-stop")

    monkeypatch.setattr(backend, "_stop_owned", lambda account, owner: asyncio.create_task(stop()))
    closing = retirement.retire(client)
    await asyncio.wait_for(closing, 1)
    assert actions == ["manager-stop", "disconnect"]


@pytest.mark.asyncio
async def test_old_client_retirement_never_cancels_new_generation_start(monkeypatch):
    import asyncio
    from telegram_mcp import retirement, secret_backend as backend

    actions = []

    class Client:
        async def disconnect(self):
            actions.append("disconnect")

    client = Client()
    old = object()
    monkeypatch.setattr(backend, "_by_account", {"acct": old})
    monkeypatch.setattr(backend, "_manager_clients", {id(old): client})
    starts = {}
    monkeypatch.setattr(backend, "_starts", starts)

    async def stop():
        actions.append("stop")

    monkeypatch.setattr(backend, "_stop_owned", lambda account, owner: asyncio.create_task(stop()))
    retirement_task = retirement.retire(client)
    # Newly published activation appears before the deferred retirement runs.
    release = asyncio.Event()
    new_start = asyncio.create_task(release.wait())
    starts["acct"] = new_start
    try:
        await asyncio.wait_for(retirement_task, 1)
        assert not new_start.cancelled() and actions == ["stop", "disconnect"]
    finally:
        release.set()
        await new_start
