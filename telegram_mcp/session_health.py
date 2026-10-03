"""Current authorization evidence without competing with a runtime-owned session."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import io
import json
import os
from pathlib import Path

from dotenv.parser import parse_stream
from telethon import errors, functions, types
from telethon.sessions import StringSession

from telegram_mcp.singleton import SessionLock, SessionLockError, session_identity

PROBE_SECONDS = 20
CLOSE_SECONDS = 5
_unclosed = []  # Failed closes retain their lease until process exit.


def result(status, *, user_id=None):
    out = {"status": status, "repairable": status in {"unauthorized", "revoked", "invalid"}}
    if type(user_id) is int and user_id > 0:
        out["user_id"] = user_id
    return out


def classify(error):
    if isinstance(
        error,
        (
            errors.SessionRevokedError,
            errors.AuthKeyDuplicatedError,
            errors.AuthKeyUnregisteredError,
            errors.AuthKeyInvalidError,
        ),
    ):
        return result("revoked")
    if isinstance(error, errors.UnauthorizedError):
        return result("unauthorized")
    if isinstance(error, (OSError, asyncio.TimeoutError, ConnectionError)):
        return result("network")
    return result("unknown")


async def probe(client):
    """A real self RPC; get_me suppresses authorization errors into None."""
    try:
        async with asyncio.timeout(PROBE_SECONDS):
            users = await client(functions.users.GetUsersRequest([types.InputUserSelf()]))
        if not users or not isinstance(users[0], types.User):
            return result("unauthorized")
        return result("healthy", user_id=users[0].id)
    except Exception as error:
        return classify(error)


def read_config(path):
    return parse_config(Path(path).read_bytes())


def parse_config(raw):
    from telegram_mcp.account_snapshot import PROCESS_ACCOUNT_VARS, _interpolate

    bindings = list(parse_stream(io.StringIO(raw.decode("utf-8"))))
    if any(binding.error for binding in bindings):
        raise ValueError("Configuration cannot be parsed.")
    values = {}
    for binding in bindings:
        if binding.key is not None:
            if binding.key in values:
                raise ValueError("Configuration contains a duplicate key.")
            values[binding.key] = binding.value
    populated = {key: value for key, value in values.items() if value is not None}
    return {**_interpolate(populated, PROCESS_ACCOUNT_VARS), **PROCESS_ACCOUNT_VARS}


def digest(value):
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def label_for(key):
    for prefix in ("TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME"):
        if key == prefix:
            return "default"
        if key.startswith(prefix + "_"):
            from telegram_mcp.aliases import normalise_account_label

            return normalise_account_label(key[len(prefix) + 1 :]).lower()
    raise ValueError("Not an account configuration key.")


async def _runtime_probe(url, key, value):
    from mcp import ClientSession
    from mcp.client.streamable_http import streamable_http_client
    import httpx2

    try:
        async with asyncio.timeout(PROBE_SECONDS + CLOSE_SECONDS):
            async with httpx2.AsyncClient(
                timeout=PROBE_SECONDS + CLOSE_SECONDS, trust_env=False
            ) as http:
                return await _call_runtime(
                    streamable_http_client, ClientSession, http, url, key, value
                )
    except Exception:
        return result("busy")


async def _call_runtime(transport, session_type, http, url, key, value):
    async with transport(url, http_client=http) as (reader, writer):
        async with session_type(reader, writer) as session:
            await session.initialize()
            reply = await session.call_tool(
                "check_account_session",
                {"config_key": key, "config_digest": digest(value)},
            )
            if reply.is_error:
                return result("busy")
            parsed = json.loads(next(item.text for item in reply.content if item.type == "text"))
            if parsed.get("status") not in {
                "healthy",
                "unauthorized",
                "revoked",
                "invalid",
                "network",
                "unknown",
                "busy",
            }:
                return result("busy")
            return result(parsed["status"], user_id=parsed.get("user_id"))


async def check_configured(key, values, *, runtime_url=None):
    label = label_for(key)
    value = values.get(key)
    if not value:
        return result("invalid")
    if key.startswith("TELEGRAM_SESSION_STRING"):
        try:
            session = StringSession(value)
        except Exception:
            return result("invalid")
        if not session.auth_key:
            return result("unauthorized")
        identity = f"string:{session.save()}"
    else:
        from telegram_mcp.session_files import session_file_path

        path = session_file_path(value)
        if not path.is_file():
            return result("invalid")
        identity = f"file:{os.path.normcase(os.path.realpath(path))}"
        session = str(path)
    lock = SessionLock(identity)
    try:
        lock.acquire(grace_seconds=0)
    except SessionLockError:
        if runtime_url:
            return await _runtime_probe(runtime_url, key, value)
        return result("busy")
    client = None
    closed = True
    try:
        from telegram_mcp.session_files import _build_client

        client = _build_client(session, label)
        client.flood_sleep_threshold = 0
        client._request_retries = 0
        client._connection_retries = 0
        client._auto_reconnect = False
        # Sender options were copied at construction, not read from the client on connect.
        client._sender._retries = 0
        client._sender._auto_reconnect = False
        closed = False
        async with asyncio.timeout(PROBE_SECONDS):
            await client.connect()
            answer = await probe(client)
    except Exception as error:
        answer = classify(error)
    finally:
        if client is not None:
            try:
                async with asyncio.timeout(CLOSE_SECONDS):
                    await client.disconnect()
                closed = not client.is_connected()
            except Exception:
                closed = False
        if closed:
            lock.release()
        else:
            _unclosed.append((client, lock))
    return answer if closed else result("unknown")


async def check_runtime(key, fingerprint):
    """Only the client generation matching this exact configuration may answer."""
    from telegram_mcp import connection
    from telegram_mcp.account_snapshot import read_snapshot

    if not isinstance(key, str) or not isinstance(fingerprint, str):
        return result("unknown")
    label = label_for(key)
    snapshot = read_snapshot()
    value = snapshot.env.get(key)
    if not value or digest(value) != fingerprint:
        return result("unknown")
    client = connection.clients.get(label)
    if client is None:
        return result("busy")
    if key.startswith("TELEGRAM_SESSION_STRING"):
        identity = f"string:{value}"
    else:
        from telegram_mcp.session_files import session_file_path

        identity = f"file:{os.path.normcase(os.path.realpath(session_file_path(value)))}"
    if session_identity(client) != identity:
        return result("unknown")
    # A revoked runtime client may already have disconnected. Reuse its lease
    # and its client, otherwise a dead login would look like a network failure.
    from telegram_mcp import admission

    if admission._lease_of(label, client) is None:
        return result("busy")
    try:
        async with asyncio.timeout(PROBE_SECONDS):
            if not client.is_connected():
                await client.connect()
            answer = await probe(client)
    except Exception as error:
        answer = classify(error)
    if connection.clients.get(label) is not client:
        return result("unknown")
    return answer


def _main(event):
    parser = argparse.ArgumentParser(
        description="Check configured Telegram sessions without login."
    )
    parser.add_argument(
        "--env-file", type=Path, default=Path(__file__).resolve().parents[1] / ".env"
    )
    parser.add_argument("--key", required=True)
    args = parser.parse_args()
    try:
        values = read_config(args.env_file)
        from dotenv import load_dotenv

        load_dotenv(args.env_file, encoding="utf-8")
        host = values.get("MCP_HOST") or os.getenv("MCP_HOST", "127.0.0.1")
        if host not in {"127.0.0.1", "localhost", "::1", "0.0.0.0"}:
            url = None
        else:
            port = int(values.get("MCP_PORT") or os.getenv("MCP_PORT", "8765"))
            if not 1 <= port <= 65535:
                raise ValueError("Invalid MCP port.")
            url = f"http://127.0.0.1:{port}/mcp"
        answer = asyncio.run(check_configured(args.key, values, runtime_url=url))
        if values.get(args.key):
            answer["config_digest"] = digest(values[args.key])
    except Exception:
        answer = result("unknown")
    event("INFO", "Session health: " + answer["status"])
    print(json.dumps(answer))


def main():
    from telegram_mcp.session_log import run_log

    with run_log("session_health") as event:
        _main(event)


if __name__ == "__main__":
    main()
