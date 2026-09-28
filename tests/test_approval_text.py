"""The approval request reads without decoding (spec 018 US1).

The owner saw a bare "-" in a quote and a raw code "first_message": every line now says
what it is, the account acting is named, the reason is a sentence, and what will be sent
is previewed.
"""

import asyncio
from types import SimpleNamespace

from telegram_mcp.safeguard import channels as ac
from telegram_mcp.safeguard import middleware, wiring


def _request(**overrides):
    fields = dict(
        tool="send_message",
        account=None,
        chat="BotFather",
        effect="send message in BotFather",
        reasons=["first_message"],
        identity="refx · 111 · @owner",
        preview="/newbot",
    )
    fields.update(overrides)
    return ac.new_request(**fields)


def test_every_line_is_labelled():
    lines = _request().text().splitlines()
    assert [line.split(":")[0] for line in lines] == [
        "Account",
        "Action",
        "Tool",
        "Chat",  # its own line, in full (owner, 2026-09-28)
        "Sends",
        "Why asked",
    ]
    html = _request().html()
    for label in ("Account:", "Action:", "Tool:", "Chat:", "Sends:", "Why asked:"):
        assert label in html


def test_the_reason_is_a_sentence_not_a_code():
    text = _request(reasons=["first_message", "tainted", "made_up"]).text()
    assert "first_message" not in text
    assert "never messaged" in text and "another chat" in text
    assert "made_up" in text  # an unknown code is still shown rather than dropped


def test_no_account_is_said_in_words_not_a_dash():
    text = _request(identity="").text()
    assert "Account: not named in the call" in text
    assert "<blockquote>-</blockquote>" not in _request(identity="").html()


def test_nothing_sent_means_no_sends_line():
    assert "Sends:" not in _request(preview="").text()
    assert "Sends:" not in _request(preview="").html()


def test_the_preview_is_escaped_in_html():
    html = _request(preview="<b>hi</b> & bye").html()
    assert "&lt;b&gt;hi&lt;/b&gt; &amp; bye" in html


def test_preview_takes_the_text_and_the_file_name():
    shown = middleware.preview({"message": "hello there"})
    assert shown == "hello there"
    shown = middleware.preview({"file_path": r"C:\files\outbox\logo.png", "caption": "Our logo"})
    assert shown == "Our logo; file logo.png"
    shown = middleware.preview({"question": "Tea?", "options": ["Yes", "No"]})
    assert shown == "Tea?; options: Yes | No"


def test_preview_is_cut_at_300_characters():
    shown = middleware.preview({"message": "x" * 1000})
    assert len(shown) == 300 and shown.endswith("…")


def test_preview_is_empty_when_nothing_is_sent():
    assert middleware.preview({"chat_id": 5, "user_id": 9}) == ""


def test_no_account_argument_names_the_only_account(monkeypatch):
    from telegram_mcp import connection

    monkeypatch.setattr(connection, "clients", {"refx": object()})
    monkeypatch.setattr(connection, "refresh_accounts", lambda: None)
    monkeypatch.setattr(wiring, "_identities", {})

    async def _me(account):
        assert account == "refx"
        return SimpleNamespace(id=111, username="owner", usernames=None)

    monkeypatch.setattr(wiring, "_me", _me)
    assert asyncio.run(wiring.identity(None)) == "refx · 111 · @owner"


def test_no_account_with_several_accounts_stays_unnamed(monkeypatch):
    from telegram_mcp import connection

    monkeypatch.setattr(connection, "clients", {"a": object(), "b": object()})
    monkeypatch.setattr(connection, "refresh_accounts", lambda: None)
    assert asyncio.run(wiring.identity(None)) == ""


# --- the Chat line names the chat like the Account line names the account ---------


def _patch_resolve(monkeypatch, entity):
    from telegram_mcp import connection, runtime

    async def _resolve(chat, client=None, account=None):
        if isinstance(entity, Exception):
            raise entity
        return entity

    monkeypatch.setattr(runtime, "resolve_entity", _resolve)
    monkeypatch.setattr(connection, "get_client", lambda account=None: object())
    monkeypatch.setattr(wiring, "_chat_labels", {})


def test_a_chat_with_a_username_is_named_like_an_account(monkeypatch):
    from telethon.tl import types as tl

    channel = tl.Channel(
        id=3768657233,
        title="MCP topic test",
        photo=tl.ChatPhotoEmpty(),
        date=None,
        megagroup=True,
        access_hash=1,
        username="mcp_topic_test",
    )
    _patch_resolve(monkeypatch, channel)
    label = asyncio.run(wiring.chat_label("refx", -1003768657233))
    assert label == "MCP topic test · -1003768657233 · @mcp_topic_test"


def test_a_chat_without_a_username_still_gets_its_title(monkeypatch):
    from telethon.tl import types as tl

    user = tl.User(id=93372553, first_name="BotFather", bot=True, access_hash=2)
    _patch_resolve(monkeypatch, user)
    assert asyncio.run(wiring.chat_label("refx", 93372553)) == "BotFather · 93372553"


def test_a_chat_that_cannot_be_resolved_is_shown_as_given(monkeypatch):
    _patch_resolve(monkeypatch, ValueError("unknown"))
    assert asyncio.run(wiring.chat_label("refx", "-100555")) == "-100555"


def test_the_approval_request_carries_the_chat_label():
    from telegram_mcp import safeguard

    seen = []

    class _Channel:
        kind = "dialog"

        def available(self):
            return True

        async def ask(self, request, timeout):
            seen.append(request)
            return "approved_once"

    async def _label(account, chat):
        return f"Group · {chat} · @group"

    async def _nothing(*args):
        return None

    guard = safeguard.Safeguard(
        hints=lambda name: (False, True),
        channels=lambda ctx, account: [_Channel()],
        first_message=lambda account, chat: _nothing(),
        ghost_on=lambda account, chat: False,
        approval_chats=lambda: frozenset(),
        account_of=lambda arguments: "main",
        after=lambda account: None,
        identity=lambda account: _nothing(),
        chat_label=_label,
        sealed_target=lambda account, arguments: _nothing(),
        warm=_nothing,
        timeout=5,
    )
    ctx = SimpleNamespace(
        method="tools/call",
        request_id=1,
        params={"name": "delete_message", "arguments": {"chat_id": -100777, "message_id": 3}},
    )

    async def call_next(_ctx):
        return "RESULT"

    asyncio.run(guard(ctx, call_next))
    assert seen[0].chat == "Group · -100777 · @group"
