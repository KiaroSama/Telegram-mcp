"""Deleting messages: saying what was actually deleted, and for whom.

`messages.deleteHistory` answers with `messages.affectedHistory`, whose `offset`
is a continuation signal: a positive value means the method has to be called
again with the same parameters until it reaches zero. One call and a "history
cleared" report is therefore a claim the server never made.

The loop that repeats it is bounded three ways -- an iteration ceiling, a wall
deadline, and a progress check -- because a server that keeps answering with the
same offset would otherwise spin forever.

No network: a fake client records the TL requests it was handed.
"""

import asyncio
import datetime
import time
from types import SimpleNamespace

import pytest

from telethon.tl.types import Channel

from telegram_mcp.tools import messages_delete as mod


class _Client:
    """Answers DeleteHistoryRequest with a scripted sequence of offsets."""

    def __init__(self, offsets=(0,), pts_counts=None, delay=0.0):
        self.requests = []
        self.offsets = list(offsets)
        self.pts_counts = list(pts_counts or [len(offsets) and 5] * len(self.offsets))
        self.delay = delay
        self.deleted = []

    async def __call__(self, request):
        self.requests.append(request)
        if self.delay:
            time.sleep(self.delay)
        index = min(len(self.requests) - 1, len(self.offsets) - 1)
        return SimpleNamespace(offset=self.offsets[index], pts_count=self.pts_counts[index])

    async def delete_messages(self, entity, message_ids, revoke=True):
        self.deleted.append((entity, message_ids, revoke))
        return [SimpleNamespace(pts_count=1)]


@pytest.fixture
def _wire(monkeypatch):
    def wire(client):
        monkeypatch.setattr(mod, "get_client", lambda account=None: client)

        async def _ensure(_client):
            return None

        async def _resolve(chat_id, _client):
            return SimpleNamespace(id=chat_id)

        monkeypatch.setattr(mod, "ensure_connected", _ensure)
        monkeypatch.setattr(mod, "resolve_entity", _resolve)
        return client

    return wire


@pytest.mark.asyncio
async def test_a_positive_offset_is_repeated_until_the_server_says_zero(_wire):
    """A single call with offset=50 left most of the history in place and still
    reported it cleared."""
    client = _wire(_Client(offsets=(50, 10, 0), pts_counts=(50, 10, 3)))

    result = await mod.delete_chat_history(1, account="a")

    assert len(client.requests) == 3, "the continuation offset was ignored"
    assert "63" in result, "the per-call counts were not aggregated"
    assert "cleared" in result


@pytest.mark.asyncio
async def test_an_offset_that_stops_shrinking_aborts_instead_of_spinning(_wire):
    """A server that answers with the same offset forever must not become an
    infinite loop inside a tool call."""
    client = _wire(_Client(offsets=(50, 50, 50, 50), pts_counts=(1, 1, 1, 1)))

    result = await mod.delete_chat_history(1, account="a")

    assert len(client.requests) == 2, "no progress check; it kept asking"
    assert "incomplete" in result.lower()
    assert "50" in result


@pytest.mark.asyncio
async def test_the_loop_gives_up_at_its_iteration_ceiling(_wire, monkeypatch):
    """Steady progress that never reaches zero is still bounded."""
    monkeypatch.setattr(mod, "_DELETE_HISTORY_MAX_PASSES", 4)
    client = _wire(_Client(offsets=(100, 90, 80, 70, 60), pts_counts=(1, 1, 1, 1, 1)))

    result = await mod.delete_chat_history(1, account="a")

    assert len(client.requests) == 4
    assert "incomplete" in result.lower()


@pytest.mark.asyncio
async def test_the_loop_gives_up_at_its_wall_deadline(_wire, monkeypatch):
    """Progress that is real but far too slow is bounded by time, not only by
    pass count."""
    # A sleep here models the RPC the deadline exists for; `sleep` guarantees a
    # lower bound, so one pass always overshoots a 10ms budget.
    monkeypatch.setattr(mod, "_DELETE_HISTORY_DEADLINE_SECONDS", 0.01)
    client = _wire(_Client(offsets=(100, 90, 80, 70, 0), pts_counts=(1, 1, 1, 1, 1), delay=0.02))

    result = await mod.delete_chat_history(1, account="a")

    assert len(client.requests) < 5
    assert "incomplete" in result.lower()


@pytest.mark.asyncio
async def test_deleting_one_message_says_whether_it_was_revoked(_wire):
    """`revoke` defaulted to Telethon's True with no parameter and no mention of
    it, so a single delete reached the other party silently. Both scopes now say
    which one they used."""
    client = _wire(_Client())

    both = await mod.delete_message(1, 5, revoke=True, account="a")
    assert client.deleted[-1][2] is True
    assert "both" in both

    mine = await mod.delete_message(1, 5, revoke=False, account="a")
    assert client.deleted[-1][2] is False
    assert "both" not in mine


# --- the call itself has to be bounded, not only the loop around it ---------


