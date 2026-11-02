"""Typing is bounded temporary presence, cleared even on cancellation."""

import asyncio
import importlib

import pytest
from telethon import functions, types


@pytest.mark.asyncio
async def test_typing_cancellation_clears_and_releases_overlap(wire_client):
    mod = importlib.import_module("telegram_mcp.tools.typing_indicator")
    started = asyncio.Event()

    class Client:
        def __init__(self):
            self.actions = []

        async def __call__(self, request, **kwargs):
            assert isinstance(request, functions.messages.SetTypingRequest)
            self.actions.append(type(request.action))
            started.set()
            return True

    client = wire_client(mod, Client(), entity=types.InputPeerChat(7))
    task = asyncio.create_task(mod.send_typing(7, seconds=2, account="default"))
    await asyncio.wait_for(started.wait(), timeout=2)
    overlap = await mod.send_typing(7, seconds=1, account="default")
    assert "active" in overlap.lower()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, timeout=2)
    assert client.actions == [types.SendMessageTypingAction, types.SendMessageCancelAction]
    before = len(client.actions)
    refused = await mod.send_typing(7, seconds=float("nan"), account="default")
    assert "seconds" in refused
    assert len(client.actions) == before
