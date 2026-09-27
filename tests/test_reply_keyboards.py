"""Reply keyboards (spec 011): which one a chat shows, and pressing its buttons.

A reply keyboard is set by one message and stays on screen while newer messages
arrive, so "the latest message's keyboard" is the wrong question - live, the
keyboard the owner was looking at came from a message thousands of ids back.
These pin Telegram Desktop's rule for the active keyboard, that a press sends
the button's OWN label and nothing else, and that every sensitive button goes
through the tool that asks the owner first.
"""

import json
from types import SimpleNamespace

import pytest
from telethon.errors import RPCError
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.button_view import active_keyboard_message
from telegram_mcp.safeguard.policy import categorize
from telegram_mcp.tools import reply_keyboard as mod
from helpers_buttons import _button, make_wire
from telegram_mcp.tools.buttons import inspect_buttons

BOT = tl.User(id=42, first_name="Bot", bot=True, access_hash=4)


def _reply_markup(*labels, single_use=None, selective=None, kinds=None):
    kinds = kinds or {}
    buttons = [
        tl.KeyboardButton(text=label, type=kinds.get(label, tl.ButtonTypeDefault()))
        for label in labels
    ]
    return tl.ReplyKeyboardMarkup(
        rows=[tl.KeyboardButtonRow(buttons=buttons)], single_use=single_use, selective=selective
    )


def _msg(message_id, markup=None, out=False, mentioned=False, text=""):
    return SimpleNamespace(
        id=message_id,
        out=out,
        mentioned=mentioned,
        reply_markup=markup,
        message=text,
        sender_id=None if out else BOT.id,
    )


# --- US1: which keyboard is active ---------------------------------------------------


def test_the_newest_incoming_keyboard_wins():
    older = _msg(10, _reply_markup("Old"))
    newer = _msg(20, _reply_markup("New"))
    state, msg = active_keyboard_message([_msg(30), newer, older])
    assert state == "shown" and msg is newer


def test_a_later_hide_clears_it():
    state, msg = active_keyboard_message(
        [_msg(30, tl.ReplyKeyboardHide()), _msg(20, _reply_markup("Menu"))]
    )
    assert state == "hidden" and msg.id == 30


def test_the_owners_own_messages_never_decide():
    state, msg = active_keyboard_message(
        [_msg(30, _reply_markup("Mine"), out=True), _msg(20, _reply_markup("Bot's"))]
    )
    assert msg.id == 20


def test_a_selective_keyboard_counts_only_when_it_addresses_the_owner():
    others = _msg(30, _reply_markup("Not for you", selective=True))
    mine = _msg(20, _reply_markup("For you", selective=True), mentioned=True)
    assert active_keyboard_message([others, mine])[1] is mine


def test_force_reply_is_reported_with_no_buttons():
    state, msg = active_keyboard_message([_msg(30, tl.ReplyKeyboardForceReply())])
    assert state == "force_reply" and msg.id == 30


def test_glass_buttons_do_not_decide_the_reply_keyboard():
    glass = _msg(30, tl.ReplyInlineMarkup(rows=[]))
    assert active_keyboard_message([glass, _msg(20, _reply_markup("Menu"))])[1].id == 20


def test_nothing_found_is_none():
    assert active_keyboard_message([_msg(3), _msg(2)]) == (None, None)


@pytest.fixture
def wire(monkeypatch):
    return make_wire(monkeypatch)


class _History:
    """The chat as a newest-first list; `limit` queries answer from it."""

    def __init__(self, messages):
        self.messages = messages

    async def get_messages(self, entity, ids=None, limit=None):
        if limit is not None:
            return self.messages[:limit]
        return next((m for m in self.messages if m.id == ids), None)

    async def __call__(self, request):
        return []


def _use_history(monkeypatch, messages):
    import telegram_mcp.tools.buttons as buttons_tool

    client = _History(messages)
    monkeypatch.setattr(buttons_tool, "get_client", lambda account=None: client)

    async def _noop(*a, **k):
        return SimpleNamespace(id=1)

    monkeypatch.setattr(buttons_tool, "ensure_connected", _noop)
    monkeypatch.setattr(buttons_tool, "resolve_entity", _noop)
    return client


