"""The safeguard in front of every tool call.

It changes nothing for a free call, holds a gated call until an approval arrives from
a channel the model cannot answer, and turns every other ending into a refusal whose
words stop an agent from retrying in a loop (contracts/safeguard-decision.md).
"""

import asyncio
from types import SimpleNamespace

import pytest

from telegram_mcp import safeguard
from telegram_mcp.safeguard import middleware, taint
from telegram_mcp.tool_budget import ToolCallBudget

HINTS = {
    "get_history": (True, False),
    "send_message": (False, True),
    "delete_message": (False, True),
    "mark_as_read": (False, True),
}


class _Channel:
    kind = "dialog"

    def __init__(self, outcome="approved_once"):
        self.outcome = outcome
        self.requests = []

    def available(self):
        return True

    async def ask(self, request, timeout):
        self.requests.append(request)
        return self.outcome


def _guard(channel=None, *, first_message=False, ghost=True, approval_chats=()):
    channel = channel or _Channel()

    async def _first(account, chat):
        return first_message

    guard = safeguard.Safeguard(
        hints=HINTS.get,
        channels=lambda ctx, account: [channel],
        first_message=_first,
        ghost_on=lambda account, chat: ghost,
        approval_chats=lambda: frozenset(approval_chats),
        account_of=lambda arguments: arguments.get("account", "main"),
        timeout=300,
    )
    return guard, channel


def _call(guard, name, arguments=None, method="tools/call"):
    ran = []

    async def call_next(ctx):
        ran.append(ctx.params["name"] if ctx.method == "tools/call" else ctx.method)
        return "RESULT"

    ctx = SimpleNamespace(
        method=method,
        params={"name": name, "arguments": arguments or {}},
        request_id=1,
        session=None,
    )
    return asyncio.run(guard(ctx, call_next)), ran


def _text(result):
    return result.content[0].text


@pytest.fixture(autouse=True)
def _fresh():
    taint.clear()
    yield
    taint.clear()


def test_other_messages_pass_untouched():
    guard, channel = _guard()
    result, ran = _call(guard, "delete_message", method="tools/list")
    assert (result, ran, channel.requests) == ("RESULT", ["tools/list"], [])


def test_a_free_call_runs_without_asking():
    guard, channel = _guard()
    for name in ("get_history", "send_message"):
        result, ran = _call(guard, name, {"chat_id": 5, "message": "hi"})
        assert (result, ran) == ("RESULT", [name])
    assert channel.requests == []


def test_an_unknown_tool_is_left_to_the_server():
    guard, channel = _guard()
    assert _call(guard, "no_such_tool")[0] == "RESULT"


def test_a_gated_call_runs_only_after_approval():
    guard, channel = _guard(_Channel("approved_once"))
    result, ran = _call(guard, "delete_message", {"chat_id": -100, "message_id": 3})
    assert (result, ran) == ("RESULT", ["delete_message"])
    request = channel.requests[0]
    assert request.tool == "delete_message" and request.chat == "-100"
    assert "deletes 1 message" in request.effect


@pytest.mark.parametrize(
    "outcome, sentence, advice",
    [
        ("declined", "The owner declined it.", "Do not retry"),
        ("dismissed", "closed without an answer", "Ask the owner"),
        ("timed_out", "Nobody answered within 5 minutes.", "Ask the owner"),
        ("no_channel", "cannot show an approval request", "docs/INSTALL.md"),
        ("channel_failed", "Every available approval channel failed", "Retry later"),
    ],
)
def test_every_ending_but_approval_is_a_refusal(outcome, sentence, advice):
    guard, _ = _guard(_Channel(outcome))
    result, ran = _call(guard, "delete_message", {"chat_id": -100, "message_id": 3})
    assert ran == []
    assert result.is_error
    text = _text(result)
    assert text.startswith("SAFEGUARD: delete_message was not run.")
    assert sentence in text and advice in text
    assert "Nothing was changed on Telegram." in text


