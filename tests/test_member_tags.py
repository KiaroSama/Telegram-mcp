"""Group member tags (spec 010), on fake clients.

A member tag is the short label Telegram shows next to a member's name in a group
(`messages.editChatParticipantRank`). What these pin: the limits (16 characters, no emoji)
are enforced before anything is sent, an empty tag clears one, the member list reports each
tag, and the group default permissions keep allowing self-tags unless told otherwise.
"""

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import members as mod
from telegram_mcp.tools import group_permissions

GROUP = types.Channel(
    id=777,
    title="Test Group",
    photo=types.ChatPhotoEmpty(),
    date=None,
    megagroup=True,
    access_hash=7,
)
NEWS = types.Channel(
    id=555, title="News", photo=types.ChatPhotoEmpty(), date=None, broadcast=True, access_hash=5
)
BOT = types.User(id=42, first_name="Numera Group Bot 1", bot=True, access_hash=4)


class FakeClient:
    def __init__(self):
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        return types.Updates(updates=[], users=[], chats=[], date=None, seq=0)

    async def get_me(self, input_peer=False):
        return types.InputPeerSelf()

    async def iter_participants(self, chat, limit=None):
        tagged = types.User(id=42, first_name="Numera Group Bot 1", bot=True, access_hash=4)
        tagged.participant = types.ChannelParticipant(user_id=42, date=None, rank="Mossad")
        plain = types.User(id=43, first_name="Sara", access_hash=3)
        plain.participant = types.ChannelParticipant(user_id=43, date=None)
        for user in (tagged, plain):
            yield user


@pytest.fixture
def client(monkeypatch):
    fake = FakeClient()
    names = {"@testgroup_x": GROUP, "@newschan": NEWS, "@numerabot1": BOT}

    async def _resolve(value, cl=None, account=None):
        return names[value]

    async def _connected(cl=None):
        return None

    for module in (mod, group_permissions):
        monkeypatch.setattr(module, "get_client", lambda account=None: fake)
        monkeypatch.setattr(module, "resolve_entity", _resolve)
        monkeypatch.setattr(module, "ensure_connected", _connected)
    return fake


# --- US1: tag a member ---------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_member_is_tagged_with_one_request(client):
    await mod.set_member_tag(chat="@testgroup_x", user="@numerabot1", tag="Mossad")
    (request,) = client.sent
    assert isinstance(request, functions.messages.EditChatParticipantRankRequest)
    assert request.peer is GROUP and request.participant is BOT and request.rank == "Mossad"


@pytest.mark.asyncio
async def test_without_a_user_the_owner_tags_themselves(client):
    await mod.set_member_tag(chat="@testgroup_x", tag="Owner")
    assert isinstance(client.sent[0].participant, types.InputPeerSelf)


@pytest.mark.asyncio
async def test_an_empty_tag_clears_it(client):
    text = await mod.set_member_tag(chat="@testgroup_x", user="@numerabot1", tag="")
    assert client.sent[0].rank == "" and "removed" in text.lower()


@pytest.mark.parametrize("tag", ["x" * 17, "Boss 😎", "★ Star"])
@pytest.mark.asyncio
async def test_a_tag_outside_the_limits_is_refused_before_sending(client, tag):
    text = await mod.set_member_tag(chat="@testgroup_x", user="@numerabot1", tag=tag)
    assert client.sent == [] and ("16" in text or "emoji" in text)


@pytest.mark.asyncio
async def test_sixteen_characters_is_allowed(client):
    await mod.set_member_tag(chat="@testgroup_x", user="@numerabot1", tag="x" * 16)
    assert client.sent[0].rank == "x" * 16


@pytest.mark.asyncio
async def test_a_channel_has_no_member_tags(client):
    text = await mod.set_member_tag(chat="@newschan", user="@numerabot1", tag="Mossad")
    assert client.sent == [] and "group" in text.lower()


@pytest.mark.asyncio
async def test_the_member_list_shows_each_tag(client):
    import json

    rows = json.loads(await mod.get_participants(chat_id="@testgroup_x"))["results"]
    by_id = {r["id"]: r for r in rows}
    assert by_id[42]["tag"] == "Mossad"
    assert "tag" not in by_id[43]


# --- US2: members may tag themselves ---------------------------------------------------


@pytest.mark.asyncio
async def test_self_tags_can_be_turned_off(client):
    await group_permissions.set_group_permissions(chat_id="@testgroup_x", edit_own_tags=False)
    assert client.sent[0].banned_rights.edit_rank is True


@pytest.mark.asyncio
async def test_default_permissions_still_allow_self_tags(client):
    """An item not passed keeps its current value: changing photos leaves self-tags on."""
    await group_permissions.set_group_permissions(chat_id="@testgroup_x", photos=False)
    assert not client.sent[0].banned_rights.edit_rank


@pytest.mark.asyncio
async def test_a_missing_last_name_is_not_printed_as_none(client):
    """Live 2026-09-27: members without a last name came back as "Numera Group Bot 1 None"."""
    import json

    rows = json.loads(await mod.get_participants(chat_id="@testgroup_x"))["results"]
    assert [r["name"] for r in rows] == ["Numera Group Bot 1", "Sara"]
