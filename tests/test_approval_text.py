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
