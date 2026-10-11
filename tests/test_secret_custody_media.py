"""Actual pinned capped RAM decoding, private durable references and failure cleanup."""

import asyncio
import base64
import json

import pytest
from telethon.tl import types
from telethon_secret_chat.files import encrypt_file, file_fingerprint

from telegram_mcp import secret_backend as backend, secret_custody as custody
from test_secret_custody import CHAT, WIRE_SEQ, OfflineClient, build_manager, seed_store

CONTENT = b"bounded media fixture\x00\xff" * 3


def media_fixture(tmp_path):
    key, iv = bytes(range(32)), bytes(range(32, 64))
    media = backend.secret_tl.DecryptedMessageMediaDocument(
        thumb=b"",
        thumb_w=0,
        thumb_h=0,
        mime_type="application/octet-stream",
        size=len(CONTENT),
        key=key,
        iv=iv,
        attributes=[],
        caption="",
    )
    attached = types.EncryptedFile(
        id=7011,
        access_hash=18,
        size=len(CONTENT),
        dc_id=2,
        key_fingerprint=file_fingerprint(key, iv),
    )
    client = OfflineClient(encrypt_file(CONTENT, key, iv))
    return (
        build_manager(
            seed_store(tmp_path / "protocol.json", media=media, attachment=attached), client
        ),
        client,
    )


@pytest.mark.asyncio
async def test_public_media_exact_cap_no_files_no_ack_and_reference_survives_ack(
    tmp_path, monkeypatch
):
    from telegram_mcp.tools import secret_custody as tools
    from telegram_mcp import connection

    monkeypatch.setattr(custody, "state_dir", lambda: tmp_path)
    manager, client = media_fixture(tmp_path)
    await manager.start()
    monkeypatch.setattr(connection, "refresh_accounts", lambda: [])
    monkeypatch.setattr(connection, "clients", {"acct": client})

    async def get_manager(account):
        return manager

    monkeypatch.setattr(tools.backend, "secret_manager", get_manager)
    monkeypatch.setattr(backend, "media_source", lambda *args: (None, 0))
    try:
        before = set(tmp_path.rglob("*"))
        reply = json.loads(
            await tools.receive_secret_media(CHAT, 43, len(CONTENT), account="acct")
        )["results"]
        assert base64.b64decode(reply["content"], validate=True) == CONTENT
        assert reply["bytes"] == len(CONTENT)
        assert reply["content_file_created"] is False and reply["acknowledged"] is False
        assert reply["archive_on"] is False and reply["independent_archive_may_write"] is False
        assert set(tmp_path.rglob("*")) == before
        assert len(manager.pending_deliveries(CHAT)) == 1
        assert all(buffer.closed for buffer in client.buffers)
        assert not {"key", "iv", "media_reference", "access_hash"}.intersection(reply)
        assert custody.acknowledge(manager, "acct", CHAT, WIRE_SEQ)["acknowledged"] is True
        reference = custody.media_source(manager, "acct", CHAT, 43)
        assert isinstance(reference, backend.MediaReference)
        assert await manager.receive_file(reference, max_bytes=len(CONTENT)) == CONTENT
        await manager.stop()
        from telethon_secret_chat import FileStorage

        restarted = build_manager(FileStorage(tmp_path / "protocol.json"), client)
        await restarted.start()
        try:
            stored, ttl = custody.receipt_media_source("acct", CHAT, 43)
            assert ttl == 7 and isinstance(stored, backend.MediaReference)
            assert await restarted.receive_file(stored, max_bytes=len(CONTENT)) == CONTENT
            assert restarted.pending_deliveries(CHAT) == []
        finally:
            await restarted.stop()
        assert client.calls == []
    finally:
        await manager.stop()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["cap", "oversize", "short", "cancel", "error", "paused"])
async def test_media_refusal_failure_and_cancel_never_create_content_or_ack(tmp_path, failure):
    manager, client = media_fixture(tmp_path)
    await manager.start()
    try:
        message = manager.pending_deliveries(CHAT)[0].message
        before = set(tmp_path.rglob("*"))
        if failure == "paused":
            manager.pause_chat(CHAT)
        elif failure != "cap":

            async def download(location, destination, **kwargs):
                client.buffers.append(destination)
                if failure == "cancel":
                    raise asyncio.CancelledError
                if failure == "error":
                    raise OSError("synthetic download failure")
                destination.write(b"x" * (1024 if failure == "oversize" else 1))

            client.download_file = download
        expected = asyncio.CancelledError if failure == "cancel" else backend.ChatPaused
        with pytest.raises(expected):
            await manager.receive_file(message, max_bytes=len(CONTENT) - (failure == "cap"))
        assert set(tmp_path.rglob("*")) == before
        assert len(manager.pending_deliveries(CHAT)) == 1
        assert all(buffer.closed for buffer in client.buffers)
        if failure in ("cap", "paused"):
            assert client.buffers == []
        assert client.calls == []
    finally:
        await manager.stop()