@pytest.mark.asyncio
async def test_listing_without_a_message_reports_the_active_keyboard(monkeypatch):
    glass = _msg(
        30,
        tl.ReplyInlineMarkup(
            rows=[
                tl.KeyboardInlineButtonRow(
                    buttons=[_button("InlineButtonTypeCallback", text="Back", data=b"b")]
                )
            ]
        ),
    )
    _use_history(monkeypatch, [glass, _msg(25), _msg(20, _reply_markup("📒 راهنما", "🧹 ریست"))])

    payload = json.loads(await inspect_buttons(1, account="default"))

    assert payload["message_id"] == 30 and payload["results"][0]["text"] == "Back"
    active = payload["active_reply_keyboard"]
    assert active["state"] == "shown" and active["message_id"] == 20
    assert [b["text"] for b in active["buttons"]] == ["📒 راهنما", "🧹 ریست"]
    assert active["buttons"][0]["press_with"] == "press_reply_button"


@pytest.mark.asyncio
async def test_a_latest_message_without_keyboard_still_shows_the_active_one(monkeypatch):
    _use_history(monkeypatch, [_msg(30, text="hi"), _msg(20, _reply_markup("Menu"))])
    payload = json.loads(await inspect_buttons(1, account="default"))
    assert payload["results"] == []
    assert payload["active_reply_keyboard"]["message_id"] == 20


@pytest.mark.asyncio
async def test_no_active_keyboard_states_how_far_it_looked(monkeypatch):
    _use_history(monkeypatch, [_msg(30, text="hi")])
    text = await inspect_buttons(1, account="default")
    assert "100" in text


@pytest.mark.asyncio
async def test_naming_a_message_reads_only_that_message(wire):
    client = wire(_msg(7, _reply_markup("Menu")))
    payload = json.loads(await inspect_buttons(1, 7, account="default"))
    assert "active_reply_keyboard" not in payload
    assert client.calls == []


# --- US2 / US3: pressing -------------------------------------------------------------


class _Chat:
    """A bot chat: history (newest first), what the bot answers, what was sent."""

    def __init__(self, history, replies=()):
        self.history = list(history)
        self.replies = list(replies)
        self.sent, self.calls = [], []

    async def get_messages(self, entity, limit=None, min_id=None, ids=None):
        if min_id is not None:
            return [m for m in self.replies if m.id > min_id]
        return self.history[:limit]

    async def send_message(self, entity, text):
        self.sent.append(text)
        return SimpleNamespace(id=900, out=True)

    async def get_me(self):
        return tl.User(id=1, first_name="Owner", last_name=None, phone="15550001111")

    async def get_input_entity(self, peer):
        return tl.InputPeerUser(user_id=getattr(peer, "id", peer), access_hash=0)

    async def __call__(self, request):
        self.calls.append(request)
        if isinstance(request, functions.messages.RequestSimpleWebViewRequest):
            return tl.WebViewResultUrl(
                url="https://app.example/launch", fullsize=False, fullscreen=False, query_id=None
            )
        return tl.Updates(updates=[], users=[], chats=[], date=None, seq=0)


class _Refusing(_Chat):
    async def __call__(self, request):
        raise RPCError(request, "GEO_POINT_INVALID", 400)


GROUP = tl.Channel(
    id=77, title="G", photo=tl.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=7
)
NEWS = tl.Channel(
    id=55, title="N", photo=tl.ChatPhotoEmpty(), date=None, broadcast=True, access_hash=5
)
FRIEND = tl.User(id=9, first_name="Friend", access_hash=9)


@pytest.fixture
def chat(monkeypatch):
    holder = {}

    def use(history, replies=()):
        holder["chat"] = _Chat(history, replies)
        return holder["chat"]

    names = {"@thebot": BOT, "@group_x": GROUP, "@news_x": NEWS, "@friend_x": FRIEND}

    async def _resolve(value, cl=None, account=None):
        return names[value]

    async def _connected(cl=None):
        return None

    monkeypatch.setattr(mod, "get_client", lambda account=None: holder["chat"])
    monkeypatch.setattr(mod, "resolve_entity", _resolve)
    monkeypatch.setattr(mod, "ensure_connected", _connected)
    return use


MENU = _msg(20, _reply_markup("📒 راهنما", "🧹 ریست"))


@pytest.mark.asyncio
async def test_a_press_sends_the_buttons_own_label_and_returns_the_answer(chat):
    answer = _msg(901, _reply_markup("Sub A", "Sub B"), text="Help menu")
    c = chat([MENU], replies=[answer])

    payload = json.loads(
        await mod.press_reply_button(
            "@thebot", button_index=0, expect_text="📒 راهنما", wait_seconds=0
        )
    )

    assert c.sent == ["📒 راهنما"]
    (reply,) = payload["results"]
    assert reply["text"] == "Help menu"
    assert [b["text"] for b in reply["keyboard"]["buttons"]] == ["Sub A", "Sub B"]


@pytest.mark.asyncio
async def test_a_changed_keyboard_sends_nothing(chat):
    c = chat([MENU])
    text = await mod.press_reply_button("@thebot", 0, expect_text="🧹 ریست", wait_seconds=0)
    assert c.sent == [] and "now reads" in text


