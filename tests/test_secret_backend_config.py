"""Empty-only backend policy and quota validation."""

import pytest


def test_policy_requires_explicit_budgets_and_refuses_occupied_store():
    from telethon_secret_chat import MemoryStorage
    from telegram_mcp import secret_backend_config as policy

    store = MemoryStorage()
    assert policy.load(store) == {"version": 1, "mode": "normal", "limits": None}
    with pytest.raises(ValueError):
        policy.make("bounded")
    selected = policy.make("bounded", records=512, bytes=8388608, age_seconds=3600)
    policy.save_empty(store, selected)
    assert policy.load(store) == selected
    store.save({"id": 123})
    with pytest.raises(ValueError, match="empty"):
        policy.save_empty(store, policy.make("normal"))
    assert policy.load(store) == selected


@pytest.mark.parametrize(
    "budgets",
    [
        {"records": True},
        {"bytes": 67108865},
        {"age_seconds": float("nan")},
        {"tasks": 65},
        {"operation_seconds": 61},
    ],
)
def test_invalid_budgets_are_rejected(budgets):
    from telegram_mcp import secret_backend_config as policy

    values = {"records": 512, "bytes": 8388608, "age_seconds": 3600}
    values.update(budgets)
    with pytest.raises(ValueError):
        policy.make("bounded", **values)


def test_orphaned_queue_refuses_empty_mode_transition(tmp_path):
    from telethon_secret_chat import FileStorage
    from telegram_mcp import secret_backend_config as policy

    storage = FileStorage(tmp_path / "store.json")
    storage.queue_out(123, {"seq_no": 0})
    with pytest.raises(ValueError, match="empty"):
        policy.save_empty(storage, policy.make("normal"))
    assert FileStorage(tmp_path / "store.json").retained_out(123) == [{"seq_no": 0}]


@pytest.mark.asyncio
async def test_bounded_manager_selected_on_same_client_and_store(tmp_path, monkeypatch):
    from telethon_secret_chat import FileStorage, BoundedSecretChatManager
    from telegram_mcp import (
        secret_backend as backend,
        secret_backend_config as policy,
        secret_archive,
    )
    from test_secret_backend import _Client, _wire

    client = _Client()
    _wire(monkeypatch, backend, client, tmp_path)
    monkeypatch.setattr(backend, "_by_account", {})
    monkeypatch.setattr(backend, "_verified_against", {})
    monkeypatch.setattr(backend, "_starts", {})
    monkeypatch.setattr(backend, "_storages", {})
    storage = FileStorage(tmp_path / "store.json")
    secret_archive.disable(storage)
    policy.save_empty(
        storage, policy.make("bounded", records=512, bytes=8388608, age_seconds=3600)
    )
    monkeypatch.setattr(backend, "_storage_for", lambda account: storage)
    try:
        manager = await backend.secret_manager("acct")
        assert isinstance(manager, BoundedSecretChatManager)
        assert len(client.handlers) == 1
        assert backend._storages[id(manager)] is storage
        assert manager.auto_save_secret_chats is None
    finally:
        await backend.close_all()


@pytest.mark.asyncio
async def test_empty_running_manager_switches_without_duplicate_subscription(
    tmp_path, monkeypatch
):
    from telethon_secret_chat import FileStorage, BoundedSecretChatManager
    from telegram_mcp import secret_backend as backend, secret_control, secret_archive
    from test_secret_backend import _Client, _wire

    client = _Client()
    _wire(monkeypatch, backend, client, tmp_path)
    for name in ("_by_account", "_verified_against", "_starts", "_storages"):
        monkeypatch.setattr(backend, name, {})
    storepath = tmp_path / "store.json"
    secret_archive.disable(FileStorage(storepath))
    monkeypatch.setattr(backend, "_storage_for", lambda account: FileStorage(storepath))
    try:
        normal = await backend.secret_manager("acct")
        result = await secret_control.configure_backend(
            "acct", "bounded", records=512, bytes=8388608, age_seconds=3600
        )
        bounded = await backend.secret_manager("acct")
        assert result["mode"] == "bounded" and isinstance(bounded, BoundedSecretChatManager)
        assert bounded is not normal and len(client.handlers) == 1
        assert (await secret_control.backend_status("acct"))["mode"] == "bounded"
    finally:
        await backend.close_all()


@pytest.mark.asyncio
async def test_failed_manager_stop_retains_lease_and_refuses_mode_switch(tmp_path, monkeypatch):
    from telethon_secret_chat import FileStorage
    from telegram_mcp import (
        secret_backend as backend,
        secret_control,
        secret_archive,
        secret_backend_config,
    )
    from test_secret_backend import _Client, _wire

    client = _Client()
    _wire(monkeypatch, backend, client, tmp_path)
    for name in ("_by_account", "_verified_against", "_starts", "_storages"):
        monkeypatch.setattr(backend, name, {})
    storepath = tmp_path / "store.json"
    secret_archive.disable(FileStorage(storepath))
    monkeypatch.setattr(backend, "_storage_for", lambda account: FileStorage(storepath))
    normal = await backend.secret_manager("acct")
    stop = normal.stop

    async def fail():
        raise OSError("synthetic failed close")

    monkeypatch.setattr(normal, "stop", fail)
    try:
        with pytest.raises(OSError, match="failed close"):
            await secret_control.configure_backend(
                "acct", "bounded", records=512, bytes=8388608, age_seconds=3600
            )
        assert backend._by_account["acct"] is normal
        assert "acct" in backend._store_locks
        assert len(client.handlers) == 1
        assert secret_backend_config.load(FileStorage(storepath))["mode"] == "normal"
    finally:
        monkeypatch.setattr(normal, "stop", stop)
        await backend.close_all()
