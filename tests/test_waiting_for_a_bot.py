"""Waiting for a bot's reply, which is most of what an agent waits for.

The defect: a bot answered in 328 ms, the reply was in the chat, and
``wait_for_settled_message`` returned ``{"event": false, "reason": "timeout"}``.
The incoming handler dropped every message whose sender was a bot before it was
ever recorded, so no wait could ever settle on one — and a timeout is
indistinguishable from "the bot never answered", which is what made it cost a
manual ``get_history`` to find out.

What is pinned here is the split the fix rests on: naming a chat is asking for
THAT chat, bot or not, while an unfiltered wait keeps its documented promise of
human messages only — so a chatty bot still cannot wake an agent waiting for a
person.
"""

import asyncio
import json
import time
from types import SimpleNamespace

import pytest

from telegram_mcp.tools import events, events_store
from telegram_mcp.tools import feed_lifecycle as lifecycle

ACCOUNT = "default"
_CURRENT_CLIENT = object()


@pytest.fixture(autouse=True)
def _clean_state(monkeypatch, tmp_path):
    monkeypatch.setattr(events_store, "_pending_msgs", {})
    monkeypatch.setattr(events, "_activity_event", None)
    monkeypatch.setattr(lifecycle, "_task", None)
    monkeypatch.setattr(lifecycle, "_stopping", None)
    monkeypatch.setattr(lifecycle, "_autostart_done", False)
    monkeypatch.setattr(events_store, "_dropped", events_store._new_drop_ledger())
    monkeypatch.delenv("TELEGRAM_EVENT_FEED", raising=False)
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_FILE", str(tmp_path / "feed.jsonl"))


async def _deliver(chat_id, *, bot=False, is_self=False, message_id=7, name="Helper"):
    sender = SimpleNamespace(bot=bot, is_self=is_self, username="helper", first_name=name)

    async def get_sender():
        return sender

    event = SimpleNamespace(
        is_private=True,
        chat_id=chat_id,
        message=SimpleNamespace(id=message_id),
        get_sender=get_sender,
    )
    events.clients.setdefault(ACCOUNT, _CURRENT_CLIENT)
    await events._on_new_incoming(ACCOUNT, events.clients[ACCOUNT], event)


def _target(chat_id):
    async def target(chat, account=None):
        return chat_id

    return target


@pytest.mark.asyncio
async def test_a_bots_reply_is_recorded_at_all():
    """Nothing downstream can return a burst the handler never wrote."""
    await _deliver(42, bot=True)

    assert (ACCOUNT, 42) in events_store._pending_msgs
    assert events_store._pending_msgs[(ACCOUNT, 42)]["bot"] is True


@pytest.mark.asyncio
async def test_waiting_for_a_named_bot_chat_settles_instead_of_timing_out(monkeypatch):
    """The reported case, end to end: the answer arrives, the wait returns it."""
    monkeypatch.setattr(events, "_wait_target", _target(42))
    await _deliver(42, bot=True, name="Verifier Bot")

    answer = json.loads(
        await events.wait_for_settled_message(
            settle_ms=50, max_wait_ms=600, chat_id="@verifierbot"
        )
    )

    assert answer["event"] is True, "a bot's reply still reads as a timeout"
    assert answer["chat_id"] == 42
    assert answer["bot"] is True
    assert (ACCOUNT, 42) not in events_store._pending_msgs, "a settled burst must be consumed"


@pytest.mark.asyncio
async def test_an_unfiltered_settled_wait_still_ignores_bots(monkeypatch):
    """The documented promise: an unnamed wait is for people, not for bots."""
    monkeypatch.setattr(events, "_wait_target", _target(None))
    await _deliver(42, bot=True)

    answer = json.loads(await events.wait_for_settled_message(settle_ms=50, max_wait_ms=250))

    assert answer["event"] is False
    assert answer["reason"] == "timeout"
    assert (ACCOUNT, 42) in events_store._pending_msgs, "the burst is skipped, not eaten"


@pytest.mark.asyncio
async def test_an_unfiltered_new_message_wait_still_ignores_bots(monkeypatch):
    monkeypatch.setattr(events, "_wait_target", _target(None))
    await _deliver(42, bot=True)

    answer = json.loads(await events.wait_for_new_message(timeout=0.25))

    assert answer["event"] is False
    assert answer["reason"] == "timeout"


@pytest.mark.asyncio
async def test_a_named_bot_chat_is_listed_by_wait_for_new_message(monkeypatch):
    monkeypatch.setattr(events, "_wait_target", _target(42))
    await _deliver(42, bot=True)

    answer = json.loads(await events.wait_for_new_message(timeout=0.25, chat_id="@verifierbot"))

    assert answer["event"] is True
    assert [chat["chat_id"] for chat in answer["pending_chats"]] == [42]


@pytest.mark.asyncio
async def test_a_bot_burst_does_not_hold_an_unfiltered_wait_awake(monkeypatch):
    """A skipped burst must not set the sleep-until-it-settles deadline either.

    `_scan_settled` returns the time to the soonest pending chat; counting a
    burst that will never be handed over turns a quiet wait into a spin.
    """
    monkeypatch.setattr(events, "_wait_target", _target(None))
    await _deliver(42, bot=True)

    settled, soonest = events._scan_settled(time.monotonic(), settle=6.0)

    assert settled is None
    assert soonest is None


@pytest.mark.asyncio
async def test_the_feed_leaves_a_bot_burst_where_it_found_it(monkeypatch, tmp_path):
    """The feed is the unfiltered consumer, so its content does not change."""
    await _deliver(42, bot=True)
    await _deliver(43, bot=False, name="Dana")

    task = asyncio.get_running_loop().create_task(events._feed_loop(settle_ms=50))
    try:
        for _ in range(200):
            await asyncio.sleep(0.01)
            if (ACCOUNT, 43) not in events_store._pending_msgs:
                break
    finally:
        task.cancel()

    written = [
        json.loads(line)
        for line in events_store.feed_file_path().read_text("utf-8").split("\n")
        if line
    ]
    assert [line["chat_id"] for line in written] == [43]
    assert (ACCOUNT, 42) in events_store._pending_msgs


@pytest.mark.asyncio
async def test_the_accounts_own_outgoing_message_is_still_not_an_event():
    """`is_self` is a different question from `bot` and keeps its early return."""
    await _deliver(42, is_self=True)

    assert events_store._pending_msgs == {}
