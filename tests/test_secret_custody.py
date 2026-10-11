"""Consumer commits and wire identity at the actual pinned package boundary."""

import time

import pytest
from telethon_secret_chat import FileStorage
from telethon_secret_chat.chat import ChatState, SecretChat

from telegram_mcp import secret_backend as backend

pytestmark = pytest.mark.timeout(10)
CHAT = 701
WIRE_SEQ = 9  # Not the package's internal counter (4).


class OfflineClient:
    """No Telegram connection: only public subscriptions and RAM downloads."""

    def __init__(self, content=b""):
        self.content = content
        self.calls = []
        self.handlers = []
        self.buffers = []

    def add_event_handler(self, handler, *args):
        self.handlers.append(handler)

    def remove_event_handler(self, handler, *args):
        self.handlers.remove(handler)

    async def __call__(self, request):
        self.calls.append(request)
        raise AssertionError("This local consumer operation must not send an RPC")

    async def download_file(self, location, destination, **kwargs):
        self.buffers.append(destination)
        destination.write(self.content)


class FaultStore(FileStorage):
    fail = False

    def _write(self):
        if self.fail:
            raise OSError("Synthetic protocol commit failure")
        return super()._write()


def seed_store(path, *, media=None, attachment=None, paused=False):
    """Small valid durable chat/relay, using actual TL serialization and storage."""
    chat = SecretChat(id=CHAT, access_hash=17, peer_user_id=5001, is_outbound=False)
    chat.adopt_key(bytes(range(256)))
    chat.state = ChatState.READY
    chat.layer = 144
    chat.in_seq_no = 5
    message = backend.secret_tl.DecryptedMessage(
        random_id=43, ttl=7, message="kept\x00 text", media=media
    )
    wrapper = backend.secret_tl.DecryptedMessageLayer(
        random_bytes=b"synthetic",
        layer=144,
        in_seq_no=0,
        out_seq_no=WIRE_SEQ,
        message=message,
    )
    relay = {
        "seq_no": 4,
        "body": bytes(wrapper).hex(),
        "file": bytes(attachment).hex() if attachment else None,
    }
    now = time.time()
    store = FaultStore(path)
    with store.transaction():
        store.save(chat.to_record())
        store.save_setting("auto_save_secret_chats", {"on": False, "folder": None})
        store.save_setting(
            "bounded_secret_chat_custody_v1",
            {
                str(CHAT): {
                    "pause": "operator" if paused else None,
                    "lost": False,
                    "since": now,
                    "relay": [relay],
                    "ages": {"in:4": now},
                }
            },
        )
    return store


def build_manager(store, client=None):
    return backend.BoundedSecretChatManager(
        client or OfflineClient(),
        store,
        limits=backend.CustodyLimits(records=32, bytes=1_048_576, age_seconds=3600),
    )


@pytest.mark.asyncio
async def test_durable_snapshot_precedes_ack_and_wire_identity_reconciles_restart(
    tmp_path, monkeypatch
):
    from telegram_mcp import secret_custody as custody

    monkeypatch.setattr(custody, "state_dir", lambda: tmp_path)
    store = seed_store(tmp_path / "protocol.json")
    client = OfflineClient()
    manager = build_manager(store, client)
    await manager.start()
    try:
        pending = custody.pending_page(manager, CHAT, limit=1, offset=0)
        assert pending["deliveries"][0]["seq_no"] == WIRE_SEQ
        assert pending["deliveries"][0]["message"]["text"] == "kept text"
        assert custody.pending_page(manager, CHAT, limit=1, offset=0) == pending
        assert len(manager.pending_deliveries(CHAT)) == 1

        original_ack = manager.acknowledge_delivery

        def observed_ack(chat_id, seq_no):
            receipt = custody.read_receipt("acct", chat_id, seq_no)
            assert receipt["message"]["message_id"] == 43
            assert receipt["seq_no"] == WIRE_SEQ
            original_ack(chat_id, seq_no)

        monkeypatch.setattr(manager, "acknowledge_delivery", observed_ack)
        result = custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)
        assert result["acknowledged"] is True and result["consumer_committed"] is True
        assert manager.pending_deliveries(CHAT) == []
        assert custody.read_receipt("acct", CHAT, WIRE_SEQ)["message"]["text"] == "kept text"
    finally:
        await manager.stop()
    restarted = build_manager(FileStorage(tmp_path / "protocol.json"), client)
    await restarted.start()
    try:
        result = custody.acknowledge(restarted, "acct", CHAT, WIRE_SEQ)
        assert result["acknowledged"] is True
        assert result["reconciled"] is True
        assert client.calls == []
        assert client.handlers == [restarted._subscription]
    finally:
        await restarted.stop()


