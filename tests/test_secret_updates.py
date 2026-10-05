"""State-driven secret recovery over the existing client's native update box."""

import asyncio
import datetime
import logging
from types import SimpleNamespace

import pytest
from telethon._updates import GapError, MessageBox
from telethon._updates.session import SessionState
from telethon.tl import functions, types


class Client:
    def __init__(self):
        self._message_box = MessageBox(logging.getLogger("synthetic.secret-updates"))
        self._message_box.load(SessionState(1, 0, False, 10, 20, 1, 30, None), [])
        self._updates_queue = asyncio.Queue()
        self._no_updates = False
        self.requests = []
        self.catchups = 0
        self.remote = types.updates.State(
            10, 19, datetime.datetime.now(datetime.timezone.utc), 31, 0
        )

    def is_connected(self):
        return True

    async def __call__(self, request):
        assert isinstance(request, functions.updates.GetStateRequest)
        self.requests.append(request)
        return self.remote

    async def catch_up(self):
        from telethon.client.updates import UpdateMethods

        self.catchups += 1
        await UpdateMethods.catch_up(self)


class Manager:
    def __init__(self):
        self.chats = [SimpleNamespace(state=SimpleNamespace(value="ready"))]
        self.handlers = {}

    def list(self):
        return list(self.chats)

    def on(self, event, handler):
        self.handlers[event] = handler


@pytest.mark.asyncio
async def test_missing_push_is_recovered_without_regressing_local_qts():
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    assert await recovery.check() is True
    assert client.catchups == 1
    marker = await asyncio.wait_for(client._updates_queue.get(), 1)
    processed = []
    with pytest.raises(GapError):
        client._message_box.process_updates(
            marker, SimpleNamespace(extend=lambda *args: None), processed
        )
    request = client._message_box.get_difference()
    assert request.qts == 20, "An older remote qts must not overwrite locally received messages"
    terminal = types.UpdateEncryption(
        types.EncryptedChatDiscarded(123, history_deleted=True), client.remote.date
    )
    diff = types.updates.Difference(
        [], [], [terminal], [], [], types.updates.State(10, 20, client.remote.date, 31, 0)
    )
    updates, _, _ = client._message_box.apply_difference(
        diff, SimpleNamespace(extend=lambda *args: None)
    )
    assert terminal in updates, "The native difference path must deliver the missed terminal"
    assert client._message_box.session_state()[0]["seq"] == 31


@pytest.mark.asyncio
async def test_restored_active_chat_starts_one_owned_worker_and_stop_drains_it(monkeypatch):
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    called = asyncio.Event()
    original = client.__class__.__call__

    async def observed(self, request):
        result = await original(self, request)
        called.set()
        return result

    monkeypatch.setattr(Client, "__call__", observed)
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    first = recovery.start()
    assert recovery.start() is first
    await asyncio.wait_for(called.wait(), 1)
    await recovery.stop()
    assert first.done()
    assert len(client.requests) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "reason",
    ["idle", "closed", "stale", "disabled", "inflight", "empty", "disconnected", "queued"],
)
async def test_recovery_does_not_request_when_account_cannot_receive(reason):
    from telegram_mcp.secret_updates import SecretUpdateRecovery
    from telethon._updates.messagebox import ENTRY_ACCOUNT

    client, manager = Client(), Manager()
    current = reason != "stale"
    if reason == "idle":
        manager.chats.clear()
    elif reason == "closed":
        manager.chats[0].state.value = "closed"
    elif reason == "disabled":
        client._no_updates = True
    elif reason == "inflight":
        client._message_box.try_begin_get_diff(ENTRY_ACCOUNT, "synthetic existing gap")
    elif reason == "empty":
        client._message_box.map.clear()
    elif reason == "disconnected":
        client.is_connected = lambda: False
    elif reason == "queued":
        client._updates_queue.put_nowait(types.UpdatesTooLong())
    recovery = SecretUpdateRecovery(client, manager, lambda: current, "acct")
    assert await recovery.check() is False
    assert client.requests == [] and client.catchups == 0


@pytest.mark.asyncio
async def test_older_state_never_replaces_local_state_or_requests_difference():
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    client.remote.seq = 29
    before = client._message_box.session_state()
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    assert await recovery.check() is False
    assert client._message_box.session_state() == before
    assert client.catchups == 0


