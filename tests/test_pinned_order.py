"""`reorder_pinned_chats`: the order of pinned chats in All chats or in the Archive (plan 016)."""

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import pinned_order as mod


class Recorder:
    def __init__(self):
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        return True


async def _peer(chat_id, cl=None, account=None):
    return types.InputPeerUser(user_id=abs(int(chat_id)), access_hash=1)


@pytest.mark.parametrize("folder_id,force", [(0, False), (1, True)])
@pytest.mark.asyncio
async def test_each_entry_becomes_a_dialog_peer_in_the_given_order(wire_client, folder_id, force):
    client = Recorder()
    wire_client(mod, client, resolve=_peer)

    result = await mod.reorder_pinned_chats([30, 10, 20], folder_id=folder_id, force=force)

    request = client.sent[0]
    assert isinstance(request, functions.messages.ReorderPinnedDialogsRequest)
    assert request.folder_id == folder_id
    assert bool(request.force) is force
    assert all(isinstance(p, types.InputDialogPeer) for p in request.order)
    assert [p.peer.user_id for p in request.order] == [30, 10, 20]
    assert '"position": 1, "chat": "30"' in result


@pytest.mark.asyncio
async def test_an_empty_order_is_refused_before_any_request(wire_client):
    client = Recorder()
    wire_client(mod, client, resolve=_peer)

    result = await mod.reorder_pinned_chats([])

    assert client.sent == []
    assert "at least one" in result


@pytest.mark.asyncio
async def test_only_all_chats_and_the_archive_have_a_pinned_list_here(wire_client):
    client = Recorder()
    wire_client(mod, client, resolve=_peer)

    result = await mod.reorder_pinned_chats([1], folder_id=5)

    assert client.sent == []
    assert "folder_id" in result