class _HangingClient:
    """Answers the first few deletes, then never returns from the next one."""

    def __init__(self, answers=()):
        self.requests = []
        self.answers = list(answers)
        self.cancelled = False

    async def __call__(self, request):
        self.requests.append(request)
        if self.answers:
            offset, pts = self.answers.pop(0)
            return SimpleNamespace(offset=offset, pts_count=pts)
        try:
            await asyncio.sleep(3600)
        except asyncio.CancelledError:
            self.cancelled = True
            raise

    async def delete_messages(self, entity, message_ids, revoke=True):
        return [SimpleNamespace(pts_count=1)]


@pytest.mark.asyncio
async def test_a_delete_that_never_returns_is_abandoned_at_the_deadline(_wire, monkeypatch):
    """The deadline was only consulted between calls, so one RPC that never came
    back outlived it entirely. That is not a budget the tool holds, it is a
    budget the server is free to opt out of."""
    monkeypatch.setattr(mod, "_DELETE_HISTORY_DEADLINE_SECONDS", 0.05)
    client = _wire(_HangingClient())

    started = time.monotonic()
    result = await asyncio.wait_for(mod.delete_chat_history(1, account="a"), timeout=5)
    elapsed = time.monotonic() - started

    assert elapsed < 2, f"the hung call was never abandoned ({elapsed:.2f}s)"
    assert "incomplete" in result.lower()
    assert client.cancelled, "the abandoned request was left running in the background"


@pytest.mark.asyncio
async def test_progress_made_before_a_hung_call_is_reported_honestly(_wire, monkeypatch):
    """Giving up must not throw away what the earlier passes actually deleted."""
    monkeypatch.setattr(mod, "_DELETE_HISTORY_DEADLINE_SECONDS", 0.2)
    _wire(_HangingClient(answers=[(40, 12)]))

    result = await asyncio.wait_for(mod.delete_chat_history(1, account="a"), timeout=5)

    assert "12" in result, "the messages that were deleted went unreported"
    assert "incomplete" in result.lower()


@pytest.mark.asyncio
async def test_an_exhausted_budget_never_starts_another_delete(_wire, monkeypatch):
    """Zero remaining time is not enough time for one more call."""
    monkeypatch.setattr(mod, "_DELETE_HISTORY_DEADLINE_SECONDS", 0.0)
    client = _wire(_Client(offsets=(50, 0), pts_counts=(1, 1)))

    result = await mod.delete_chat_history(1, account="a")

    assert client.requests == [], "a delete was sent with no budget left to bound it"
    assert "incomplete" in result.lower()


# --- deleting for everyone is a choice, not a default ----------------------


@pytest.mark.asyncio
async def test_deleting_one_message_does_not_reach_the_other_party_by_default(_wire):
    """`revoke=True` by default made the least alarming-sounding call the most
    destructive one available: an agent asked to tidy its own view took the
    message out of the recipient's chat too, irreversibly."""
    client = _wire(_Client())

    mine = await mod.delete_message(1, 5, account="a")

    assert client.deleted[-1][2] is False, "the default still deleted for everyone"
    assert "both" not in mine
    assert "you only" in mine


@pytest.mark.asyncio
async def test_deleting_for_everyone_is_available_when_it_is_asked_for(_wire):
    client = _wire(_Client())

    both = await mod.delete_message(1, 5, revoke=True, account="a")

    assert client.deleted[-1][2] is True
    assert "both" in both


@pytest.mark.asyncio
async def test_a_channel_deletion_is_not_reported_as_private(_wire, monkeypatch):
    """A channel keeps no per-account copy, so `revoke` is ignored and the post
    goes for every subscriber. Reporting the FLAG said "for you only" about a
    message that was already gone for everyone - the one reading of that
    sentence that matters, and the wrong one."""
    client = _wire(_Client())

    async def _resolve_channel(chat_id, _client):
        return Channel(
            id=chat_id,
            title="a channel",
            photo=None,
            date=datetime.datetime.now(datetime.timezone.utc),
            broadcast=True,
        )

    monkeypatch.setattr(mod, "resolve_entity", _resolve_channel)

    said = await mod.delete_message(-1001129051609, 973, revoke=True, account="a")

    assert client.deleted[-1][1] == 973, "the delete itself must still have been issued"
    assert "you only" not in said, "a channel post is never deleted for you alone"
    assert "both parties" not in said, "a channel has subscribers, not two parties"
    assert "for everyone" in said


# --- the ids are checked against this chat before anything is removed -------


class _PeerClient(_Client):
    """Answers `get_messages` with scripted per-id peers, and records the sends."""

    def __init__(self, peers, entity_peer=1):
        super().__init__()
        self.peers = peers  # id -> peer marker, or None for "not found"
        self.entity_peer = entity_peer
        self.sent_ids = []

    async def get_messages(self, entity, ids):
        return [
            None if self.peers.get(i) is None else SimpleNamespace(peer_id=self.peers[i])
            for i in ids
        ]

    async def __call__(self, request):
        self.requests.append(request)
        self.sent_ids.extend(getattr(request, "id", []))
        return SimpleNamespace(pts_count=99, offset=0)


