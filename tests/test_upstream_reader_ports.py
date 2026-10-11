"""Approved US3 read/pin seams, using Telegram-shaped external clients only."""

import json
from types import SimpleNamespace

import pytest
from telethon.tl import types

from telegram_mcp.tools import members, messages_read, messages_state


class HistoryClient:
    def __init__(self):
        self.calls = []

    async def get_messages(self, entity, **kwargs):
        self.calls.append(kwargs)
        # General has no top_id, and nested replies name another message.
        if kwargs.get("reply_to") == 1:
            return [types.Message(id=90, peer_id=types.PeerChannel(7), message="general")]
        return [
            types.Message(
                id=89,
                peer_id=types.PeerChannel(7),
                message="nested",
                reply_to=types.MessageReplyHeader(
                    reply_to_msg_id=88, reply_to_top_id=50, forum_topic=True
                ),
            )
        ]


@pytest.mark.asyncio
@pytest.mark.parametrize("topic,expected", [(50, 89), (1, 90)])
async def test_topic_history_is_server_scoped_including_nested_and_general(
    wire_client, topic, expected
):
    client = wire_client(
        messages_read,
        HistoryClient(),
        entity=types.Channel(
            id=7, title="Forum", photo=None, date=None, megagroup=True, forum=True
        ),
    )
    payload = json.loads(await messages_read.get_history(7, 999, "default", topic_id=topic))
    assert client.calls == [{"limit": 200, "reply_to": topic}]
    assert [record["id"] for record in payload["results"]] == [expected]
    assert (payload["requested_limit"], payload["effective_limit"]) == (999, 200)


@pytest.mark.asyncio
@pytest.mark.parametrize("topic", [True, False, "1", 1.0, 0, -1, 2**31])
async def test_invalid_topic_is_refused_without_fetching(wire_client, topic):
    client = wire_client(messages_read, HistoryClient())
    result = await messages_read.get_history(7, account="default", topic_id=topic)
    assert result.startswith("Error: topic_id")
    assert client.calls == []


@pytest.mark.asyncio
async def test_ordinary_history_preserves_positional_account_and_omits_reply_to(wire_client):
    client = wire_client(messages_read, HistoryClient())
    await messages_read.get_history(7, 2, "default")
    assert client.calls == [{"limit": 2}]


@pytest.mark.asyncio
async def test_topic_history_refuses_nonforum(wire_client):
    client = wire_client(
        messages_read,
        HistoryClient(),
        entity=types.Channel(id=7, title="Plain", photo=None, date=None, megagroup=True),
    )
    result = await messages_read.get_history(7, account="default", topic_id=1)
    assert "forum supergroup" in result
    assert client.calls == []


class ParticipantClient:
    def __init__(self, count):
        self.count = count
        self.yielded = 0
        self.limit = None

    async def iter_participants(self, chat, *, limit):
        self.limit = limit
        # Basic groups ignore limit: the caller must stop even this stream.
        for i in range(self.count):
            self.yielded += 1
            yield SimpleNamespace(id=i, first_name="User", participant=SimpleNamespace(rank="tag"))


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "count,page,expected,more,yielded",
    [
        (4, 2, [2, 3], False, 4),
        (5, 2, [2, 3], True, 5),
        (1, 2, [], False, 1),
        (100, 1, [0, 1], True, 3),
    ],
)
async def test_member_page_has_real_lookahead_and_empty_metadata(
    wire_client, count, page, expected, more, yielded
):
    client = wire_client(members, ParticipantClient(count))
    payload = json.loads(await members.get_participants(7, page, 2, "default"))
    assert [record["id"] for record in payload["results"]] == expected
    assert payload["has_more"] is more
    assert (payload["page"], payload["offset"], payload["returned"]) == (
        page,
        (page - 1) * 2,
        len(expected),
    )
    assert client.yielded == yielded
    assert client.limit == (page - 1) * 2 + 3
    assert all(record["tag"] == "tag" for record in payload["results"])


class PinClient:
    def __init__(self):
        self.calls = []

    async def pin_message(self, entity, message_id, *, notify=False):
        self.calls.append((message_id, notify))


@pytest.mark.asyncio
@pytest.mark.parametrize("notify", [False, True])
async def test_pin_notification_matches_public_approval_detail(wire_client, notify):
    from telegram_mcp.approval_details import detail_for

    client = wire_client(messages_state, PinClient())
    result = await messages_state.pin_message(7, 99, "default", notify=notify)
    assert "pinned" in result
    assert client.calls == [(99, notify)]
    detail = await detail_for("pin_message", {"notify": notify})
    assert detail == ("Notifications: on" if notify else "Notifications: off (silent)")
    assert await detail_for("pin_message", {}) == "Notifications: off (silent)"
