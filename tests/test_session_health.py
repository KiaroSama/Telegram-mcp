"""Current self-RPC evidence, not cached authorization or socket liveness."""

import pytest
from telethon import errors, types

from telegram_mcp import session_health as health


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "outcome, status",
    [
        (types.User(id=42), "healthy"),
        (errors.SessionRevokedError(None), "revoked"),
        (errors.AuthKeyUnregisteredError(None), "revoked"),
        (ConnectionError("private-canary"), "network"),
        (RuntimeError("private-canary"), "unknown"),
    ],
)
async def test_health_observes_current_self_request(outcome, status):
    calls = []

    class Client:
        async def __call__(self, request):
            calls.append(request)
            if isinstance(outcome, Exception):
                raise outcome
            return [outcome]

    answer = await health.probe(Client())
    assert answer["status"] == status
    assert answer["repairable"] is (status == "revoked")
    assert len(calls) == 1
    assert isinstance(calls[0].id[0], types.InputUserSelf)
    assert "private-canary" not in str(answer)


@pytest.mark.asyncio
async def test_busy_lease_never_constructs_a_second_client(monkeypatch):
    from telethon.crypto import AuthKey
    from telethon.sessions import StringSession
    from telegram_mcp import session_files

    session = StringSession()
    session.set_dc(2, "149.154.167.51", 443)
    session.auth_key = AuthKey(bytes(range(256)))
    value = session.save()

    class Lock:
        def __init__(self, identity):
            assert identity == "string:" + value

        def acquire(self, **kwargs):
            assert kwargs == {"grace_seconds": 0}
            raise health.SessionLockError("busy")

    monkeypatch.setattr(health, "SessionLock", Lock)
    monkeypatch.setattr(session_files, "_build_client", lambda *a: pytest.fail("competing client"))
    answer = await health.check_configured(
        "TELEGRAM_SESSION_STRING_WORK",
        {
            "TELEGRAM_SESSION_STRING_WORK": value,
        },
    )
    assert answer == {"status": "busy", "repairable": False}


@pytest.mark.asyncio
@pytest.mark.parametrize("close_fails", [False, True])
async def test_offline_probe_holds_lease_until_disconnect(monkeypatch, close_fails):
    from telethon.crypto import AuthKey
    from telethon.sessions import StringSession
    from telegram_mcp import session_files

    session = StringSession()
    session.set_dc(2, "149.154.167.51", 443)
    session.auth_key = AuthKey(bytes(range(256)))
    actions = []

    class Lock:
        def __init__(self, _):
            pass

        def acquire(self, **kwargs):
            actions.append("lease")

        def release(self):
            actions.append("release")

    from types import SimpleNamespace

    class Client:
        _sender = SimpleNamespace()

        async def connect(self):
            actions.append("connect")
            assert self._sender._retries == 0 and self._sender._auto_reconnect is False

        async def __call__(self, request):
            actions.append("rpc")
            return [types.User(id=42)]

        async def disconnect(self):
            actions.append("disconnect")
            if close_fails:
                raise OSError("private-canary")

        def is_connected(self):
            return close_fails

    monkeypatch.setattr(health, "SessionLock", Lock)
    monkeypatch.setattr(session_files, "_build_client", lambda *a: Client())
    monkeypatch.setattr(health, "_unclosed", [])
    answer = await health.check_configured(
        "TELEGRAM_SESSION_STRING",
        {
            "TELEGRAM_SESSION_STRING": session.save(),
        },
    )
    assert actions == ["lease", "connect", "rpc", "disconnect"] + (
        [] if close_fails else ["release"]
    )
    assert answer["status"] == ("unknown" if close_fails else "healthy")
    assert len(health._unclosed) == int(close_fails)


@pytest.mark.asyncio
async def test_runtime_reuses_only_matching_client_generation(monkeypatch):
    from types import SimpleNamespace
    from telegram_mcp import connection, account_snapshot, admission

    monkeypatch.setattr(admission, "_lease_of", lambda label, client: object())
    calls = []

    class Client:
        session = SimpleNamespace(save=lambda: "synthetic-session")

        def is_connected(self):
            return True

        async def __call__(self, request):
            calls.append(request)
            return [types.User(id=42)]

    client = Client()
    monkeypatch.setattr(connection, "clients", {"work": client})
    monkeypatch.setattr(
        account_snapshot,
        "read_snapshot",
        lambda: SimpleNamespace(env={"TELEGRAM_SESSION_STRING_WORK": "synthetic-session"}),
    )
    key = "TELEGRAM_SESSION_STRING_WORK"
    assert (await health.check_runtime(key, "wrong-fingerprint"))["status"] == "unknown"
    assert calls == []
    assert (await health.check_runtime(key, health.digest("synthetic-session")))[
        "status"
    ] == "healthy"
    assert len(calls) == 1
    client.session = SimpleNamespace(save=lambda: "different-generation")
    assert (await health.check_runtime(key, health.digest("synthetic-session")))[
        "status"
    ] == "unknown"
    assert len(calls) == 1


def test_private_run_logs_are_distinct_and_closed(tmp_path, monkeypatch):
    from telegram_mcp.session_log import session_log

    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state with spaces"))
    with session_log("session_health") as event:
        event("INFO", "Session health: healthy")
    with pytest.raises(RuntimeError):
        with session_log("session_health"):
            raise RuntimeError("private-canary")
    logs = list((tmp_path / "state with spaces" / "telegram-mcp" / "logs").glob("*.log"))
    assert len(logs) == 2
    texts = [p.read_text(encoding="utf-8") for p in logs]
    assert any("Session health: healthy" in t for t in texts)
    assert any("[ERROR]" in t for t in texts)
    assert all(
        "private-canary" not in t and "[session_health]" in t and " UTC]" in t for t in texts
    )
    for p in logs:
        p.unlink()  # Windows would refuse an unclosed file handle.


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "payload, status",
    [
        ({"status": "healthy", "user_id": 42}, "healthy"),
        ({"status": "revoked"}, "revoked"),
        ({"status": "not-a-status"}, "busy"),
    ],
)
async def test_runtime_transport_preserves_only_known_status(payload, status):
    from contextlib import asynccontextmanager
    import json
    from types import SimpleNamespace

    @asynccontextmanager
    async def transport(url, http_client):
        assert url == "http://127.0.0.1:8765/mcp"
        yield "read", "write"

    class Session:
        def __init__(self, reader, writer):
            assert (reader, writer) == ("read", "write")

        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            pass

        async def initialize(self):
            pass

        async def call_tool(self, name, args):
            assert name == "check_account_session"
            assert args == {
                "config_key": "TELEGRAM_SESSION_STRING",
                "config_digest": health.digest("synthetic-session"),
            }
            return SimpleNamespace(
                is_error=False, content=[SimpleNamespace(type="text", text=json.dumps(payload))]
            )

    answer = await health._call_runtime(
        transport,
        Session,
        object(),
        "http://127.0.0.1:8765/mcp",
        "TELEGRAM_SESSION_STRING",
        "synthetic-session",
    )
    assert answer["status"] == status