@pytest.mark.asyncio
async def test_no_active_keyboard_sends_nothing(chat):
    c = chat([_msg(30, tl.ReplyKeyboardHide()), MENU])
    text = await mod.press_reply_button("@thebot", 0, expect_text="📒 راهنما", wait_seconds=0)
    assert c.sent == [] and "no reply keyboard" in text.lower()


@pytest.mark.asyncio
async def test_look_alike_labels_cannot_be_pressed(chat):
    twins = _msg(20, _reply_markup("Pay", "P​ay"))
    c = chat([twins])
    text = await mod.press_reply_button("@thebot", 0, expect_text="Pay", wait_seconds=0)
    assert c.sent == [] and "same label" in text


@pytest.mark.asyncio
async def test_a_sensitive_button_is_refused_and_the_other_tool_named(chat):
    phone = _msg(
        20, _reply_markup("Share phone", kinds={"Share phone": tl.ButtonTypeRequestPhone()})
    )
    c = chat([phone])
    text = await mod.press_reply_button("@thebot", 0, expect_text="Share phone", wait_seconds=0)
    assert c.sent == [] and "answer_reply_button" in text


@pytest.mark.asyncio
@pytest.mark.parametrize("wait", [-1, 31, float("nan")])
async def test_an_unusable_wait_is_refused_before_sending(chat, wait):
    c = chat([MENU])
    text = await mod.press_reply_button("@thebot", 0, expect_text="📒 راهنما", wait_seconds=wait)
    assert c.sent == [] and "wait_seconds" in text


@pytest.mark.asyncio
async def test_no_answer_in_time_is_said_plainly(chat):
    c = chat([MENU])
    payload = json.loads(
        await mod.press_reply_button("@thebot", 0, expect_text="📒 راهنما", wait_seconds=0)
    )
    assert c.sent == ["📒 راهنما"] and payload["results"] == []
    assert "no answer" in payload["no_answer"].lower()


def _kb(label, kind):
    return _msg(20, _reply_markup(label, kinds={label: kind}))


@pytest.mark.asyncio
async def test_phone_shares_the_owners_contact_without_echoing_the_number(chat):
    c = chat([_kb("Phone", tl.ButtonTypeRequestPhone())])
    text = await mod.answer_reply_button("@thebot", 0, expect_text="Phone", wait_seconds=0)
    (req,) = c.calls
    assert isinstance(req, functions.messages.SendMediaRequest)
    assert isinstance(req.media, tl.InputMediaContact)
    assert req.media.phone_number == "15550001111"
    assert "15550001111" not in text


@pytest.mark.asyncio
async def test_location_sends_the_given_point(chat):
    c = chat([_kb("Where", tl.ButtonTypeRequestGeoLocation())])
    await mod.answer_reply_button(
        "@thebot", 0, expect_text="Where", latitude=35.7, longitude=51.4, wait_seconds=0
    )
    point = c.calls[0].media.geo_point
    assert (point.lat, point.long) == (35.7, 51.4)


@pytest.mark.asyncio
@pytest.mark.parametrize("lat,lon", [(None, 51.4), (91, 0), (0, 181)])
async def test_a_missing_or_impossible_location_is_refused(chat, lat, lon):
    c = chat([_kb("Where", tl.ButtonTypeRequestGeoLocation())])
    text = await mod.answer_reply_button(
        "@thebot", 0, expect_text="Where", latitude=lat, longitude=lon, wait_seconds=0
    )
    assert c.calls == [] and "latitude" in text


def _peer_button(peer_type, max_quantity=1):
    return tl.ButtonTypeRequestPeer(button_id=5, peer_type=peer_type, max_quantity=max_quantity)


@pytest.mark.asyncio
async def test_a_chosen_group_is_shared_with_the_button_id(chat):
    c = chat([_kb("Pick", _peer_button(tl.RequestPeerTypeChat()))])
    await mod.answer_reply_button(
        "@thebot", 0, expect_text="Pick", peers=["@group_x"], wait_seconds=0
    )
    (req,) = c.calls
    assert isinstance(req, functions.messages.SendBotRequestedPeerRequest)
    assert req.button_id == 5 and req.msg_id == 20 and len(req.requested_peers) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "peers,why",
    [
        (["@news_x"], "group"),
        (["@friend_x", "@friend_x"], "at most 1"),
        ([], "peers"),
    ],
)
async def test_the_wrong_kind_or_count_of_chats_is_refused(chat, peers, why):
    c = chat([_kb("Pick", _peer_button(tl.RequestPeerTypeChat()))])
    text = await mod.answer_reply_button(
        "@thebot", 0, expect_text="Pick", peers=peers, wait_seconds=0
    )
    assert c.calls == [] and why in text


