"""Topic management beyond create/edit, and turning a basic group into a supergroup (spec 020).

What these pin: a topic is deleted until Telegram reports nothing left, General (id 1) is
never deleted, pin/unpin and the pinned order reach Telegram as given, topics can be turned
off, and an upgrade returns the NEW supergroup id and can turn topics on in the same call.
Every refusal happens before a request is sent.
"""

import json
from datetime import datetime, timezone

import pytest
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tools import topic_admin as mod

WHEN = datetime(2026, 9, 28, tzinfo=timezone.utc)
FORUM = tl.Channel(
    id=700,
    title="Forum",
    photo=tl.ChatPhotoEmpty(),
    date=WHEN,
    megagroup=True,
    forum=True,
    access_hash=7,
)
SUPER = tl.Channel(
    id=701, title="Plain", photo=tl.ChatPhotoEmpty(), date=WHEN, megagroup=True, access_hash=8
)
BASIC = tl.Chat(
    id=55, title="Basic", photo=tl.ChatPhotoEmpty(), participants_count=3, date=WHEN, version=1
)
NEW = tl.Channel(
    id=9001, title="Basic", photo=tl.ChatPhotoEmpty(), date=WHEN, megagroup=True, access_hash=9
)


class _Client:
    def __init__(self, answers):
        self.answers = answers
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        answer = self.answers.get(type(request).__name__)
        if isinstance(answer, list):
            answer = answer.pop(0)
        if isinstance(answer, Exception):
            raise answer
        return answer

    def of(self, kind):
        return [r for r in self.sent if isinstance(r, kind)]


@pytest.fixture
def wire(monkeypatch):
    def use(answers=None, entity=FORUM):
        client = _Client(answers or {})

        async def _resolve(value, cl=None, account=None):
            return entity

        async def _connected(cl=None):
            return None

        monkeypatch.setattr(mod, "get_client", lambda account=None: client)
        monkeypatch.setattr(mod, "resolve_entity", _resolve)
        monkeypatch.setattr(mod, "ensure_connected", _connected)
        return client

    return use


def _affected(offset):
    return tl.messages.AffectedHistory(pts=1, pts_count=1, offset=offset)


@pytest.mark.asyncio
async def test_a_topic_is_deleted_until_nothing_is_left(wire):
    c = wire({"DeleteTopicHistoryRequest": [_affected(40), _affected(0)]})
    payload = json.loads(await mod.delete_forum_topic("f", 12))
    assert payload["results"][0]["deleted"] is True
    requests = c.of(functions.messages.DeleteTopicHistoryRequest)
    assert [r.top_msg_id for r in requests] == [12, 12]


@pytest.mark.asyncio
async def test_general_is_never_deleted(wire):
    c = wire()
    assert "General" in await mod.delete_forum_topic("f", 1)
    assert c.sent == []


@pytest.mark.asyncio
async def test_topics_need_a_forum(wire):
    c = wire(entity=SUPER)
    assert "forum" in (await mod.delete_forum_topic("f", 12)).lower()
    assert "forum" in (await mod.pin_forum_topic("f", 12)).lower()
    assert c.sent == []


@pytest.mark.asyncio
@pytest.mark.parametrize("pinned", [True, False])
async def test_a_topic_is_pinned_or_unpinned(wire, pinned):
    c = wire({"UpdatePinnedForumTopicRequest": tl.Updates([], [], [], WHEN, 0)})
    await mod.pin_forum_topic("f", 12, pinned=pinned)
    (request,) = c.of(functions.messages.UpdatePinnedForumTopicRequest)
    assert request.topic_id == 12 and request.pinned is pinned


@pytest.mark.asyncio
async def test_the_pinned_order_is_sent_as_given(wire):
    c = wire({"ReorderPinnedForumTopicsRequest": tl.Updates([], [], [], WHEN, 0)})
    await mod.reorder_pinned_topics("f", [30, 12, 5])
    (request,) = c.of(functions.messages.ReorderPinnedForumTopicsRequest)
    assert request.order == [30, 12, 5] and not request.force
    assert "at least one" in await mod.reorder_pinned_topics("f", [])


