"""Colour, emoji status and sticker sets of a channel (plan 015, appearance group)."""

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import channel_appearance as mod
from test_channel_identity import BASIC_GROUP, CHANNEL, _wire


def _w(monkeypatch, entity=CHANNEL):
    return _wire(monkeypatch, entity, modules=(mod,))


@pytest.mark.asyncio
async def test_colour_carries_the_three_fields(monkeypatch):
    client = _w(monkeypatch)

    await mod.set_channel_color("@announcements", 5, 777, for_profile=True)

    request = client.sent[0]
    assert isinstance(request, functions.channels.UpdateColorRequest)
    assert request.channel is CHANNEL
    assert (request.color, request.background_emoji_id, request.for_profile) == (5, 777, True)


@pytest.mark.asyncio
async def test_colour_without_values_resets_and_names_no_profile(monkeypatch):
    client = _w(monkeypatch)

    result = await mod.set_channel_color("@announcements", None, None)

    request = client.sent[0]
    assert request.color is None and request.background_emoji_id is None
    assert not request.for_profile, "for_profile=False must leave the flag absent"
    assert "reset" in result.lower()


@pytest.mark.asyncio
async def test_colour_must_not_be_negative(monkeypatch):
    client = _w(monkeypatch)

    result = await mod.set_channel_color("@announcements", -1, None)

    assert client.sent == []
    assert "color" in result


@pytest.mark.parametrize(
    "emoji_id,expected",
    [(12345, types.EmojiStatus), (None, types.EmojiStatusEmpty)],
)
@pytest.mark.asyncio
async def test_emoji_status_set_and_cleared(monkeypatch, emoji_id, expected):
    client = _w(monkeypatch)

    await mod.set_channel_emoji_status("@announcements", emoji_id)

    request = client.sent[0]
    assert isinstance(request, functions.channels.UpdateEmojiStatusRequest)
    assert isinstance(request.emoji_status, expected)
    if emoji_id:
        assert request.emoji_status.document_id == emoji_id


@pytest.mark.parametrize(
    "tool,request_type",
    [
        ("set_channel_stickers", functions.channels.SetStickersRequest),
        ("set_channel_emoji_pack", functions.channels.SetEmojiStickersRequest),
    ],
)
@pytest.mark.asyncio
async def test_sticker_sets_by_short_name_and_cleared_by_none(monkeypatch, tool, request_type):
    client = _w(monkeypatch)

    await getattr(mod, tool)("@announcements", "https://t.me/addstickers/MyPack")
    await getattr(mod, tool)("@announcements", None)

    first, second = client.sent
    assert isinstance(first, request_type)
    assert isinstance(first.stickerset, types.InputStickerSetShortName)
    assert first.stickerset.short_name == "MyPack", "a pasted link is reduced to its name"
    assert isinstance(second.stickerset, types.InputStickerSetEmpty)


@pytest.mark.parametrize(
    "call",
    [
        lambda: mod.set_channel_color(-777, 1, None),
        lambda: mod.set_channel_emoji_status(-777, 1),
        lambda: mod.set_channel_stickers(-777, "x"),
        lambda: mod.set_channel_emoji_pack(-777, "x"),
    ],
)
@pytest.mark.asyncio
async def test_a_basic_group_gets_a_sentence(monkeypatch, call):
    client = _w(monkeypatch, BASIC_GROUP)

    result = await call()

    assert client.sent == []
    assert "basic group" in result.lower()


def test_colour_arguments_are_optional():
    """Measured live 2026-09-29: the preflight refused a colour with no background emoji."""
    import inspect

    params = inspect.signature(mod.set_channel_color).parameters
    assert params["color"].default is None and params["background_emoji_id"].default is None
