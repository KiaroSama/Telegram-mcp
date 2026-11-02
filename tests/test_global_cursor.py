"""Global search continues across chat-local IDs through the public tool."""

import json
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon import functions, types
from telegram_mcp.tools import message_search as mod


@pytest.mark.asyncio
async def test_native_cursor_passes_low_ids_and_keeps_filter(wire_client):
    peer = types.User(id=77, access_hash=88, first_name="Fixture")
    now = datetime.now(timezone.utc)

    class Client:
        _self_id = 999
        _mb_entity_cache = {}

        def __init__(self):
            self.requests = []

        async def __call__(self, request):
            assert isinstance(request, functions.messages.SearchGlobalRequest)
            assert isinstance(request.filter, types.InputMessagesFilterPhotos)
            self.requests.append(request)
            if request.offset_id == 0:
                ids = [3, 2]
            elif request.offset_id == 2:
                ids = [1]
            else:
                ids = []
            messages = [
                types.Message(id=i, peer_id=types.PeerUser(77), message="hit", date=now)
                for i in ids
            ]
            return SimpleNamespace(messages=messages, users=[peer], chats=[], next_rate=7)

    client = wire_client(mod, Client())
    first = json.loads(
        await mod.search_global("hit", media_type="photos", page_size=2, account="default")
    )
    assert [r["id"] for r in first["results"]] == [3, 2]
    assert first["exhausted"] is False
    second = json.loads(
        await mod.search_global(
            "hit", media_type="photos", page_size=2, cursor=first["next_cursor"], account="default"
        )
    )
    assert [r["id"] for r in second["results"]] == [1]
    assert client.requests[1].offset_peer.user_id == 77
    assert client.requests[1].offset_peer.access_hash == 88
    before = len(client.requests)
    refused = await mod.search_global(
        "other", media_type="photos", cursor=first["next_cursor"], account="default"
    )
    assert "cursor" in refused.lower()
    assert len(client.requests) == before


@pytest.mark.asyncio
async def test_empty_rate_progress_and_legacy_pages_keep_the_native_position(wire_client):
    peer = types.User(id=77, access_hash=88, first_name="Fixture")

    class Client:
        _self_id = 999
        _mb_entity_cache = {}

        def __init__(self):
            self.requests = []

        async def __call__(self, request):
            self.requests.append(request)
            if request.offset_rate == 0:
                return types.messages.MessagesSlice(
                    count=6, topics=[], messages=[], chats=[], users=[], next_rate=1
                )
            start = request.offset_id or 7
            ids = list(range(start - 1, max(0, start - 1 - request.limit), -1))
            return types.messages.MessagesSlice(
                count=6,
                topics=[],
                messages=[
                    types.Message(id=i, peer_id=types.PeerUser(77), message="hit") for i in ids
                ],
                chats=[],
                users=[peer],
                next_rate=2,
            )

    client = wire_client(mod, Client())
    result = json.loads(await mod.search_global("hit", page=2, page_size=2, account="default"))
    assert [row["id"] for row in result["results"]] == [4, 3]
    assert result["next_cursor"] and not result["exhausted"]
    assert [request.offset_rate for request in client.requests] == [0, 1, 2]
    assert all(request.limit <= 100 for request in client.requests)


@pytest.mark.asyncio
async def test_invalid_cursor_refuses_before_connection(wire_client, monkeypatch):
    class Client:
        async def __call__(self, request):
            raise AssertionError("Invalid cursor reached Telegram")

    wire_client(mod, Client())

    async def connect(*_):
        raise AssertionError("Invalid cursor connected")

    monkeypatch.setattr(mod, "ensure_connected", connect)
    for cursor in ("!", "x" * 2049, "W10=", "bnVsbA=="):
        answer = await mod.search_global("hit", cursor=cursor, account="default")
        assert "cursor is invalid" in answer
