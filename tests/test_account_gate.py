"""A call that could act through an account must name it (spec 019).

The owner was asked to approve a send that named no account; approved, it then failed
because several accounts are configured. Now such a call is refused before the safeguard
sees it, reads included, even with one account configured.
"""

import asyncio
from types import SimpleNamespace

import pytest
from mcp.types import CallToolResult

from telegram_mcp import account_gate

LABELS = ("kgb_verifier", "refx_nexus_3")


def _gate():
    return account_gate.AccountGate(
        takes_account=lambda name: name != "list_accounts", labels=lambda: LABELS
    )


def _call(gate, name, arguments):
    reached = []
    ctx = SimpleNamespace(method="tools/call", params={"name": name, "arguments": arguments})

    async def call_next(_ctx):
        reached.append(name)
        return "ran"

    return asyncio.run(gate(ctx, call_next)), reached


@pytest.mark.parametrize("arguments", [{}, {"account": None}, {"account": "  "}])
def test_a_call_without_an_account_is_refused_before_anything_runs(arguments):
    result, reached = _call(_gate(), "send_message", dict(arguments, chat_id="BotFather"))
    assert reached == [] and isinstance(result, CallToolResult) and result.is_error
    text = result.content[0].text
    assert "send_message was not run" in text and "kgb_verifier, refx_nexus_3" in text


def test_an_unknown_account_is_refused_too():
    result, reached = _call(_gate(), "get_chat", {"chat_id": 1, "account": "someone"})
    assert reached == [] and "someone" in result.content[0].text


@pytest.mark.parametrize("label", ["refx_nexus_3", "REFX_NEXUS_3"])
def test_a_named_account_goes_through(label):
    assert _call(_gate(), "get_chat", {"chat_id": 1, "account": label}) == ("ran", ["get_chat"])


def test_a_tool_without_an_account_parameter_is_untouched():
    assert _call(_gate(), "list_accounts", {}) == ("ran", ["list_accounts"])


def test_a_view_of_every_account_still_works_without_one():
    # `account` only filters these; omitted, they show every account (safeguard_status),
    # or it names whose grant to drop - possibly an account no longer configured.
    import telegram_mcp.tools  # noqa: F401
    from telegram_mcp.runtime import mcp

    gate = account_gate.AccountGate(labels=lambda: LABELS)
    for name in sorted(account_gate.SCOPE_ONLY):
        assert mcp._tool_manager.get_tool(name) is not None, name
        assert _call(gate, name, {}) == ("ran", [name])
    assert _call(gate, "set_ghost_mode", {"enabled": True})[1] == []


def test_other_methods_pass():
    gate = _gate()
    ctx = SimpleNamespace(method="tools/list", params=None)

    async def call_next(_ctx):
        return "listed"

    assert asyncio.run(gate(ctx, call_next)) == "listed"


def test_every_account_tool_lists_account_as_required():
    import telegram_mcp.tools  # noqa: F401 - installs the gate
    from telegram_mcp.runtime import mcp

    tools = asyncio.run(mcp.list_tools())
    with_account = [
        t
        for t in tools
        if "account" in t.input_schema.get("properties", {})
        and t.name not in account_gate.SCOPE_ONLY
    ]
    assert len(with_account) > 200
    missing = [t.name for t in with_account if "account" not in t.input_schema.get("required", [])]
    assert missing == []
    schema = with_account[0].input_schema["properties"]["account"]
    assert "default" not in schema and "list_accounts" in schema["description"]


def test_the_gate_sits_just_in_front_of_the_safeguard_once():
    import telegram_mcp.tools  # noqa: F401
    from telegram_mcp.command_log import CommandLog
    from telegram_mcp.runtime import mcp
    from telegram_mcp.safeguard import Safeguard

    account_gate.install(mcp)
    kinds = [type(m) for m in mcp.middleware]
    assert kinds.count(account_gate.AccountGate) == 1
    assert kinds.index(CommandLog) < kinds.index(account_gate.AccountGate)
    assert kinds.index(account_gate.AccountGate) + 1 == kinds.index(Safeguard)