@pytest.fixture
def _peers(monkeypatch):
    """`get_peer_id` is identity here so the test can use plain markers."""
    monkeypatch.setattr(mod.telethon_utils, "get_peer_id", lambda peer: peer)

    def wire(client, entity_peer=1):
        monkeypatch.setattr(mod, "get_client", lambda account=None: client)

        async def _ensure(_client):
            return None

        async def _resolve(chat_id, _client):
            return entity_peer

        monkeypatch.setattr(mod, "ensure_connected", _ensure)
        monkeypatch.setattr(mod, "resolve_entity", _resolve)
        return client

    return wire


@pytest.mark.asyncio
async def test_an_id_from_another_conversation_is_refused_before_any_delete(_peers):
    """`messages.deleteMessages` carries NO peer, so an id copied from another
    chat deletes a message THERE while the caller names this one - irreversibly
    and with no error."""
    client = _peers(_PeerClient({5: 1, 6: 2}))

    said = await mod.delete_messages_bulk(1, [5, 6], account="a")

    assert "Refusing" in said
    assert "6" in said
    assert client.sent_ids == [], "it deleted anyway"


@pytest.mark.asyncio
async def test_ids_that_are_all_in_this_chat_are_deleted(_peers):
    client = _peers(_PeerClient({5: 1, 6: 1}))

    said = await mod.delete_messages_bulk(1, [5, 6], account="a")

    assert client.sent_ids == [5, 6]
    assert "2 message(s)" in said


@pytest.mark.asyncio
async def test_a_repeated_id_is_one_deletion_not_two(_peers):
    client = _peers(_PeerClient({5: 1}))

    said = await mod.delete_messages_bulk(1, [5, 5, 5], account="a")

    assert client.sent_ids == [5]
    assert "1 message(s)" in said


@pytest.mark.asyncio
async def test_a_missing_id_is_reported_rather_than_counted(_peers):
    client = _peers(_PeerClient({5: 1, 7: None}))

    said = await mod.delete_messages_bulk(1, [5, 7], account="a")

    assert client.sent_ids == [5]
    assert "already gone" in said


@pytest.mark.asyncio
async def test_no_count_is_claimed_that_telegram_did_not_give(_peers):
    """`pts_count` counts update events, not messages removed. Reported as a
    deletion count it produced sentences like "Deleted 7 of 2 messages"."""
    client = _peers(_PeerClient({5: 1, 6: 1}))

    said = await mod.delete_messages_bulk(1, [5, 6], account="a")

    assert client.sent_ids == [5, 6], "the deletion itself must still have happened"
    assert "99" not in said, "the update counter was reported as a deletion count"


@pytest.mark.asyncio
async def test_more_than_a_hundred_ids_are_sent_in_chunks(_peers):
    ids = list(range(1, 151))
    client = _peers(_PeerClient({i: 1 for i in ids}))

    await mod.delete_messages_bulk(1, ids, account="a")

    assert [len(getattr(r, "id", [])) for r in client.requests] == [100, 50]
    assert client.sent_ids == ids


@pytest.mark.asyncio
async def test_a_channel_needs_no_preflight(_peers, monkeypatch):
    """A channel request carries the peer, so an id from elsewhere is simply not
    found there - and `get_messages` must not be called at all."""
    channel = Channel(
        id=7,
        title="c",
        photo=None,
        date=datetime.datetime.now(datetime.timezone.utc),
        broadcast=True,
    )
    client = _PeerClient({})

    async def _boom(entity, ids):
        raise AssertionError("a channel delete must not run the account-global preflight")

    client.get_messages = _boom
    monkeypatch.setattr(mod, "get_client", lambda account=None: client)

    async def _ensure(_client):
        return None

    async def _resolve(chat_id, _client):
        return channel

    monkeypatch.setattr(mod, "ensure_connected", _ensure)
    monkeypatch.setattr(mod, "resolve_entity", _resolve)

    said = await mod.delete_messages_bulk(-100, [5, 6], account="a")

    assert client.sent_ids == [5, 6]
    assert "2 message(s)" in said


# --- a channel has no deletion that only you see ----------------------------


@pytest.mark.asyncio
async def test_a_private_deletion_is_refused_in_a_channel_rather_than_widened(_wire, monkeypatch):
    """`revoke=False` asks for something a channel does not have. Going ahead
    would remove the post for every subscriber while the caller believed they
    were tidying their own view."""
    client = _wire(_Client())

    async def _resolve_channel(chat_id, _client):
        return Channel(
            id=chat_id,
            title="a channel",
            photo=None,
            date=datetime.datetime.now(datetime.timezone.utc),
            broadcast=True,
        )

    monkeypatch.setattr(mod, "resolve_entity", _resolve_channel)

    said = await mod.delete_message(-1001129051609, 973, account="a")

    assert "Refusing" in said
    assert "revoke=True" in said
    assert client.deleted == [], "it deleted for everyone anyway"