@pytest.mark.asyncio
async def test_topics_can_be_turned_off(wire):
    c = wire({"ToggleForumRequest": tl.Updates([], [], [], WHEN, 0)})
    await mod.disable_forum_topics("f")
    (request,) = c.of(functions.channels.ToggleForumRequest)
    assert request.enabled is False
    c = wire(entity=SUPER)
    assert "already" in (await mod.disable_forum_topics("f")).lower() and c.sent == []


@pytest.mark.asyncio
async def test_an_upgrade_returns_the_new_supergroup_and_can_enable_topics(wire):
    old = tl.Chat(
        id=55,
        title="Basic",
        photo=tl.ChatPhotoEmpty(),
        participants_count=3,
        date=WHEN,
        version=2,
        migrated_to=tl.InputChannel(9001, 9),
    )
    answer = tl.Updates([], [], [old, NEW], WHEN, 0)
    c = wire(
        {"MigrateChatRequest": answer, "ToggleForumRequest": tl.Updates([], [], [], WHEN, 0)},
        entity=BASIC,
    )
    payload = json.loads(await mod.upgrade_to_supergroup("g", enable_topics=True))
    row = payload["results"][0]
    assert row["new_chat_id"] == -1000000009001 and row["old_chat_id"] == -55
    assert row["topics_enabled"] is True
    (migrate,) = c.of(functions.messages.MigrateChatRequest)
    assert migrate.chat_id == 55
    (toggle,) = c.of(functions.channels.ToggleForumRequest)
    assert toggle.enabled is True and toggle.channel.id == 9001


@pytest.mark.asyncio
async def test_only_a_basic_group_is_upgraded(wire):
    c = wire(entity=SUPER)
    assert "already a supergroup" in await mod.upgrade_to_supergroup("g")
    assert c.sent == []


@pytest.mark.asyncio
async def test_a_telegram_refusal_is_named(wire):
    from telethon.errors import RPCError

    wire({"DeleteTopicHistoryRequest": RPCError(None, "TOPIC_ID_INVALID", 400)})
    assert "TOPIC_ID_INVALID" in await mod.delete_forum_topic("f", 99)


@pytest.mark.asyncio
async def test_one_topic_is_marked_read_up_to_its_newest_message(wire, monkeypatch):
    """Upstream chigwell PR #82 (forum part, rewritten): send_read_acknowledge left a
    topic unread. The topic's own read cursor moves to its top message, and its
    mentions are cleared."""
    from telegram_mcp.tools import read_receipts

    topic = tl.ForumTopic(
        id=12,
        date=WHEN,
        peer=tl.PeerChannel(700),
        title="T",
        icon_color=0,
        top_message=345,
        read_inbox_max_id=300,
        read_outbox_max_id=0,
        unread_count=4,
        unread_mentions_count=1,
        unread_reactions_count=0,
        unread_poll_votes_count=0,
        from_id=tl.PeerUser(1),
        notify_settings=tl.PeerNotifySettings(),
    )
    forum_topics = tl.messages.ForumTopics(
        count=1, topics=[topic], messages=[], chats=[], users=[], pts=1
    )
    c = wire({"GetForumTopicsByIDRequest": forum_topics})
    for name in ("get_client", "resolve_entity"):
        monkeypatch.setattr(read_receipts, name, getattr(mod, name))

    await read_receipts.mark_as_read("f", topic_id=12)
    (read,) = c.of(functions.messages.ReadDiscussionRequest)
    assert read.msg_id == 12 and read.read_max_id == 345
    (mentions,) = c.of(functions.messages.ReadMentionsRequest)
    assert mentions.top_msg_id == 12
