"""Every tool call an agent makes lands in its own log (spec 018 US2).

One JSON line per call: when, which client, which tool and account, the arguments as
sent (secrets redacted), how it ended, how long it took, the error in full and the start
of the result. A logging failure never touches the call itself.
"""

import asyncio
import json
import os
import time
from types import SimpleNamespace

import pytest
from mcp.types import CallToolResult, TextContent

from telegram_mcp import command_log

BOT_TOKEN = "123456789:" + "A" * 35
INVITE = "https://t.me/+abcDEF123"


def _ctx(name="send_message", arguments=None, method="tools/call"):
    client = SimpleNamespace(client_info=SimpleNamespace(name="claude-code", version="2.1.0"))
    return SimpleNamespace(
        method=method,
        request_id=7,
        params={"name": name, "arguments": arguments or {}},
        session=SimpleNamespace(client_params=client),
    )


def _result(text, is_error=False):
    return CallToolResult(content=[TextContent(type="text", text=text)], is_error=is_error)


def _run(recorder, ctx, answer):
    async def call_next(_ctx):
        if isinstance(answer, BaseException):
            raise answer
        return answer

    return asyncio.run(recorder(ctx, call_next))


def _records(directory):
    (log,) = list(directory.glob("agent-commands_*_UTC.log"))
    return [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]


@pytest.fixture
def recorder(tmp_path):
    return command_log.CommandLog(directory=tmp_path)


def test_a_call_is_recorded_with_everything_useful(recorder, tmp_path):
    args = {"chat_id": "BotFather", "message": "/newbot", "account": "refx"}
    _run(recorder, _ctx(arguments=args), _result("Message sent (id 42)."))
    (record,) = _records(tmp_path)
    assert record["tool"] == "send_message" and record["account"] == "refx"
    assert record["arguments"] == args
    assert record["outcome"] == "ok" and record["result"] == "Message sent (id 42)."
    assert record["client"] == "claude-code 2.1.0" and record["request_id"] == 7
    assert record["time"].endswith("Z") and isinstance(record["duration_ms"], int)


def test_one_file_per_run_named_in_utc(recorder, tmp_path):
    _run(recorder, _ctx(), _result("a"))
    _run(recorder, _ctx(), _result("b"))
    (log,) = list(tmp_path.iterdir())
    assert log.name.startswith("agent-commands_") and log.name.endswith("_UTC.log")
    assert len(_records(tmp_path)) == 2


def test_secrets_are_redacted_by_shape_and_by_name(recorder, tmp_path):
    args = {"message": f"token {BOT_TOKEN} link {INVITE}", "password": "hunter2hunter2"}
    _run(recorder, _ctx(arguments=args), _result(f"echo {INVITE}"))
    raw = next(tmp_path.iterdir()).read_text(encoding="utf-8")
    assert BOT_TOKEN not in raw and INVITE not in raw and "hunter2" not in raw
    (record,) = _records(tmp_path)
    assert record["arguments"]["password"] == "[REDACTED]"
    assert record["arguments"]["message"].startswith("token [REDACTED]")


def test_the_result_is_cut_but_an_error_is_kept_whole(recorder, tmp_path):
    _run(recorder, _ctx(), _result("r" * 5000))
    _run(recorder, _ctx(), _result("e" * 5000, is_error=True))
    ok, failed = _records(tmp_path)
    assert len(ok["result"]) == 1000 and "error" not in ok
    assert failed["outcome"] == "error" and len(failed["error"]) == 5000


@pytest.mark.parametrize(
    "text, outcome",
    [
        ("SAFEGUARD: send_message was not run. The owner declined it.", "refused_by_safeguard"),
        ("This tool call was stopped after 55s because it had not answered.", "timed_out"),
    ],
)
def test_refusals_and_timeouts_are_named(recorder, tmp_path, text, outcome):
    _run(recorder, _ctx(), _result(text, is_error=True))
    (record,) = _records(tmp_path)
    assert record["outcome"] == outcome and record["error"] == text


def test_an_exception_is_recorded_and_raised_again(recorder, tmp_path):
    with pytest.raises(RuntimeError):
        _run(recorder, _ctx(), RuntimeError("boom"))
    (record,) = _records(tmp_path)
    assert record["outcome"] == "exception" and record["error"] == "RuntimeError: boom"


def test_other_methods_are_not_recorded(recorder, tmp_path):
    _run(recorder, _ctx(method="tools/list"), _result("x"))
    assert list(tmp_path.iterdir()) == []


def test_a_write_failure_never_breaks_the_call(tmp_path, monkeypatch):
    blocker = tmp_path / "not-a-dir"
    blocker.write_text("x", encoding="utf-8")
    recorder = command_log.CommandLog(directory=blocker / "logs")
    answer = _result("fine")
    assert _run(recorder, _ctx(), answer) is answer


def test_files_older_than_30_days_are_deleted(tmp_path, recorder):
    old = tmp_path / "agent-commands_2026-01-01_00-00-00_UTC.log"
    recent = tmp_path / "agent-commands_2026-09-20_00-00-00_UTC.log"
    other = tmp_path / "keep-me.txt"
    for path in (old, recent, other):
        path.write_text("{}\n", encoding="utf-8")
    month_ago = time.time() - 31 * 24 * 3600
    os.utime(old, (month_ago, month_ago))
    _run(recorder, _ctx(), _result("x"))
    assert not old.exists() and recent.exists() and other.exists()


def test_installed_outermost_exactly_once():
    server = SimpleNamespace(middleware=["safeguard", "budget"])
    command_log.install(server)
    command_log.install(server)
    assert isinstance(server.middleware[0], command_log.CommandLog)
    assert server.middleware[1:] == ["safeguard", "budget"]


def test_the_server_records_calls_outside_the_safeguard():
    import telegram_mcp.tools  # noqa: F401 - installs the chain
    from telegram_mcp.runtime import mcp
    from telegram_mcp.safeguard import Safeguard

    kinds = [type(m) for m in mcp.middleware]
    assert kinds.index(command_log.CommandLog) < kinds.index(Safeguard)
