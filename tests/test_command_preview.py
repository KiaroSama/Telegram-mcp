"""The "/" command preview, and the three things a caller must not have to guess.

The preview is a property of the chat, so every case here drives a whole
full-chat reply rather than a message. The doubles carry Telethon's real shapes -
`BotInfo`, `BotCommand`, `BotMenuButton*` - because a look-alike is how a model
change slips through a green suite.
"""

import json

import pytest
from telethon.tl.types import (
    BotCommand,
    BotInfo,
    BotMenuButton,
    BotMenuButtonDefault,
    Channel,
    User,
)

import telegram_mcp.tools.command_preview as preview
from telegram_mcp.tools.command_preview import list_chat_commands


def _bot_info(user_id, commands=(), menu_button=None):
    return BotInfo(
        user_id=user_id,
        description=None,
        commands=[BotCommand(command=c, description=d) for c, d in commands],
        menu_button=menu_button,
    )


def _user(user_id, username, bot=True):
    return User(id=user_id, bot=bot, username=username)


class _FullChat:
    """`channels.getFullChannel` / `messages.getFullChat` reply shape: the full
    object beside the users it names."""

    def __init__(self, infos, users):
        self.full_chat = type("Full", (), {"bot_info": infos})()
        self.users = users


class _FullUser:
    """`users.getFullUser` carries ONE bot_info, not a vector."""

    def __init__(self, info, users):
        self.full_user = type("Full", (), {"bot_info": info})()
        self.users = users


@pytest.fixture
def wire(monkeypatch):
    """Point the tool at a chat and a canned full-chat reply."""

    def use(entity, reply):
        class _Client:
            async def __call__(self, request):
                return reply

        client = _Client()
        monkeypatch.setattr(preview, "get_client", lambda account=None: client)
        monkeypatch.setattr(preview, "ensure_connected", _noop)
        monkeypatch.setattr(preview, "resolve_entity", _resolves(entity))
        return client

    async def _noop(cl):
        return None

    def _resolves(entity):
        async def resolve(chat_id, cl):
            return entity

        return resolve

    return use


def _payload(text):
    return json.loads(text)


# --- the ready-to-send form ---------------------------------------------------


@pytest.mark.asyncio
async def test_a_group_command_is_published_qualified(wire):
    """The whole point of `send_as_text`. In a group Telegram writes
    `/status@AdTimerBot`, and a caller that rebuilds it puts the `@bot` on the
    wrong one exactly when two bots share a command name."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat(
            [_bot_info(11, [("status", "bot state")])],
            [_user(11, "AdTimerBot")],
        ),
    )

    payload = _payload(await list_chat_commands(-100007, account="default"))

    assert [e["send_as_text"] for e in payload["results"]] == ["/status@AdTimerBot"]
    assert payload["chat_scope"] == "group"


@pytest.mark.asyncio
async def test_a_private_command_is_published_bare(wire):
    """A private chat has one possible recipient, and Telegram writes `/status`."""
    wire(
        _user(11, "AdTimerBot"),
        _FullUser(_bot_info(11, [("status", "bot state")]), [_user(11, "AdTimerBot")]),
    )

    payload = _payload(await list_chat_commands(11, account="default"))

    assert [e["send_as_text"] for e in payload["results"]] == ["/status"]
    assert payload["chat_scope"] == "private"


@pytest.mark.asyncio
async def test_two_bots_sharing_a_command_name_stay_distinguishable(wire):
    """The case the qualified form exists for."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat(
            [_bot_info(11, [("help", "one")]), _bot_info(22, [("help", "two")])],
            [_user(11, "FirstBot"), _user(22, "SecondBot")],
        ),
    )

    payload = _payload(await list_chat_commands(-100007, account="default"))

    assert sorted(e["send_as_text"] for e in payload["results"]) == [
        "/help@FirstBot",
        "/help@SecondBot",
    ]


# --- the menu button ----------------------------------------------------------


@pytest.mark.asyncio
async def test_a_bot_with_no_commands_but_a_menu_button_is_still_reported(wire):
    """Its button is then its only entry point; omitting it says the bot has none."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat(
            [_bot_info(11, [], menu_button=BotMenuButton(text="Open", url="https://e.example"))],
            [_user(11, "AppBot")],
        ),
    )

    payload = _payload(await list_chat_commands(-100007, account="default"))

    assert payload["results"] == []
    assert payload["bots"][0]["menu_button"]["text"] == "Open"
    assert payload["bots"][0]["bot"] == "@AppBot"


@pytest.mark.asyncio
async def test_the_default_menu_button_is_not_reported_as_one(wire):
    """`BotMenuButtonDefault` means the bot set nothing; reporting it invents a
    button the operator cannot see."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat(
            [_bot_info(11, [("go", "d")], menu_button=BotMenuButtonDefault())],
            [_user(11, "PlainBot")],
        ),
    )

    payload = _payload(await list_chat_commands(-100007, account="default"))

    assert payload["bots"][0]["menu_button"] is None


# --- no bots is not a failure -------------------------------------------------


@pytest.mark.asyncio
async def test_a_chat_with_no_bots_says_so_in_words(wire):
    """An empty list alone cannot distinguish this from a read that went wrong."""
    wire(Channel(id=7, title="G", photo=None, date=None), _FullChat([], []))

    payload = _payload(await list_chat_commands(-100007, account="default"))

    assert payload["results"] == []
    assert payload["bot_count"] == 0
    assert "No bot is present" in payload["note"]


@pytest.mark.asyncio
async def test_bots_present_but_nothing_matching_says_something_different(wire):
    """The two empty results must not read alike."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat([_bot_info(11, [("status", "d")])], [_user(11, "AdTimerBot")]),
    )

    payload = _payload(await list_chat_commands(-100007, prefix="zzz", account="default"))

    assert payload["results"] == []
    assert payload["bot_count"] == 1
    assert "No bot is present" not in payload["note"]


# --- the prefix, as typing narrows a client's own list ------------------------


@pytest.mark.asyncio
async def test_a_prefix_narrows_the_list(wire):
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat(
            [_bot_info(11, [("status", "a"), ("stop", "b"), ("help", "c")])],
            [_user(11, "AdTimerBot")],
        ),
    )

    payload = _payload(await list_chat_commands(-100007, prefix="st", account="default"))

    assert sorted(e["command"] for e in payload["results"]) == ["status", "stop"]


@pytest.mark.asyncio
async def test_a_prefix_may_carry_the_slash_the_operator_typed(wire):
    """They typed `/st`; requiring them to strip it is a trap with no upside."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat([_bot_info(11, [("status", "a"), ("help", "c")])], [_user(11, "AdTimerBot")]),
    )

    payload = _payload(await list_chat_commands(-100007, prefix="/ST", account="default"))

    assert [e["command"] for e in payload["results"]] == ["status"]


# --- a bot with no username ---------------------------------------------------


@pytest.mark.asyncio
async def test_a_bot_without_a_username_is_reported_not_guessed(wire):
    """It cannot be addressed as `@name`, so no qualified form is invented."""
    wire(
        Channel(id=7, title="G", photo=None, date=None),
        _FullChat([_bot_info(11, [("status", "d")])], [_user(11, None)]),
    )

    payload = _payload(await list_chat_commands(-100007, account="default"))

    assert payload["bots"][0]["bot"] is None
    assert payload["results"][0]["send_as_text"] == "/status"
