"""Convert, relocate, delete a channel, and its autotranslation (plan 015, structure group).

Two of these are one-way doors, so what is pinned here is what they SAY about
themselves as much as what they send: `destructiveHint=True`, and a docstring
that tells the approver the change cannot be undone.
"""

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import channel_structure as mod
from test_channel_identity import BASIC_GROUP, CHANNEL, _wire

SUPERGROUP = types.Channel(
    id=556, title="Big Group", photo=None, date=None, megagroup=True, broadcast=False
)


def _w(monkeypatch, entity=SUPERGROUP):
    return _wire(monkeypatch, entity, modules=(mod,))


@pytest.mark.asyncio
async def test_convert_sends_its_request_for_a_supergroup(monkeypatch):
    client = _w(monkeypatch)

    result = await mod.convert_to_gigagroup("@big")

    assert isinstance(client.sent[0], functions.channels.ConvertToGigagroupRequest)
    assert client.sent[0].channel is SUPERGROUP
    assert "cannot be undone" in result


@pytest.mark.asyncio
async def test_a_broadcast_channel_cannot_become_a_broadcast_group(monkeypatch):
    client = _w(monkeypatch, CHANNEL)

    result = await mod.convert_to_gigagroup("@announcements")

    assert client.sent == []
    assert "supergroup" in result


@pytest.mark.asyncio
async def test_delete_sends_its_request(monkeypatch):
    client = _w(monkeypatch, CHANNEL)

    result = await mod.delete_channel("@announcements")

    assert isinstance(client.sent[0], functions.channels.DeleteChannelRequest)
    assert client.sent[0].channel is CHANNEL
    assert "deleted" in result


@pytest.mark.asyncio
async def test_location_carries_point_and_address(monkeypatch):
    client = _w(monkeypatch)

    await mod.set_channel_location("@big", 35.7, 51.4, "Tehran")

    request = client.sent[0]
    assert isinstance(request, functions.channels.EditLocationRequest)
    assert isinstance(request.geo_point, types.InputGeoPoint)
    assert (request.geo_point.lat, request.geo_point.long) == (35.7, 51.4)
    assert request.address == "Tehran"


@pytest.mark.parametrize(
    "lat,long,address",
    [(91, 0, "x"), (0, -181, "x"), (0, 0, "  "), ("north", 0, "x")],
)
@pytest.mark.asyncio
async def test_location_is_checked_before_any_request(monkeypatch, lat, long, address):
    client = _w(monkeypatch)

    result = await mod.set_channel_location("@big", lat, long, address)

    assert client.sent == []
    assert result


@pytest.mark.parametrize("enabled", [True, False])
@pytest.mark.asyncio
async def test_autotranslation_both_ways(monkeypatch, enabled):
    client = _w(monkeypatch, CHANNEL)

    await mod.set_autotranslation("@announcements", enabled)

    assert isinstance(client.sent[0], functions.channels.ToggleAutotranslationRequest)
    assert client.sent[0].enabled is enabled


@pytest.mark.parametrize(
    "call",
    [
        lambda: mod.convert_to_gigagroup(-777),
        lambda: mod.delete_channel(-777),
        lambda: mod.set_channel_location(-777, 1, 1, "x"),
        lambda: mod.set_autotranslation(-777, True),
    ],
)
@pytest.mark.asyncio
async def test_a_basic_group_gets_a_sentence(monkeypatch, call):
    client = _w(monkeypatch, BASIC_GROUP)

    result = await call()

    assert client.sent == []
    assert "basic group" in result.lower()


def test_the_one_way_doors_say_so():
    from telegram_mcp.runtime import mcp

    for name in ("delete_channel", "convert_to_gigagroup"):
        tool = mcp._tool_manager.get_tool(name)
        assert tool.annotations.destructive_hint is True
        assert tool.annotations.read_only_hint is False
        assert "cannot be undone" in tool.description
    for name in ("set_channel_location", "set_autotranslation"):
        assert mcp._tool_manager.get_tool(name).annotations.destructive_hint is False
