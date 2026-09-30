"""Recent actions (admin log) filtered the way Telegram Desktop 7.2.10's filter dialog does.

Desktop groups the checkboxes into Members and admins / Group settings / Messages and maps
each to ChannelAdminLogEventsFilter flags (history_admin_log_filter.cpp, _inner.cpp). What
these pin: each checkbox asks for exactly Desktop's flags, a wrong type is refused before any
request, admins and the search text reach the request, paging goes back through max_id, and
every event comes back with its actor, time and Desktop's name for its type.
"""

import json
from datetime import datetime, timezone

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import moderation as mod

GROUP = types.Channel(
    id=777, title="Test Group", photo=types.ChatPhotoEmpty(), date=None, megagroup=True
)
NEWS = types.Channel(id=555, title="News", photo=types.ChatPhotoEmpty(), date=None, broadcast=True)
ADMIN = types.User(id=42, first_name="Sara", username="sara_admin", access_hash=4)
WHEN = datetime(2026, 10, 1, 12, 0, tzinfo=timezone.utc)


def _event(event_id, action):
    return types.ChannelAdminLogEvent(id=event_id, date=WHEN, user_id=42, action=action)


DELETED = types.ChannelAdminLogEventActionDeleteMessage(
    message=types.Message(id=9, peer_id=types.PeerChannel(777), date=WHEN, message="spam")
)


class FakeClient:
    def __init__(self, events=None):
        self.sent = []
        self.events = [_event(30, DELETED)] if events is None else events

    async def __call__(self, request):
        self.sent.append(request)
        return types.channels.AdminLogResults(events=self.events, chats=[], users=[ADMIN])


@pytest.fixture
def wire(wire_client):
    def _wire(entity=GROUP, events=None):
        client = FakeClient(events)
        names = {"@group": entity, "@sara_admin": ADMIN, 42: ADMIN}

        async def _resolve(value, cl=None, account=None):
            return names[value]

        return wire_client(mod, client, resolve=_resolve)

    return _wire


def _filter_flags(request):
    flt = request.events_filter
    if flt is None:
        return None
    return {k for k, v in flt.to_dict().items() if v is True}


@pytest.mark.asyncio
async def test_no_filter_asks_for_everything_newest_first(wire):
    client = wire()
    rows = json.loads(await mod.get_recent_actions("@group"))
    (request,) = client.sent
    assert isinstance(request, functions.channels.GetAdminLogRequest)
    assert request.events_filter is None and not request.admins
    assert request.q == "" and request.max_id == 0 and request.limit == 20
    (event,) = rows["results"]
    assert event["id"] == 30
    assert event["type"] == "Deleted messages"
    assert event["action"] == "DeleteMessage"
    assert event["time"].startswith("2026-10-01T12:00")
    assert event["actor"] == {"id": 42, "name": "Sara", "username": "sara_admin"}
    assert event["details"]["message"]["message"] == "spam"


@pytest.mark.parametrize(
    "event_types,flags",
    [
        (["admin_rights"], {"promote", "demote"}),
        (["tag_changes"], {"edit_rank"}),
        (["new_restrictions"], {"ban", "unban", "kick", "unkick"}),
        (["new_members"], {"join", "invite"}),
        (["members_leaving"], {"leave"}),
        (["group_info"], {"info", "settings"}),
        (["invite_links"], {"invites"}),
        (["video_chats"], {"group_call"}),
        (["subscription_renewals"], {"sub_extend"}),
        (["topics"], {"forums"}),
        (["deleted_messages"], {"delete"}),
        (["edited_messages"], {"edit"}),
        (["pinned_messages"], {"pinned"}),
        (["deleted_messages", "admin_rights"], {"delete", "promote", "demote"}),
    ],
)
@pytest.mark.asyncio
async def test_each_desktop_checkbox_asks_for_exactly_its_flags(wire, event_types, flags):
    client = wire()
    await mod.get_recent_actions("@group", event_types=event_types)
    assert _filter_flags(client.sent[0]) == flags


