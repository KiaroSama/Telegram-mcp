"""Keyboards built from Telethon's OWN classes, not from fakes named after them.

The rest of the button suite builds `SimpleNamespace` subclasses whose class NAME
drives the description. That is what let Telethon 1.45 rearrange the button model
underneath this project in silence: the fakes went on being called
``KeyboardButtonCallback`` with a flat ``data`` attribute, while a real inline
keyboard became ``KeyboardInlineButton(text, type=InlineButtonTypeCallback(data=...))``
— the fields moved onto a nested ``type`` object and every old class name stopped
existing. Every button on every real message then described as
``kind: unknown, pressable: false, "Not a callback button."``.

So this file imports the real classes and lets an import error be the alarm. It
covers only the shape; behaviour lives in `test_buttons.py` and
`test_button_pressing.py`.
"""

import json
from types import SimpleNamespace

import pytest
from telethon.tl.types import (
    ButtonTypeDefault,
    ButtonTypeRequestPeer,
    ButtonTypeRequestPhone,
    InlineButtonTypeCallback,
    InlineButtonTypeCopy,
    InlineButtonTypeDisabled,
    InlineButtonTypeSwitchInline,
    InlineButtonTypeUrl,
    InlineButtonTypeUserProfile,
    InlineButtonTypeWebView,
    KeyboardButton,
    KeyboardButtonRow,
    KeyboardButtonStyle,
    KeyboardInlineButton,
    KeyboardInlineButtonRow,
    ReplyInlineMarkup,
    ReplyKeyboardMarkup,
    RequestPeerTypeUser,
)

from helpers_buttons import _inspect, _tokens_of, make_wire
from telegram_mcp.button_view import describe_keyboard
from telegram_mcp.tools.buttons import click_button


@pytest.fixture
def _wire(monkeypatch):
    return make_wire(monkeypatch)


def _glass(*buttons):
    """A message carrying a real inline ("glass") keyboard, one row."""
    markup = ReplyInlineMarkup(rows=[KeyboardInlineButtonRow(buttons=list(buttons))])
    return SimpleNamespace(id=7, reply_markup=markup)


def _described(*buttons):
    return describe_keyboard(_glass(*buttons))["buttons"]


def test_a_real_callback_button_is_a_callback_button():
    """The defect verbatim: a force-join "I joined" button, reported unknown."""
    described = _described(
        KeyboardInlineButton(text="عضو شدم", type=InlineButtonTypeCallback(data=b"joined"))
    )

    assert described[0]["kind"] == "callback"
    assert described[0]["pressable"] is True
    assert described[0]["has_callback_data"] is True
    assert "press_note" not in described[0]


def test_every_real_inline_button_type_is_classified():
    """A kind table keyed on a class name is only as good as the names in it."""
    described = _described(
        KeyboardInlineButton(text="Go", type=InlineButtonTypeUrl(url="https://e.example")),
        KeyboardInlineButton(text="App", type=InlineButtonTypeWebView(url="https://app.example")),
        KeyboardInlineButton(text="Who", type=InlineButtonTypeUserProfile(user_id=11)),
        KeyboardInlineButton(text="Copy", type=InlineButtonTypeCopy(copy_text="take me")),
        KeyboardInlineButton(text="Find", type=InlineButtonTypeSwitchInline(query="q")),
        KeyboardInlineButton(text="Off", type=InlineButtonTypeDisabled()),
    )

    assert [b["kind"] for b in described] == [
        "url",
        "webview",
        "user_profile",
        "copy",
        "switch_inline",
        "disabled",
    ]
    assert not any(b["pressable"] for b in described)
    assert all(b["press_note"] for b in described)


def test_the_fields_a_caller_reads_come_off_the_nested_type():
    """`url`, `copy_text`, `query` and `user_id` all moved onto ``type``."""
    described = _described(
        KeyboardInlineButton(text="Go", type=InlineButtonTypeUrl(url="https://e.example/x")),
        KeyboardInlineButton(text="Copy", type=InlineButtonTypeCopy(copy_text="take me")),
        KeyboardInlineButton(text="Find", type=InlineButtonTypeSwitchInline(query="q")),
        KeyboardInlineButton(text="Who", type=InlineButtonTypeUserProfile(user_id=11)),
    )

    assert described[0]["url"] == "https://e.example/x"
    assert described[1]["copy_text"] == "take me"
    assert described[2]["query"] == "q"
    assert described[3]["user_id"] == 11


def test_a_password_gated_callback_is_reported_as_ungated_by_nothing():
    described = _described(
        KeyboardInlineButton(
            text="Transfer", type=InlineButtonTypeCallback(data=b"x", requires_password=True)
        )
    )

    assert described[0]["kind"] == "callback"
    assert described[0]["requires_password"] is True
    assert described[0]["pressable"] is False


def test_a_real_style_still_reports_its_background_and_icon():
    described = _described(
        KeyboardInlineButton(
            text="Pay",
            type=InlineButtonTypeCallback(data=b"pay"),
            style=KeyboardButtonStyle(bg_danger=True, icon=5),
        )
    )

    assert described[0]["style"]["background"] == "danger"
    assert described[0]["style"]["icon_document_id"] == 5


def test_a_real_reply_keyboard_is_still_not_glass():
    """``ReplyKeyboardMarkup`` rows carry ``KeyboardButton``, with ``ButtonType*``."""
    markup = ReplyKeyboardMarkup(
        rows=[
            KeyboardButtonRow(
                buttons=[
                    KeyboardButton(text="Say it", type=ButtonTypeDefault()),
                    KeyboardButton(text="My number", type=ButtonTypeRequestPhone()),
                    KeyboardButton(
                        text="Pick",
                        type=ButtonTypeRequestPeer(
                            button_id=1, peer_type=RequestPeerTypeUser(), max_quantity=1
                        ),
                    ),
                ]
            )
        ]
    )
    keyboard = describe_keyboard(SimpleNamespace(id=7, reply_markup=markup))

    assert keyboard["is_glass"] is False
    assert [b["kind"] for b in keyboard["buttons"]] == ["plain", "request_phone", "request_peer"]
    # A TLObject copied as itself reached json.dumps and failed the whole listing.
    assert keyboard["buttons"][2]["peer_type"] == "RequestPeerTypeUser"


@pytest.mark.asyncio
async def test_a_real_callback_button_can_actually_be_pressed(_wire):
    """End to end: the payload the press sends also lives on ``type`` now."""
    client = _wire(
        _glass(
            KeyboardInlineButton(text="عضو شدم", type=InlineButtonTypeCallback(data=b"joined"))
        ),
        answer=SimpleNamespace(message="Welcome", alert=None, url=None),
    )
    token = _tokens_of(await _inspect())[0]

    result = json.loads(
        await click_button(1, 7, 0, expect_text="عضو شدم", press_token=token, account="default")
    )

    assert result["results"][0]["bot_message"] == "Welcome"
    assert client.calls[-1].data == b"joined"
