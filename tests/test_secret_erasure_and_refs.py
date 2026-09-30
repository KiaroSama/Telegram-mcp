"""Deletion covers durable file keys; malformed references never overwrite keys."""

import asyncio
import json

import pytest

from telegram_mcp import secret_backend, secret_history, secret_media_refs as refs
from telegram_mcp.tools import secret_actions
from secret_fakes import CHAT_ID, SECRET_ID


@pytest.fixture(autouse=True)
def history_cache(monkeypatch, tmp_path):
    monkeypatch.setattr(secret_history, "_cache", {})
    monkeypatch.setattr(secret_history, "state_dir", lambda: tmp_path)
    monkeypatch.setattr(refs, "state_dir", lambda: tmp_path)


@pytest.mark.parametrize("payload", [b"{bad", b"[]", b'{"12":[]}', b'{"12":{"1":null}}'])
def test_invalid_reference_file_is_not_replaced(payload):
    path = refs._path("acct")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    with pytest.raises(ValueError):
        refs.remember("acct", 12, 2, {"test": "synthetic"}, 0)
    assert path.read_bytes() == payload


def test_reference_retention_follows_arrival_order_not_random_id(monkeypatch):
    monkeypatch.setattr(refs, "_PER_CHAT_LIMIT", 2)
    for message_id in (30, 10, 20):
        refs.remember("acct", 12, message_id, {"test": message_id}, 0)
    assert refs.load("acct", 12, 30) is None
    assert refs.load("acct", 12, 10) is not None
    assert refs.load("acct", 12, 20) is not None


@pytest.mark.asyncio
@pytest.mark.parametrize("whole", [False, True])
async def test_local_deletion_removes_retrievable_file_reference(backend, whole):
    refs.remember("acct", SECRET_ID, 1, {"synthetic": 1}, 0)
    refs.remember("acct", SECRET_ID, 2, {"synthetic": 2}, 0)
    if whole:
        await secret_actions.clear_secret_history(CHAT_ID, CHAT_ID, account="acct")
    else:
        await secret_actions.delete_secret_message(CHAT_ID, 1, account="acct")
    assert refs.load("acct", SECRET_ID, 1) is None
    assert (refs.load("acct", SECRET_ID, 2) is None) is whole


@pytest.mark.asyncio
@pytest.mark.parametrize("whole", [False, True])
async def test_local_history_failure_does_not_prevent_key_erasure_or_hide_remote_ack(
    backend, monkeypatch, whole
):
    refs.remember("acct", SECRET_ID, 1, {"synthetic": 1}, 0)

    def failed(*args):
        raise OSError("synthetic history failure")

    monkeypatch.setattr(secret_history, "clear" if whole else "forget", failed)
    if whole:
        answer = await secret_actions.clear_secret_history(CHAT_ID, CHAT_ID, account="acct")
    else:
        answer = await secret_actions.delete_secret_message(CHAT_ID, 1, account="acct")
    assert refs.load("acct", SECRET_ID, 1) is None
    result = json.loads(answer)["results"]
    assert result["remote_request_accepted"] is True
    assert result["local_cleanup"]["history"] is False
    assert result["local_cleanup"]["media"] is True


@pytest.mark.asyncio
@pytest.mark.parametrize("whole", [False, True])
async def test_real_service_event_erases_application_copies(monkeypatch, tmp_path, whole):
    from telethon_secret_chat import MemoryStorage
    from telethon_secret_chat.events import ServiceActionReceived
    from test_secret_backend import _Client

    client = _Client(user_id=111)
    for name in (
        "_by_account",
        "_verified_against",
        "_store_locks",
        "_starts",
        "_stops",
        "_acquisitions",
    ):
        monkeypatch.setattr(secret_backend, name, {}, raising=False)
    monkeypatch.setattr(secret_backend, "_closing", False)
    monkeypatch.setattr(secret_backend, "_lock", asyncio.Lock())
    monkeypatch.setattr(secret_backend, "_telethon_client", lambda account: client)
    monkeypatch.setattr(secret_backend, "_storage_for", lambda account: MemoryStorage())
    monkeypatch.setattr(secret_backend, "_store_lock_dir", lambda: tmp_path)
    monkeypatch.setattr(secret_backend, "_owner_path", lambda account: tmp_path / "owner.json")
    manager = await secret_backend.secret_manager("acct")
    for account, chat in (("acct", 12), ("other", 12), ("acct", 13)):
        for message_id in (1, 2):
            secret_history.record(account, chat, {"message_id": message_id})
            refs.remember(account, chat, message_id, {"synthetic": message_id}, 0)
    tl = secret_backend.secret_tl
    action = (
        tl.DecryptedMessageActionFlushHistory()
        if whole
        else tl.DecryptedMessageActionDeleteMessages([1])
    )
    event = ServiceActionReceived(12, type(action).__name__, action, True)
    try:
        manager._emit(event)
        if manager._handler_tasks:
            await asyncio.gather(*manager._handler_tasks)
        left = secret_history.read("acct", 12, 10)
        assert [m["message_id"] for m in left] == ([] if whole else [2])
        assert refs.load("acct", 12, 1) is None
        assert refs.load("other", 12, 1) is not None
        assert refs.load("acct", 13, 1) is not None
        manager._emit(event)
        if manager._handler_tasks:
            await asyncio.gather(*manager._handler_tasks)
        assert secret_history.read("acct", 12, 10) == left
    finally:
        await secret_backend.close_all()


def test_reference_permission_refusal_never_publishes_data(monkeypatch):
    monkeypatch.setattr(refs, "restrict_to_owner_strict", lambda path: False)
    with pytest.raises(OSError, match="owner-only"):
        refs.remember("acct", 12, 1, {"synthetic": 1}, 0)
    assert not refs._path("acct").exists()
    assert not list(refs._path("acct").parent.glob("*.tmp"))
