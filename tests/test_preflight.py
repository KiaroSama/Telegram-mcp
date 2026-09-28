"""A call bound to fail is refused before anyone is asked to approve it (spec 021).

Found live 2026-09-28: deleting the General topic asked the owner, was approved, and only
then did the tool refuse. The safeguard runs before the tool validates its arguments, so
the owner's approval was spent on a call that could never succeed. Now the tool's own
schema and a table of argument rules run first, and the refusal tells the agent why.
"""

import asyncio
from types import SimpleNamespace

import pytest
from mcp.types import CallToolResult

from telegram_mcp import preflight


def _call(name, arguments):
    reached = []
    ctx = SimpleNamespace(method="tools/call", params={"name": name, "arguments": arguments})

    async def call_next(_ctx):
        reached.append(name)
        return "ran"

    result = asyncio.run(preflight.Preflight()(ctx, call_next))
    return result, reached


def _text(result):
    assert isinstance(result, CallToolResult) and result.is_error
    return result.content[0].text


@pytest.fixture(autouse=True)
def _tools():
    import telegram_mcp.tools  # noqa: F401 - registers every tool


@pytest.mark.parametrize(
    "name, arguments, words",
    [
        ("delete_forum_topic", {"chat_id": 5, "topic_id": 1, "account": "a"}, "General"),
        ("reorder_pinned_topics", {"chat_id": 5, "topic_ids": [], "account": "a"}, "at least one"),
        ("create_paid_invite_link", {"chat_id": 5, "monthly_fee_stars": 0, "account": "a"}, "1"),
        (
            "schedule_message",
            {
                "chat_id": 5,
                "message": "x",
                "when": "2030-01-01T00:00:00Z",
                "parse_mode": "md",
                "entities": [{"type": "bold", "offset": 0, "length": 1}],
                "account": "a",
            },
            "not both",
        ),
    ],
)
def test_a_call_its_tool_would_refuse_never_reaches_the_approval(name, arguments, words):
    result, reached = _call(name, arguments)
    text = _text(result)
    assert reached == [] and text.startswith("PREFLIGHT:") and words in text
    assert "no approval was asked" in text


def test_arguments_the_tool_schema_rejects_are_refused_with_the_field():
    result, reached = _call(
        "delete_forum_topic", {"chat_id": 5, "topic_id": "abc", "account": "a"}
    )
    text = _text(result)
    assert reached == [] and "topic_id" in text


def test_a_missing_required_argument_is_named():
    result, reached = _call("delete_forum_topic", {"chat_id": 5, "account": "a"})
    assert reached == [] and "topic_id" in _text(result)


def test_a_valid_call_goes_through():
    assert _call("delete_forum_topic", {"chat_id": 5, "topic_id": 12, "account": "a"}) == (
        "ran",
        ["delete_forum_topic"],
    )


def test_an_unknown_tool_is_left_to_the_server():
    assert _call("no_such_tool", {}) == ("ran", ["no_such_tool"])


def test_it_sits_after_the_account_gate_and_before_the_safeguard_once():
    from telegram_mcp.account_gate import AccountGate
    from telegram_mcp.runtime import mcp
    from telegram_mcp.safeguard import Safeguard

    preflight.install(mcp)
    kinds = [type(m) for m in mcp.middleware]
    assert kinds.count(preflight.Preflight) == 1
    assert kinds.index(AccountGate) < kinds.index(preflight.Preflight) < kinds.index(Safeguard)


def test_the_command_log_names_the_preflight_refusal():
    from telegram_mcp import command_log

    prefixes = dict(command_log._OUTCOME_PREFIXES)
    assert prefixes["PREFLIGHT:"] == "refused_before_approval"
    assert prefixes["ACCOUNT REQUIRED:"] == "refused_account"
