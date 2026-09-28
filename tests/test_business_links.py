"""Business chat links: t.me/m/<slug> links that open a chat with prepared text (plan 018)."""

import pytest
from telethon.errors import RPCError
from telethon.tl import functions, types

from telegram_mcp.tools import business_links as mod


class Recorder:
    def __init__(self, answer=True):
        self.sent = []
        self.answer = answer

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(self.answer, Exception):
            raise self.answer
        return self.answer


LINK = types.BusinessChatLink(
    link="https://t.me/m/AbCdEf123", message="Hello​, I want to order", views=4, title="Orders"
)


@pytest.mark.asyncio
async def test_list_describes_each_link_with_its_slug(wire_client):
    client = Recorder(types.account.BusinessChatLinks(links=[LINK], chats=[], users=[]))
    wire_client(mod, client)

    result = await mod.list_business_chat_links()

    assert isinstance(client.sent[0], functions.account.GetBusinessChatLinksRequest)
    assert '"slug": "AbCdEf123"' in result
    assert '"message": "Hello, I want to order"' in result, "prepared text is sanitized"
    assert '"views": 4' in result and '"title": "Orders"' in result


@pytest.mark.asyncio
async def test_create_sends_text_and_title(wire_client):
    client = Recorder(LINK)
    wire_client(mod, client)

    result = await mod.create_business_chat_link("I want to order", "Orders")

    request = client.sent[0]
    assert isinstance(request, functions.account.CreateBusinessChatLinkRequest)
    assert request.link == types.InputBusinessChatLink(
        message="I want to order", entities=None, title="Orders"
    )
    assert "AbCdEf123" in result


@pytest.mark.asyncio
async def test_edit_accepts_a_pasted_link_as_the_slug(wire_client):
    client = Recorder(LINK)
    wire_client(mod, client)

    await mod.edit_business_chat_link("https://t.me/m/AbCdEf123", "New text", None)

    request = client.sent[0]
    assert isinstance(request, functions.account.EditBusinessChatLinkRequest)
    assert request.slug == "AbCdEf123"
    assert request.link.message == "New text" and request.link.title is None


@pytest.mark.asyncio
async def test_delete_sends_its_request(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.delete_business_chat_link("AbCdEf123")

    assert isinstance(client.sent[0], functions.account.DeleteBusinessChatLinkRequest)
    assert client.sent[0].slug == "AbCdEf123"


@pytest.mark.asyncio
async def test_resolve_names_the_peer_and_the_text(wire_client):
    user = types.User(id=42, first_name="Shop", username="shop")
    client = Recorder(
        types.account.ResolvedBusinessChatLinks(
            peer=types.PeerUser(user_id=42), message="Hi", chats=[], users=[user]
        )
    )
    wire_client(mod, client)

    result = await mod.resolve_business_chat_link("t.me/m/AbCdEf123")

    assert client.sent[0].slug == "AbCdEf123"
    assert '"peer_id": 42' in result and '"name": "Shop"' in result
    assert '"message": "Hi"' in result


@pytest.mark.parametrize(
    "call",
    [
        lambda: mod.create_business_chat_link("  "),
        lambda: mod.edit_business_chat_link("", "x"),
        lambda: mod.delete_business_chat_link("https://t.me/m/"),
        lambda: mod.resolve_business_chat_link(""),
    ],
)
@pytest.mark.asyncio
async def test_missing_text_or_slug_is_refused_before_any_request(wire_client, call):
    client = Recorder()
    wire_client(mod, client)

    result = await call()

    assert client.sent == []
    assert result


@pytest.mark.asyncio
async def test_a_premium_refusal_is_a_sentence(wire_client):
    client = Recorder(RPCError(request=None, message="PREMIUM_ACCOUNT_REQUIRED", code=403))
    wire_client(mod, client)

    result = await mod.create_business_chat_link("Hi")

    assert "Telegram Premium" in result


def test_hints():
    from telegram_mcp.runtime import mcp

    get = mcp._tool_manager.get_tool
    assert get("list_business_chat_links").annotations.read_only_hint is True
    assert get("delete_business_chat_link").annotations.destructive_hint is True
    for name in ("create_business_chat_link", "edit_business_chat_link"):
        assert get(name).annotations.destructive_hint is False
    # Resolving counts a view on the link's owner side: not a pure read.
    resolve = get("resolve_business_chat_link").annotations
    assert resolve.read_only_hint is False and resolve.destructive_hint is False
