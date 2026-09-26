"""Clearing a chat's history, and deleting a chat (spec 008, US4), on a fake client.

What these pin: "both sides" is never assumed. A call without it is refused before
anything reaches Telegram, telling the agent to ask the owner. Clear history keeps the
chat (just_clear); Delete chat removes it, and for a group or channel that means
leaving it - the group itself is never deleted.
"""

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import chat_removal as mod

BOT = types.User(id=42, first_name="userinfobot", bot=True, access_hash=4)
ME = types.User(id=1, first_name="Me", is_self=True, access_hash=1)
BASIC = types.Chat(
    id=9,
    title="Old Group",
    photo=types.ChatPhotoEmpty(),
    participants_count=3,
    date=None,
    version=1,
)
SUPER = types.Channel(
    id=777, title="Talk", photo=types.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=7
)
CHANNEL = types.Channel(
    id=555, title="News", photo=types.ChatPhotoEmpty(), date=None, broadcast=True, access_hash=5
)

_DONE = types.messages.AffectedHistory(pts=1, pts_count=1, offset=0)


class FakeClient:
    def __init__(self):
        self.sent = []
        self.history_answers = []

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(request, functions.messages.DeleteHistoryRequest):
            return self.history_answers.pop(0) if self.history_answers else _DONE
        if isinstance(request, functions.channels.DeleteHistoryRequest):
            return types.Updates(updates=[], users=[], chats=[], date=None, seq=0)
        return types.Updates(updates=[], users=[], chats=[], date=None, seq=0)


@pytest.fixture
def client(monkeypatch):
    fake = FakeClient()
    by_name = {
        "@userinfobot": BOT,
        "@me_myself": ME,
        "@basicgrp": BASIC,
        "@supergrp": SUPER,
        "@newschan": CHANNEL,
    }

    async def _resolve(value, cl=None, account=None):
        return by_name[value]

    monkeypatch.setattr(mod, "get_client", lambda account=None: fake)
    monkeypatch.setattr(mod, "resolve_entity", _resolve)
    return fake


@pytest.mark.parametrize("tool", ["clear_chat_history", "delete_chat"])
@pytest.mark.asyncio
async def test_both_sides_is_never_assumed(client, tool):
    text = await getattr(mod, tool)(chat="@userinfobot")
    assert "ask the owner" in text and client.sent == []


@pytest.mark.parametrize("both", [False, True])
@pytest.mark.asyncio
async def test_clear_keeps_the_chat(client, both):
    await mod.clear_chat_history(chat="@userinfobot", both_sides=both)
    request = client.sent[0]
    assert isinstance(request, functions.messages.DeleteHistoryRequest)
    assert request.just_clear is True and bool(request.revoke) is both


@pytest.mark.asyncio
async def test_a_partial_answer_is_repeated_until_telegram_says_done(client):
    client.history_answers = [types.messages.AffectedHistory(pts=1, pts_count=1, offset=50)]
    text = await mod.clear_chat_history(chat="@userinfobot", both_sides=False)
    assert len(client.sent) == 2 and "cleared" in text.lower()


@pytest.mark.asyncio
async def test_clearing_a_supergroup_uses_the_channel_request(client):
    await mod.clear_chat_history(chat="@supergrp", both_sides=False)
    request = client.sent[0]
    assert isinstance(request, functions.channels.DeleteHistoryRequest)
    assert not request.for_everyone


@pytest.mark.asyncio
async def test_a_broadcast_channel_has_no_history_to_clear(client):
    text = await mod.clear_chat_history(chat="@newschan", both_sides=False)
    assert "channel" in text.lower() and client.sent == []


@pytest.mark.parametrize("chat", ["@basicgrp", "@supergrp", "@newschan", "@me_myself"])
@pytest.mark.asyncio
async def test_both_sides_where_it_does_not_exist_is_refused(client, chat):
    text = await mod.delete_chat(chat=chat, both_sides=True)
    assert "both_sides=false" in text and client.sent == []


@pytest.mark.parametrize("both", [False, True])
@pytest.mark.asyncio
async def test_deleting_a_private_chat_removes_it(client, both):
    await mod.delete_chat(chat="@userinfobot", both_sides=both)
    request = client.sent[0]
    assert isinstance(request, functions.messages.DeleteHistoryRequest)
    assert not request.just_clear and bool(request.revoke) is both


@pytest.mark.parametrize("chat", ["@supergrp", "@newschan"])
@pytest.mark.asyncio
async def test_deleting_a_channel_or_supergroup_leaves_it(client, chat):
    text = await mod.delete_chat(chat=chat, both_sides=False)
    assert [type(r) for r in client.sent] == [functions.channels.LeaveChannelRequest]
    assert "left" in text.lower()


@pytest.mark.asyncio
async def test_deleting_a_basic_group_leaves_then_removes_the_chat(client):
    await mod.delete_chat(chat="@basicgrp", both_sides=False)
    leave, remove = client.sent
    assert isinstance(leave, functions.messages.DeleteChatUserRequest)
    assert isinstance(leave.user_id, types.InputUserSelf)
    assert isinstance(remove, functions.messages.DeleteHistoryRequest) and not remove.just_clear