@pytest.mark.asyncio
@pytest.mark.parametrize("fault", ["consumer", "acl", "corrupt", "protocol"])
async def test_failed_commit_never_loses_delivery_and_retains_reconciliation(
    tmp_path, monkeypatch, fault
):
    from telegram_mcp import secret_custody as custody

    monkeypatch.setattr(custody, "state_dir", lambda: tmp_path)
    store = seed_store(tmp_path / "protocol.json")
    manager = build_manager(store)
    await manager.start()
    try:
        before = manager.pending_deliveries(CHAT)
        if fault == "consumer":

            def failed_replace(*args):
                raise OSError("Synthetic consumer replace failure")

            with monkeypatch.context() as patch:
                patch.setattr(custody.os, "replace", failed_replace)
                with pytest.raises(OSError):
                    custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)
            assert custody.read_receipt("acct", CHAT, WIRE_SEQ) is None
        elif fault == "acl":
            with monkeypatch.context() as patch:
                patch.setattr(custody, "restrict_to_owner_strict", lambda path: False)
                with pytest.raises(PermissionError):
                    custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)
            assert custody.read_receipt("acct", CHAT, WIRE_SEQ) is None
        elif fault == "corrupt":
            path = tmp_path / "secret-chats" / "acct-consumer.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("not a receipt", encoding="utf-8")
            with pytest.raises(ValueError):
                custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)
            assert path.read_text(encoding="utf-8") == "not a receipt"
            assert manager.pending_deliveries(CHAT) == before
            return
        else:
            store.fail = True
            result = custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)
            assert result["acknowledged"] is None
            assert result["consumer_committed"] is True
            assert custody.read_receipt("acct", CHAT, WIRE_SEQ)["message"]["message_id"] == 43
            store.fail = False
        assert manager.pending_deliveries(CHAT) == before
        assert list(tmp_path.rglob("*.consumer.tmp")) == []
        assert custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)["acknowledged"] is True
        assert manager.pending_deliveries(CHAT) == []
        assert manager._client.calls == []
    finally:
        store.fail = False
        await manager.stop()


@pytest.mark.asyncio
async def test_interrupted_ack_retains_receipt_and_same_wire_identity_after_restart(
    tmp_path, monkeypatch
):
    import asyncio
    from telegram_mcp import secret_custody as custody

    monkeypatch.setattr(custody, "state_dir", lambda: tmp_path)
    store = seed_store(tmp_path / "protocol.json")
    manager = build_manager(store)
    await manager.start()
    try:

        def interrupted(*args):
            raise asyncio.CancelledError

        monkeypatch.setattr(manager, "acknowledge_delivery", interrupted)
        with pytest.raises(asyncio.CancelledError):
            custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)
        assert custody.read_receipt("acct", CHAT, WIRE_SEQ)["message"]["message_id"] == 43
        committed_at = custody.read_receipt("acct", CHAT, WIRE_SEQ)["committed_at"]
        assert manager.pending_deliveries(CHAT)[0].seq_no == WIRE_SEQ
    finally:
        await manager.stop()
    restarted = build_manager(FileStorage(tmp_path / "protocol.json"))
    await restarted.start()
    try:
        monkeypatch.setattr(custody, "_commit_time", lambda: committed_at + 100)
        result = custody.acknowledge(restarted, "acct", CHAT, WIRE_SEQ)
        assert custody.read_receipt("acct", CHAT, WIRE_SEQ)["committed_at"] == committed_at
        assert result["acknowledged"] is True and result["reconciled"] is True
        assert restarted.pending_deliveries(CHAT) == []
        with pytest.raises(ValueError):
            custody.acknowledge(restarted, "acct", CHAT, WIRE_SEQ + 2)
    finally:
        await restarted.stop()


@pytest.mark.asyncio
async def test_receipt_public_history_survives_ack_restart_without_callbacks(
    tmp_path, monkeypatch
):
    from telegram_mcp import secret_custody as custody

    monkeypatch.setattr(custody, "state_dir", lambda: tmp_path)
    path = tmp_path / "protocol.json"
    manager = build_manager(seed_store(path))
    await manager.start()
    try:
        monkeypatch.setattr(custody, "_commit_time", lambda: 100.0)
        assert custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)["acknowledged"] is True
    finally:
        await manager.stop()
    restarted = build_manager(FileStorage(path))
    await restarted.start()
    try:
        records = custody.read_history("acct", CHAT, 30)
        assert len(records) == 1 and records[0]["message_id"] == 43
        assert records[0]["text"] == "kept text"
        assert records[0]["consumer_committed_at"] == 100.0
        assert records[0]["date_basis"] == "consumer_commit"
        assert "date" not in records[0]
        assert not {"media_reference", "key", "iv"}.intersection(records[0])
        records[0]["text"] = "changed detached projection"
        assert custody.read_history("acct", CHAT, 30)[0]["text"] == "kept text"
        assert restarted.pending_deliveries(CHAT) == []
        from telegram_mcp import connection, secret_history
        from telegram_mcp.tools import secret_messaging, secret_actions
        import json

        monkeypatch.setattr(secret_history, "state_dir", lambda: tmp_path)
        monkeypatch.setattr(secret_history, "_cache", {})
        monkeypatch.setattr(connection, "refresh_accounts", lambda: [])
        monkeypatch.setattr(connection, "clients", {"acct": OfflineClient()})

        async def manager_for(account):
            return restarted

        monkeypatch.setattr(secret_messaging, "secret_manager", manager_for)
        monkeypatch.setattr(secret_actions, "secret_manager", manager_for)
        outgoing = secret_history.entry(message_id=61, is_outgoing=True, text="outgoing")
        outgoing["date"] = 200
        secret_history.record("acct", CHAT, outgoing)
        public = json.loads(await secret_messaging.read_secret_messages(CHAT, account="acct"))[
            "results"
        ]["messages"]
        assert [item["message_id"] for item in public] == [43, 61]
        assert secret_history.read("acct", CHAT, 1)[0]["message_id"] == 61
        found = json.loads(
            await secret_actions.search_secret_messages(CHAT, "kept", account="acct")
        )["results"]["messages"]
        assert len(found) == 1 and found[0]["message_id"] == 43
        secret_history.record(
            "acct", CHAT, secret_history.entry(message_id=43, is_outgoing=False, text="kept text")
        )
        assert [item["message_id"] for item in secret_history.read("acct", CHAT, 30)] == [61, 43]
    finally:
        await restarted.stop()
