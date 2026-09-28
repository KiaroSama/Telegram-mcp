"""Asking an inline bot and sending what it offered, previews included (spec 021).

A person typing `@bot query` sees a list of previews - title, description, a picture, the
message each would send and its buttons - and the "switch to PM" button above them. The
agent has to see the same things to pick one, so every field comes back. A result is sent
by the opaque handle the query returned, bound to the account and to Telegram's cache time.
"""

import json
import time
from datetime import datetime, timezone

import pytest
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tools import inline_bots as mod

WHEN = datetime(2026, 9, 28, tzinfo=timezone.utc)
BOT = tl.User(id=777, bot=True, username="GodVerifyPaymentBot", access_hash=9)

TEXT_RESULT = tl.BotInlineResult(
    id="note-5",
    type="article",
    title="Note #5",
    description="Paid 12 USDT",
    url="https://example.org/n/5",
    thumb=tl.WebDocumentNoProxy(
        url="https://example.org/t.png", size=900, mime_type="image/png", attributes=[]
    ),
    send_message=tl.BotInlineMessageText(
        message="Invoice 5 is paid",
        entities=[tl.MessageEntityBold(offset=0, length=7)],
        reply_markup=tl.ReplyInlineMarkup(
            rows=[
                tl.KeyboardButtonRow(
                    buttons=[
                        tl.KeyboardButton(
                            text="Open",
                            type=tl.InlineButtonTypeUrl(url="https://t.me/GodVerifyPaymentBot"),
                        ),
                        tl.KeyboardButton(
                            text="Refund", type=tl.InlineButtonTypeCallback(data=b"r5")
                        ),
                    ]
                )
            ]
        ),
    ),
)
PHOTO_RESULT = tl.BotInlineMediaResult(
    id="pic",
    type="photo",
    photo=tl.Photo(id=55, access_hash=1, file_reference=b"", date=WHEN, sizes=[], dc_id=2),
    send_message=tl.BotInlineMessageMediaAuto(message="Receipt"),
)
ANSWER = tl.messages.BotResults(
    query_id=4242,
    results=[TEXT_RESULT, PHOTO_RESULT],
    cache_time=300,
    users=[BOT],
    next_offset="2",
    switch_pm=tl.InlineBotSwitchPM(text="Open the admin panel", start_param="admin"),
)


class _Client:
    def __init__(self, answers):
        self.answers = answers
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        return self.answers.get(type(request).__name__)

    async def get_input_entity(self, value):
        return tl.InputPeerUser(777, 9)


@pytest.fixture
def wire(monkeypatch):
    def use(answers=None):
        client = _Client(answers or {})

        async def _resolve(value, cl=None, account=None):
            return BOT if str(value).lstrip("@").lower() == "godverifypaymentbot" else value

        async def _connected(cl=None):
            return None

        monkeypatch.setattr(mod, "get_client", lambda account=None: client)
        monkeypatch.setattr(mod, "resolve_entity", _resolve)
        monkeypatch.setattr(mod, "ensure_connected", _connected)
        return client

    return use


@pytest.mark.asyncio
async def test_every_preview_field_comes_back(wire):
    c = wire({"GetInlineBotResultsRequest": ANSWER})
    payload = json.loads(await mod.inline_query("@GodVerifyPaymentBot", "5 note", account="refx"))
    text, photo = payload["results"]
    assert text["type"] == "article" and text["title"] == "Note #5"
    assert text["description"] == "Paid 12 USDT" and text["url"] == "https://example.org/n/5"
    assert text["thumb"] == {
        "url": "https://example.org/t.png",
        "mime_type": "image/png",
        "size": 900,
    }
    message = text["message"]
    assert message["kind"] == "text" and message["text"] == "Invoice 5 is paid"
    assert message["entities"][0]["type"] == "bold"
    assert message["buttons"] == [
        {"text": "Open", "kind": "url", "url": "https://t.me/GodVerifyPaymentBot"},
        {"text": "Refund", "kind": "callback"},
    ]
    assert photo["photo"]["id"] == 55 and photo["message"]["kind"] == "media_auto"
    assert payload["switch_pm"] == {"text": "Open the admin panel", "start_param": "admin"}
    assert payload["next_offset"] == "2" and payload["returned"] == 2
    (request,) = [
        r for r in c.sent if isinstance(r, functions.messages.GetInlineBotResultsRequest)
    ]
    assert request.query == "5 note"


@pytest.mark.asyncio
async def test_a_result_is_sent_by_its_handle(wire):
    c = wire({"GetInlineBotResultsRequest": ANSWER})
    payload = json.loads(await mod.inline_query("GodVerifyPaymentBot", "5 note", account="refx"))
    handle = payload["results"][0]["result_id"]

    sent = tl.Updates(
        updates=[tl.UpdateMessageID(id=900, random_id=1)], users=[], chats=[], date=WHEN, seq=0
    )
    c = wire({"SendInlineBotResultRequest": sent})
    reply = await mod.send_inline_result("me", handle, account="refx")
    (request,) = [
        r for r in c.sent if isinstance(r, functions.messages.SendInlineBotResultRequest)
    ]
    assert request.query_id == 4242 and request.id == "note-5"
    assert "900" in reply


@pytest.mark.asyncio
async def test_a_handle_from_another_account_or_expired_is_refused(wire):
    c = wire()
    handle = mod._handle("refx", int(time.time()) + 60, 4242, "note-5")
    assert "another account" in await mod.send_inline_result("me", handle, account="kgb")
    expired = mod._handle("refx", int(time.time()) - 1, 4242, "note-5")
    assert "expired" in await mod.send_inline_result("me", expired, account="refx")
    assert "inline_query" in await mod.send_inline_result("me", "garbage", account="refx")
    assert c.sent == []


@pytest.mark.asyncio
async def test_only_a_bot_can_be_queried(wire, monkeypatch):
    c = wire()

    async def _person(value, cl=None, account=None):
        return tl.User(id=5, first_name="Ada", access_hash=1)

    monkeypatch.setattr(mod, "resolve_entity", _person)
    assert "not a bot" in await mod.inline_query("ada", "x", account="refx")
    assert c.sent == []
