"""Public bounded controls and the existing owner-approval fallback."""

import json

import pytest

from test_secret_custody import CHAT, WIRE_SEQ, OfflineClient, build_manager, seed_store


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "name",
    [
        "configure_secret_backend",
        "pause_secret_chat",
        "check_secret_custody",
        "resume_secret_chat",
        "acknowledge_secret_delivery",
        "receive_secret_media",
    ],
)
@pytest.mark.parametrize("outcome", ["approved_once", "declined"])
async def test_real_safeguard_requires_owner_decision_for_each_write(
    tmp_path, monkeypatch, name, outcome
):
    from types import SimpleNamespace
    from telegram_mcp import safeguard
    from telegram_mcp.safeguard import grants, taint
    from telegram_mcp.tools import secret_custody as tools
    from test_safeguard_middleware import _Channel, _identity, _no_warm

    taint.clear()
    monkeypatch.setattr(grants, "grants_path", lambda: tmp_path / "grants.json")
    grants.reset_cache()
    channel = _Channel(outcome)

    def hints(tool_name):
        annotations = tools.mcp._tool_manager.get_tool(tool_name).annotations
        return annotations.read_only_hint, annotations.destructive_hint

    async def first_message(account, chat):
        return False

    guard = safeguard.Safeguard(
        hints=hints,
        channels=lambda ctx, account: [channel],
        first_message=first_message,
        ghost_on=lambda account, chat: True,
        approval_chats=lambda: frozenset(),
        account_of=lambda args: args["account"],
        identity=_identity,
        warm=_no_warm,
        timeout=1,
    )
    arguments = {
        "account": "acct",
        "secret_chat_id": CHAT,
        "seq_no": WIRE_SEQ,
        "message_id": 43,
        "max_bytes": 1024,
        "mode": "bounded",
        "records": 32,
        "bytes": 4096,
        "age_seconds": 60,
    }
    ran = []

    async def call_next(ctx):
        ran.append(ctx.params["name"])
        return "RESULT"

    context = SimpleNamespace(
        method="tools/call",
        params={"name": name, "arguments": arguments},
        request_id=1,
        session=None,
    )
    try:
        result = await guard(context, call_next)
        assert len(channel.requests) == 1
        assert channel.requests[0].tool == name
        assert channel.requests[0].identity == "acct | 7 | @acct_user"
        if outcome == "approved_once":
            assert result == "RESULT" and ran == [name]
        else:
            assert result.is_error and ran == []
            assert "The owner declined it" in result.content[0].text
    finally:
        grants.reset_cache()
        taint.clear()


@pytest.mark.asyncio
async def test_public_pause_poll_check_resume_is_preserving_and_gated(tmp_path, monkeypatch):
    from telegram_mcp.tools import secret_custody as tools
    from telegram_mcp import connection, preflight, approval_details
    from telegram_mcp.safeguard.policy import categorize

    manager = build_manager(seed_store(tmp_path / "protocol.json"))
    await manager.start()
    client = OfflineClient()
    monkeypatch.setattr(connection, "refresh_accounts", lambda: [])
    monkeypatch.setattr(connection, "clients", {"acct": client})

    async def get_manager(account):
        assert account == "acct"
        return manager

    monkeypatch.setattr(tools.backend, "secret_manager", get_manager)
    try:
        poll = json.loads(await tools.list_secret_deliveries(CHAT, 1, 0, account="acct"))[
            "results"
        ]
        assert poll["deliveries"][0]["seq_no"] == WIRE_SEQ
        paused = json.loads(await tools.pause_secret_chat(CHAT, account="acct"))["results"]
        assert paused["paused"] is True and paused["resumable"] is True
        status = json.loads(await tools.get_secret_pause_status(CHAT, account="acct"))["results"]
        assert status == paused and status["reason"] == "operator"
        checked = json.loads(await tools.check_secret_custody(account="acct"))["results"]
        assert checked["chats"] == [status]
        resumed = json.loads(await tools.resume_secret_chat(CHAT, account="acct"))["results"]
        assert resumed["paused"] is False
        assert len(manager.pending_deliveries(CHAT)) == 1
        assert client.calls == []
        for name in (
            "configure_secret_backend",
            "pause_secret_chat",
            "check_secret_custody",
            "resume_secret_chat",
            "acknowledge_secret_delivery",
            "receive_secret_media",
        ):
            tool = tools.mcp._tool_manager.get_tool(name)
            assert tool is not None
            assert tool.annotations.destructive_hint is True
            assert tool.annotations.read_only_hint is False
            assert (
                categorize(
                    name, tool.annotations.read_only_hint, tool.annotations.destructive_hint
                )
                == "gated"
            )
            assert name in preflight.RULES
            assert await approval_details.detail_for(name, {"seq_no": WIRE_SEQ})
        assert "limit" in preflight.RULES["list_secret_deliveries"](
            {"secret_chat_id": CHAT, "limit": True}
        )
    finally:
        await manager.stop()


@pytest.mark.asyncio
async def test_config_status_public_dispatch_and_invalid_inputs_do_not_start_manager(monkeypatch):
    from telegram_mcp.tools import secret_custody as tools
    from telegram_mcp import connection, preflight

    client = OfflineClient()
    monkeypatch.setattr(connection, "refresh_accounts", lambda: [])
    monkeypatch.setattr(connection, "clients", {"acct": client})
    calls = []

    async def configure(account, mode, **budgets):
        calls.append((account, mode, budgets))
        return {"mode": mode, "limits": budgets}

    async def status(account):
        assert account == "acct"
        return {"mode": "normal", "limits": None, "archive": {"on": True}}

    async def forbidden_manager(*args):
        raise AssertionError("Invalid arguments must refuse before startup")

    monkeypatch.setattr(tools.secret_control, "configure_backend", configure)
    monkeypatch.setattr(tools.secret_control, "backend_status", status)
    monkeypatch.setattr(tools.backend, "secret_manager", forbidden_manager)
    result = json.loads(
        await tools.configure_secret_backend(
            "bounded", records=32, bytes=4096, age_seconds=60, account="acct"
        )
    )["results"]
    assert result["mode"] == "bounded"
    assert calls == [("acct", "bounded", {"records": 32, "bytes": 4096, "age_seconds": 60})]
    observed = json.loads(await tools.get_secret_backend_status(account="acct"))["results"]
    assert observed == {"mode": "normal", "limits": None, "archive": {"on": True}}
    for tool, args in (
        ("acknowledge_secret_delivery", {"secret_chat_id": CHAT, "seq_no": True}),
        ("acknowledge_secret_delivery", {"secret_chat_id": CHAT, "seq_no": None}),
        ("receive_secret_media", {"secret_chat_id": CHAT, "message_id": 43, "max_bytes": True}),
        ("list_secret_deliveries", {"secret_chat_id": CHAT, "limit": 101}),
        ("pause_secret_chat", {"secret_chat_id": False}),
    ):
        assert preflight.RULES[tool](args)
        assert (await getattr(tools, tool)(**args, account="acct")).startswith("Error:")
    assert (
        await tools.configure_secret_backend(
            "bounded", records=True, bytes=4096, age_seconds=60, account="acct"
        )
    ).startswith("Error:")
    assert len(calls) == 1
    for name in ("pause_secret_chat", "acknowledge_secret_delivery", "receive_secret_media"):
        model = tools.mcp._tool_manager.get_tool(name).fn_metadata.arg_model
        with pytest.raises(ValueError):
            model.model_validate(
                {
                    "secret_chat_id": True,
                    "account": "acct",
                    "seq_no": 9,
                    "message_id": 43,
                    "max_bytes": 20,
                }
            )
