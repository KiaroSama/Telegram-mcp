"""Username <-> id for users, bots, groups and channels (spec 020).

Every spelling of a username resolves; an id comes back with every username the peer has,
collectible ones included; an id the account has never seen is explained, not dumped.
"""

import json
from datetime import datetime, timezone

import pytest
from telethon.tl import types as tl

from telegram_mcp.tools import peer_lookup as mod

WHEN = datetime(2026, 9, 28, tzinfo=timezone.utc)
CHANNEL = tl.Channel(
    id=2092185941,
    title="Relay",
    photo=tl.ChatPhotoEmpty(),
    date=WHEN,
    broadcast=True,
    access_hash=1,
    usernames=[tl.Username("relay_new", active=True), tl.Username("relay_old")],
)
BOT = tl.User(id=93372553, first_name="BotFather", bot=True, username="BotFather", access_hash=2)


@pytest.fixture
def seen(monkeypatch):
    asked = []

    def use(entity):
        async def _resolve(value, cl=None, account=None):
            asked.append(value)
            if isinstance(entity, Exception):
                raise entity
            return entity

        async def _connected(cl=None):
            return None

        monkeypatch.setattr(mod, "get_client", lambda account=None: object())
        monkeypatch.setattr(mod, "resolve_entity", _resolve)
        monkeypatch.setattr(mod, "ensure_connected", _connected)
        return asked

    return use


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "query", ["@BotFather", "BotFather", "https://t.me/BotFather", "t.me/BotFather"]
)
async def test_every_spelling_of_a_username_resolves(seen, query):
    asked = seen(BOT)
    (row,) = json.loads(await mod.lookup_peer(query))["results"]
    assert asked == ["BotFather"]
    assert row["id"] == 93372553 and row["type"] == "bot"
    assert row["usernames"] == [{"username": "BotFather", "active": True}]
    assert row["link"] == "https://t.me/BotFather"


@pytest.mark.asyncio
@pytest.mark.parametrize("query", ["-1002092185941", -1002092185941])
async def test_an_id_gives_back_every_username(seen, query):
    asked = seen(CHANNEL)
    (row,) = json.loads(await mod.lookup_peer(query))["results"]
    assert asked == [-1002092185941]
    assert row["id"] == -1002092185941 and row["bare_id"] == 2092185941
    assert row["type"] == "channel" and row["name"] == "Relay"
    assert row["usernames"] == [
        {"username": "relay_new", "active": True},
        {"username": "relay_old", "active": False},
    ]
    assert row["link"] == "https://t.me/relay_new"


@pytest.mark.asyncio
async def test_an_id_the_account_never_saw_is_explained(seen):
    seen(ValueError("Could not find the input entity for PeerUser(user_id=5)"))
    text = await mod.lookup_peer("5")
    assert "never seen" in text and "Traceback" not in text


@pytest.mark.asyncio
async def test_a_private_invite_link_is_not_a_username(seen):
    asked = seen(BOT)
    assert "invite link" in await mod.lookup_peer("https://t.me/+abcDEF")
    assert asked == []