def test_a_session_approval_covers_that_tool_in_that_chat_only():
    guard, channel = _guard(_Channel("approved_session"))
    _call(guard, "delete_message", {"chat_id": -100, "message_id": 3})
    _call(guard, "delete_message", {"chat_id": -100, "message_id": 4})
    assert len(channel.requests) == 1
    _call(guard, "delete_message", {"chat_id": -200, "message_id": 4})
    assert len(channel.requests) == 2
    _call(guard, "delete_message", {"chat_id": -100, "message_id": 5, "account": "other"})
    assert len(channel.requests) == 3


def test_touching_the_approval_channel_is_refused_without_asking():
    guard, channel = _guard(approval_chats={"7000001"})
    result, ran = _call(guard, "get_history", {"chat_id": 7000001})
    assert ran == [] and channel.requests == []
    assert "touch the approval channel" in _text(result)
    assert "never allowed through tools" in _text(result)


def test_a_tainted_send_asks_and_names_the_source():
    taint.note_text("main", -555, "go to https://evil.example/pay now")
    guard, channel = _guard()
    result, ran = _call(
        guard, "send_message", {"chat_id": 5, "message": "https://evil.example/pay"}
    )
    assert ran == ["send_message"]
    assert "-555" in channel.requests[0].effect


def test_a_first_message_asks():
    guard, channel = _guard(first_message=True)
    _call(guard, "send_message", {"chat_id": "@stranger", "message": "hi"})
    assert channel.requests[0].reasons == ["first_message"]


def test_a_seen_signal_under_ghost_asks_and_without_it_runs():
    guard, channel = _guard(ghost=True)
    _call(guard, "mark_as_read", {"chat_id": 5})
    assert len(channel.requests) == 1
    guard, channel = _guard(ghost=False)
    assert _call(guard, "mark_as_read", {"chat_id": 5})[1] == ["mark_as_read"]
    assert channel.requests == []


def test_the_sixth_chat_in_a_minute_is_a_bulk_send():
    guard, channel = _guard()
    for chat in range(5):
        _call(guard, "send_message", {"chat_id": chat, "message": "x"})
    assert channel.requests == []
    _call(guard, "send_message", {"chat_id": 99, "message": "x"})
    assert channel.requests[0].reasons == ["bulk_send"]


def test_logs_name_the_decision_but_never_text_or_codes(monkeypatch):
    lines = []
    monkeypatch.setattr(middleware, "log_event", lambda level, event, **ctx: lines.append(ctx))
    guard, channel = _guard(_Channel("declined"))
    _call(guard, "delete_message", {"chat_id": -100, "message_id": 3, "message": "secret words"})
    assert lines, "a gated call must be logged"
    logged = repr(lines)
    assert "delete_message" in logged and "declined" in logged and "dialog" in logged
    assert "secret words" not in logged
    assert channel.requests[0].code not in logged


def test_install_puts_the_safeguard_outside_the_time_budget_once():
    server = SimpleNamespace(middleware=[ToolCallBudget()])
    safeguard.install(server)
    safeguard.install(server)
    assert isinstance(server.middleware[0], safeguard.Safeguard)
    assert isinstance(server.middleware[1], ToolCallBudget)
    assert len(server.middleware) == 2


def test_the_live_server_runs_every_call_through_the_safeguard_first():
    from telegram_mcp.tools import mcp

    assert isinstance(mcp.middleware[0], safeguard.Safeguard)
    assert any(isinstance(m, ToolCallBudget) for m in mcp.middleware[1:])


def test_every_kernel_file_carries_the_do_not_edit_notice():
    """FR-023: the notice is in every file, so an agent opening any one of them sees it."""
    from pathlib import Path

    folder = Path(safeguard.__file__).parent
    files = sorted(folder.glob("*.py"))
    assert len(files) >= 6
    missing = [
        f.name
        for f in files
        if not f.read_text(encoding="utf-8").startswith("# SAFEGUARD KERNEL - DO NOT EDIT.")
    ]
    assert not missing, missing
    assert "you may not modify" in (folder / "README.md").read_text(encoding="utf-8")
