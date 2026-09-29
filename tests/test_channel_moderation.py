"""Anti-spam and boost settings (plan 015, moderation group)."""

import pytest
import telethon.errors.rpcerrorlist as rpc
from telethon.tl import functions

from telegram_mcp.tools import channel_moderation as mod
from telethon.tl import types
from test_channel_identity import BASIC_GROUP, CHANNEL, _wire

# Anti-spam and boost limits exist for supergroups only (measured live 2026-09-29: a
# broadcast channel answered ChatIdInvalidError).
SUPERGROUP = types.Channel(
    id=556, title="Talk", photo=None, date=None, creator=True, left=False, megagroup=True
)


def _w(monkeypatch, entity=SUPERGROUP, answer=True):
    return _wire(monkeypatch, entity, answer, modules=(mod,))


@pytest.mark.parametrize(
    "tool,kwargs,request_type,checks",
    [
        (
            "set_anti_spam",
            {"enabled": True},
            functions.channels.ToggleAntiSpamRequest,
            {"enabled": True},
        ),
        (
            "set_anti_spam",
            {"enabled": False},
            functions.channels.ToggleAntiSpamRequest,
            {"enabled": False},
        ),
        (
            "report_anti_spam_false_positive",
            {"message_id": 42},
            functions.channels.ReportAntiSpamFalsePositiveRequest,
            {"msg_id": 42},
        ),
        (
            "set_boosts_to_unblock",
            {"boosts": 3},
            functions.channels.SetBoostsToUnblockRestrictionsRequest,
            {"boosts": 3},
        ),
        (
            "set_boosts_to_unblock",
            {"boosts": 0},
            functions.channels.SetBoostsToUnblockRestrictionsRequest,
            {"boosts": 0},
        ),
    ],
)
@pytest.mark.asyncio
async def test_each_tool_sends_its_own_request(monkeypatch, tool, kwargs, request_type, checks):
    client = _w(monkeypatch)

    await getattr(mod, tool)("@announcements", **kwargs)

    assert len(client.sent) == 1
    request = client.sent[0]
    assert isinstance(request, request_type)
    assert request.channel is SUPERGROUP
    for field, value in checks.items():
        assert getattr(request, field) == value


@pytest.mark.parametrize("boosts", [-1, "many", 1.5, True])
@pytest.mark.asyncio
async def test_boosts_must_be_a_whole_number_from_zero(monkeypatch, boosts):
    client = _w(monkeypatch)

    result = await mod.set_boosts_to_unblock("@announcements", boosts)

    assert client.sent == []
    assert "boosts" in result


@pytest.mark.parametrize(
    "call",
    [
        lambda: mod.set_anti_spam(-777, True),
        lambda: mod.report_anti_spam_false_positive(-777, 1),
        lambda: mod.set_boosts_to_unblock(-777, 1),
    ],
)
@pytest.mark.asyncio
async def test_a_basic_group_gets_a_sentence(monkeypatch, call):
    client = _w(monkeypatch, BASIC_GROUP)

    result = await call()

    assert client.sent == []
    assert "basic group" in result.lower()


@pytest.mark.asyncio
async def test_admin_required_is_a_sentence(monkeypatch):
    _w(monkeypatch, answer=rpc.ChatAdminRequiredError(request=None))

    result = await mod.set_anti_spam("@announcements", True)

    assert result == "Cannot change this setting: admin privileges are required."


@pytest.mark.parametrize(
    "call",
    [
        lambda: mod.set_anti_spam("@announcements", True),
        lambda: mod.report_anti_spam_false_positive("@announcements", 1),
        lambda: mod.set_boosts_to_unblock("@announcements", 1),
    ],
)
@pytest.mark.asyncio
async def test_a_broadcast_channel_gets_a_sentence(monkeypatch, call):
    client = _w(monkeypatch, CHANNEL)

    result = await call()

    assert client.sent == []
    assert "supergroup" in result.lower()
