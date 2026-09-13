"""The ceilings on the un-collected burst map, and what a dropped burst says.

Split from `test_event_feed.py`, which had grown past the size ceiling. This is
a real seam rather than a slice: the feed FILE and the pending MAP are two
different stores with two different failure modes. The file grows on disk and is
bounded by rotation; the map grows in memory, is bounded by a count and a TTL,
and every eviction is a message the agent will never answer - which is why the
drop ledger exists and why its own size is bounded too.

`test_event_feed.py` keeps the consumer, the feed file and the waits.
"""

import time
from types import SimpleNamespace

import pytest

from telegram_mcp.tools import events, events_store

# Every pre-seeded burst belongs to a login; these tests only need one.
ACCOUNT = "default"

# The handler is bound to the client it was registered for, so it can tell
# whether the generation it belongs to is still the current one. Delivering
# means naming that client - which here is simply the account's own.
_CURRENT_CLIENT = object()


async def _deliver(event):
    events.clients.setdefault(ACCOUNT, _CURRENT_CLIENT)
    return await events._on_new_incoming(ACCOUNT, events.clients[ACCOUNT], event)


class _IncomingEvent:
    """The few attributes `_on_new_incoming` reads off a Telethon event."""

    def __init__(self, chat_id, message_id, name="Dana"):
        self.is_private = True
        self.chat_id = chat_id
        self.message = SimpleNamespace(id=message_id)
        self._sender = SimpleNamespace(bot=False, is_self=False, username=None, first_name=name)

    async def get_sender(self):
        return self._sender


def _pending_record(last_ts, count=2, name="Client"):
    return {
        "first_ts": last_ts - 1.0,
        "last_ts": last_ts,
        "count": count,
        "first_id": 10,
        "last_id": 11,
        "name": name,
        "username": "client",
        "account": ACCOUNT,
    }


def _mono(seconds_ago=0.0):
    return time.monotonic() - seconds_ago


@pytest.fixture(autouse=True)
def _clean_state(monkeypatch, tmp_path):
    monkeypatch.setattr(events_store, "_pending_msgs", {})
    monkeypatch.setattr(events, "_activity_event", None)
    monkeypatch.setattr(events_store, "_dropped", events_store._new_drop_ledger())
    for name in (
        "TELEGRAM_EVENT_FEED",
        "TELEGRAM_EVENT_PENDING_MAX",
        "TELEGRAM_EVENT_PENDING_TTL_SECONDS",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_FILE", str(tmp_path / "feed.jsonl"))


def _incoming(chat_id, message_id=1, name="Dana"):
    return _IncomingEvent(chat_id, message_id, name=name)


@pytest.mark.asyncio
async def test_the_pending_map_stops_at_its_ceiling(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_PENDING_MAX", "5")

    for chat in range(9):
        await _deliver(_incoming(chat))

    assert len(events_store._pending_msgs) == 5, events_store._pending_msgs


@pytest.mark.asyncio
async def test_an_overflow_drop_is_reported_rather_than_silent(monkeypatch):
    """A dropped burst is a message the agent will never answer. Losing it may be
    unavoidable once the ceiling is reached; losing it quietly is not."""
    monkeypatch.setenv("TELEGRAM_EVENT_PENDING_MAX", "2")

    for chat in range(5):
        await _deliver(_incoming(chat))

    state = events_store.overflow_state()
    assert state["dropped_total"] == 3
    assert state["dropped_reason_counts"]["overflow"] == 3
    assert state["recent_dropped"], "no record of which chats were dropped"
    assert state["recent_dropped"][-1]["reason"] == "overflow"


@pytest.mark.asyncio
async def test_the_oldest_burst_is_the_one_dropped(monkeypatch):
    """The newest message is the one an agent still has a chance of answering."""
    monkeypatch.setenv("TELEGRAM_EVENT_PENDING_MAX", "2")

    for chat in (1, 2, 3):
        await _deliver(_incoming(chat))

    assert sorted(chat for _account, chat in events_store._pending_msgs) == [2, 3]


@pytest.mark.asyncio
async def test_a_burst_nobody_collected_expires(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_PENDING_TTL_SECONDS", "30")
    events_store._pending_msgs[(ACCOUNT, 1)] = _pending_record(_mono(3600))
    events_store._pending_msgs[(ACCOUNT, 2)] = _pending_record(_mono(1))

    events_store._expire_pending()

    assert list(events_store._pending_msgs) == [(ACCOUNT, 2)]
    assert events_store.overflow_state()["dropped_reason_counts"]["expired"] == 1


@pytest.mark.asyncio
async def test_an_expiry_does_not_take_a_burst_that_is_still_fresh(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_PENDING_TTL_SECONDS", "3600")
    events_store._pending_msgs[(ACCOUNT, 1)] = _pending_record(_mono(60))

    events_store._expire_pending()

    assert (ACCOUNT, 1) in events_store._pending_msgs


@pytest.mark.asyncio
async def test_the_drop_ledger_itself_is_bounded(monkeypatch):
    """A ledger of unbounded drops is the leak it was added to report."""
    monkeypatch.setenv("TELEGRAM_EVENT_PENDING_MAX", "1")

    for chat in range(200):
        await _deliver(_incoming(chat))

    state = events_store.overflow_state()
    assert state["dropped_total"] == 199
    assert len(state["recent_dropped"]) <= events_store._DROP_LEDGER_MAX