@pytest.mark.asyncio
async def test_pending_queue_marker_prevents_duplicate_recovery_requests():
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    assert await recovery.check() is True
    assert await recovery.check() is False
    assert client.catchups == 1 and client._updates_queue.qsize() == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["generation", "closed", "caught_up", "box", "queued"])
async def test_state_response_does_not_recover_an_obsolete_generation(monkeypatch, change):
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    entered, release = asyncio.Event(), asyncio.Event()
    current = True
    original = Client.__call__

    async def held(self, request):
        entered.set()
        await release.wait()
        return await original(self, request)

    monkeypatch.setattr(Client, "__call__", held)
    recovery = SecretUpdateRecovery(client, manager, lambda: current, "acct")
    task = asyncio.create_task(recovery.check())
    try:
        await asyncio.wait_for(entered.wait(), 1)
        if change == "generation":
            current = False
        elif change == "closed":
            manager.chats.clear()
        elif change == "caught_up":
            client._message_box.seq = 31
        elif change == "queued":
            client._updates_queue.put_nowait(types.UpdatesTooLong())
        else:
            client._message_box = Client()._message_box
        release.set()
        assert await asyncio.wait_for(task, 1) is False
        assert client.catchups == 0
    finally:
        release.set()
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
async def test_idle_manager_wakes_only_for_an_active_chat_and_stops_inflight(monkeypatch):
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    manager.chats.clear()
    entered = asyncio.Event()
    cancelled = asyncio.Event()

    async def held(self, request):
        entered.set()
        try:
            await asyncio.Event().wait()
        finally:
            cancelled.set()

    monkeypatch.setattr(Client, "__call__", held)
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    task = recovery.start()
    await asyncio.sleep(0)
    assert not entered.is_set()
    manager.chats.append(SimpleNamespace(state=SimpleNamespace(value="ready")))
    manager.handlers["ChatReady"](None)
    try:
        await asyncio.wait_for(entered.wait(), 1)
    finally:
        await recovery.stop()
    assert cancelled.is_set() and task.done()


@pytest.mark.asyncio
async def test_state_request_timeout_is_bounded(monkeypatch):
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    async def held(self, request):
        await asyncio.Event().wait()

    monkeypatch.setattr(Client, "__call__", held)
    recovery = SecretUpdateRecovery(Client(), Manager(), lambda: True, "acct")
    recovery.REQUEST_TIMEOUT = 0.01
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(recovery.check(), 1)


@pytest.mark.asyncio
@pytest.mark.parametrize("failing", [False, True])
async def test_chat_wakes_do_not_bypass_check_interval_or_flood_wait(monkeypatch, failing):
    from telethon.errors import FloodWaitError
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    first, repeated = asyncio.Event(), asyncio.Event()
    attempts = 0
    original = Client.__call__

    async def observed(self, request):
        nonlocal attempts
        attempts += 1
        (first if attempts == 1 else repeated).set()
        if failing:
            raise FloodWaitError(request, capture=120)
        return await original(self, request)

    monkeypatch.setattr(Client, "__call__", observed)
    client, manager = Client(), Manager()
    client.remote.seq = 30
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    recovery.start()
    try:
        await asyncio.wait_for(first.wait(), 1)
        recovery.refresh()
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(repeated.wait(), 0.02)
        assert attempts == 1
    finally:
        await recovery.stop()


@pytest.mark.asyncio
async def test_outbound_pending_chat_is_detected_without_an_event(monkeypatch):
    from telegram_mcp.secret_updates import SecretUpdateRecovery

    client, manager = Client(), Manager()
    manager.chats.clear()
    called = asyncio.Event()
    original = Client.__call__

    async def observed(self, request):
        result = await original(self, request)
        called.set()
        return result

    monkeypatch.setattr(Client, "__call__", observed)
    recovery = SecretUpdateRecovery(client, manager, lambda: True, "acct")
    recovery.INTERVAL = 0.01
    recovery.start()
    try:
        await asyncio.sleep(0)
        assert not called.is_set()
        manager.chats.append(SimpleNamespace(state=SimpleNamespace(value="requested")))
        await asyncio.wait_for(called.wait(), 1)
    finally:
        await recovery.stop()
    assert len(client.requests) == 1
