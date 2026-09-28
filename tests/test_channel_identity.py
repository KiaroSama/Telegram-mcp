"""Channel usernames and the discussion-group candidates (plan 015, identity group).

The three username tools reuse `channel_settings._toggle`, so the seams that
matter (`get_client`, `resolve_entity`, `ensure_connected`) are patched on the
module that OWNS them: `channel_settings`. The read tool owns its own client.
"""

import pytest
from telethon.tl import functions, types

from telegram_mcp import connection as conn
from telegram_mcp.tools import channel_identity as mod
from telegram_mcp.tools import channel_settings as settings_mod

CHANNEL = types.Channel(
    id=555,
    title="Announcements",
    photo=None,
    date=None,
    creator=True,
    left=False,
    broadcast=True,
    megagroup=False,
)

BASIC_GROUP = types.Chat(
    id=777, title="Old Group", photo=None, participants_count=3, date=None, version=1
)


class Recorder:
    def __init__(self, answer=True):
        self.sent = []
        self.answer = answer

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(self.answer, Exception):
            raise self.answer
        return self.answer


def _wire(monkeypatch, entity, answer=True, modules=(mod,)):
    """Shared by the other channel suites: pass the module under test."""
    client = Recorder(answer)

    async def _resolve(chat_id, cl=None, account=None):
        return entity

    async def _connected(cl=None):
        return None

    # The wired client is the whole registry, as in conftest's `wire_client`: a
    # `.env` on the developer's machine must not make these tools multi-account.
    monkeypatch.setattr(conn, "refresh_accounts", lambda: [])
    monkeypatch.setattr(conn, "clients", {"default": client})
    for module in (settings_mod, *modules):
        monkeypatch.setattr(module, "get_client", lambda account=None: client)
        monkeypatch.setattr(module, "resolve_entity", _resolve)
        monkeypatch.setattr(module, "ensure_connected", _connected)
    return client


@pytest.mark.parametrize("active", [True, False])
@pytest.mark.asyncio
async def test_toggle_username_sends_the_flag_both_ways(monkeypatch, active):
    client = _wire(monkeypatch, CHANNEL)

    result = await mod.toggle_channel_username("@announcements", "@second_name", active)

    request = client.sent[0]
    assert isinstance(request, functions.channels.ToggleUsernameRequest)
    assert request.channel is CHANNEL
    assert request.username == "second_name", "the leading @ must not reach Telegram"
    assert request.active is active
    assert "second_name" in result


@pytest.mark.asyncio
async def test_reorder_usernames_sends_the_order_as_given(monkeypatch):
    client = _wire(monkeypatch, CHANNEL)

    await mod.reorder_channel_usernames("@announcements", ["@b", "a"])

    request = client.sent[0]
    assert isinstance(request, functions.channels.ReorderUsernamesRequest)
    assert request.order == ["b", "a"]


@pytest.mark.asyncio
async def test_an_empty_order_is_refused_before_any_request(monkeypatch):
    client = _wire(monkeypatch, CHANNEL)

    result = await mod.reorder_channel_usernames("@announcements", [])

    assert client.sent == []
    assert "at least one" in result


@pytest.mark.asyncio
async def test_an_empty_username_is_refused_before_any_request(monkeypatch):
    client = _wire(monkeypatch, CHANNEL)

    result = await mod.toggle_channel_username("@announcements", "  @ ", True)

    assert client.sent == []
    assert "username" in result.lower()


@pytest.mark.asyncio
async def test_deactivate_all_sends_its_request(monkeypatch):
    client = _wire(monkeypatch, CHANNEL)

    await mod.deactivate_channel_usernames("@announcements")

    assert isinstance(client.sent[0], functions.channels.DeactivateAllUsernamesRequest)
    assert client.sent[0].channel is CHANNEL


@pytest.mark.parametrize(
    "call",
    [
        lambda: mod.toggle_channel_username(-777, "x", True),
        lambda: mod.reorder_channel_usernames(-777, ["x"]),
        lambda: mod.deactivate_channel_usernames(-777),
    ],
)
@pytest.mark.asyncio
async def test_a_basic_group_gets_a_sentence_and_no_request(monkeypatch, call):
    client = _wire(monkeypatch, BASIC_GROUP)

    result = await call()

    assert client.sent == []
    assert "basic group" in result.lower()


@pytest.mark.asyncio
async def test_list_discussion_candidates_describes_each_group(monkeypatch):
    group = types.Channel(
        id=901,
        title="Talk​ Room",
        photo=None,
        date=None,
        megagroup=True,
        username="talkroom",
    )
    client = _wire(monkeypatch, None, answer=types.messages.Chats(chats=[group, BASIC_GROUP]))

    result = await mod.list_discussion_candidates()

    assert isinstance(client.sent[0], functions.channels.GetGroupsForDiscussionRequest)
    assert "Talk Room" in result, "titles are sanitized (zero-width removed)"
    assert "-1000000000901" in result and "talkroom" in result
    assert "Old Group" in result


def test_hints():
    from telegram_mcp.runtime import mcp

    tool = mcp._tool_manager.get_tool("list_discussion_candidates")
    assert tool.annotations.read_only_hint is True
    assert tool.annotations.destructive_hint is False
    for name in (
        "toggle_channel_username",
        "reorder_channel_usernames",
        "deactivate_channel_usernames",
    ):
        hints = mcp._tool_manager.get_tool(name).annotations
        assert hints.read_only_hint is False and hints.destructive_hint is False