@pytest.mark.asyncio
async def test_a_button_asking_to_create_a_bot_is_refused(chat):
    create = getattr(tl, "RequestPeerTypeCreateBot", None)
    if create is None:
        pytest.skip("installed layer has no create-bot request")
    c = chat([_kb("Make", _peer_button(create()))])
    text = await mod.answer_reply_button(
        "@thebot", 0, expect_text="Make", peers=["@friend_x"], wait_seconds=0
    )
    assert c.calls == [] and "not supported" in text


@pytest.mark.asyncio
async def test_a_quiz_button_needs_a_correct_answer(chat):
    c = chat([_kb("Quiz", tl.ButtonTypeRequestPoll(quiz=True))])
    text = await mod.answer_reply_button(
        "@thebot", 0, expect_text="Quiz", question="Q?", options=["a", "b"], wait_seconds=0
    )
    assert c.calls == [] and "correct_option_index" in text


@pytest.mark.asyncio
async def test_a_regular_poll_button_refuses_a_quiz(chat):
    c = chat([_kb("Poll", tl.ButtonTypeRequestPoll(quiz=False))])
    text = await mod.answer_reply_button(
        "@thebot",
        0,
        expect_text="Poll",
        question="Q?",
        options=["a", "b"],
        correct_option_index=0,
        wait_seconds=0,
    )
    assert c.calls == [] and "not a quiz" in text


@pytest.mark.asyncio
async def test_a_mini_app_button_returns_its_address(chat):
    c = chat([_kb("Open", tl.ButtonTypeSimpleWebView(url="https://app.example"))])
    payload = json.loads(await mod.answer_reply_button("@thebot", 0, expect_text="Open"))
    (req,) = c.calls
    assert isinstance(req, functions.messages.RequestSimpleWebViewRequest)
    assert req.url == "https://app.example"
    assert payload["results"][0]["url"] == "https://app.example/launch"


@pytest.mark.asyncio
async def test_an_argument_the_button_does_not_take_is_refused(chat):
    c = chat([_kb("Phone", tl.ButtonTypeRequestPhone())])
    text = await mod.answer_reply_button(
        "@thebot", 0, expect_text="Phone", latitude=1.0, wait_seconds=0
    )
    assert c.calls == [] and "latitude" in text


@pytest.mark.asyncio
async def test_a_plain_button_points_back_to_press_reply_button(chat):
    c = chat([MENU])
    text = await mod.answer_reply_button("@thebot", 0, expect_text="📒 راهنما", wait_seconds=0)
    assert c.calls == [] and c.sent == [] and "press_reply_button" in text


@pytest.mark.asyncio
async def test_a_telegram_refusal_is_reported_by_name(chat):
    c = chat([_kb("Where", tl.ButtonTypeRequestGeoLocation())])
    c.__class__ = _Refusing
    text = await mod.answer_reply_button(
        "@thebot", 0, expect_text="Where", latitude=1.0, longitude=1.0, wait_seconds=0
    )
    assert "GEO_POINT_INVALID" in text


def test_ordinary_presses_run_like_sends_and_sensitive_ones_always_ask():
    assert categorize("press_reply_button", False, False) == "send"
    assert categorize("answer_reply_button", False, False) == "gated"


@pytest.mark.asyncio
async def test_a_poll_button_sends_the_agents_poll(chat, monkeypatch):
    chat([_kb("Poll", tl.ButtonTypeRequestPoll())])
    seen = {}

    async def fake_create_poll(**kwargs):
        seen.update(kwargs)
        return "poll sent"

    monkeypatch.setattr(mod.poll_creation, "create_poll", fake_create_poll)
    payload = json.loads(
        await mod.answer_reply_button(
            "@thebot", 0, expect_text="Poll", question="Q?", options=["a", "b"], wait_seconds=0
        )
    )
    assert seen["chat_id"] == "@thebot" and seen["options"] == ["a", "b"]
    assert seen["quiz_mode"] is False and payload["poll"] == "poll sent"


@pytest.mark.asyncio
async def test_the_bots_answer_is_remembered_by_the_safeguard(chat, monkeypatch):
    answer = _msg(901, text="Visit https://evil.example")
    chat([MENU], replies=[answer])
    noted = []
    monkeypatch.setattr(mod, "note_rendered", lambda msg, account=None: noted.append(msg))
    await mod.press_reply_button("@thebot", 0, expect_text="📒 راهنما", wait_seconds=0)
    assert noted == [answer]