@pytest.mark.asyncio
async def test_an_unknown_type_is_refused_before_any_request(wire):
    client = wire()
    text = await mod.get_recent_actions("@group", event_types=["deleted", "admin_rights"])
    assert client.sent == []
    assert "deleted" in text and "deleted_messages" in text


@pytest.mark.parametrize("event_type", ["topics", "pinned_messages"])
@pytest.mark.asyncio
async def test_a_group_only_type_is_refused_on_a_channel(wire, event_type):
    """Desktop offers Topics and Pinned messages for groups only."""
    client = wire(entity=NEWS)
    text = await mod.get_recent_actions("@group", event_types=[event_type])
    assert client.sent == [] and "group" in text.lower()


@pytest.mark.asyncio
async def test_admins_and_search_text_reach_the_request(wire):
    client = wire()
    await mod.get_recent_actions("@group", admins=["@sara_admin"], query="spam")
    (request,) = client.sent
    assert request.q == "spam"
    assert [a.user_id for a in request.admins] == [42]


@pytest.mark.asyncio
async def test_a_full_page_says_how_to_go_further_back(wire):
    events = [_event(i, DELETED) for i in (50, 49, 48)]
    client = wire(events=events)
    reply = json.loads(await mod.get_recent_actions("@group", limit=3, max_id=51))
    assert client.sent[0].max_id == 51 and client.sent[0].limit == 3
    assert reply["next_max_id"] == 48


@pytest.mark.asyncio
async def test_a_short_page_has_nothing_further_back(wire):
    wire()
    reply = json.loads(await mod.get_recent_actions("@group", limit=5))
    assert reply.get("next_max_id") is None


@pytest.mark.asyncio
async def test_a_huge_limit_is_clamped_to_desktops_page(wire):
    client = wire()
    reply = json.loads(await mod.get_recent_actions("@group", limit=5000))
    assert client.sent[0].limit == 100
    assert reply["requested_limit"] == 5000 and reply["effective_limit"] == 100


@pytest.mark.parametrize("max_id", [-1, True, "x"])
@pytest.mark.asyncio
async def test_a_bad_max_id_is_refused(wire, max_id):
    client = wire()
    text = await mod.get_recent_actions("@group", max_id=max_id)
    assert client.sent == [] and "max_id" in text


@pytest.mark.parametrize(
    "action,group_name,channel_name",
    [
        (
            types.ChannelAdminLogEventActionParticipantJoin(),
            "New members",
            "New subscribers",
        ),
        (
            types.ChannelAdminLogEventActionParticipantLeave(),
            "Members leaving",
            "Subscribers leaving",
        ),
        (
            types.ChannelAdminLogEventActionChangeTitle(prev_value="a", new_value="b"),
            "Group info",
            "Channel info",
        ),
        (
            types.ChannelAdminLogEventActionToggleSlowMode(prev_value=0, new_value=30),
            "Group info",
            "Channel info",
        ),
        (
            types.ChannelAdminLogEventActionDiscardGroupCall(
                call=types.InputGroupCall(id=1, access_hash=2)
            ),
            "Video chats",
            "Live stream",
        ),
        (
            types.ChannelAdminLogEventActionParticipantEditRank(
                user_id=42, prev_rank="", new_rank="Boss"
            ),
            "Tag Changes",
            "Tag Changes",
        ),
    ],
)
@pytest.mark.asyncio
async def test_each_event_carries_desktops_name_for_its_type(
    wire, action, group_name, channel_name
):
    wire(events=[_event(1, action)])
    (row,) = json.loads(await mod.get_recent_actions("@group"))["results"]
    assert row["type"] == group_name
    wire(entity=NEWS, events=[_event(1, action)])
    (row,) = json.loads(await mod.get_recent_actions("@group"))["results"]
    assert row["type"] == channel_name


@pytest.mark.asyncio
async def test_no_events_says_so(wire):
    wire(events=[])
    assert "No recent" in await mod.get_recent_actions("@group")
