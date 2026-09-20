"""An expired session must still be able to re-initialize.

Reported 2026-09-20 by a second agent driving this server, and reproduced here
against the live process with a bogus session id:

    tools/list  with an unknown Mcp-Session-Id  ->  404
    initialize  with an unknown Mcp-Session-Id  ->  404   <- the defect

The first 404 is DELIBERATE and must stay. `runner._stateless_http` chose stateful
HTTP precisely so that a restart is observable: "that 404 is the whole point ... the
spec has the client re-initialise when it sees one - which refetches the tools."

The second 404 is what makes that sentence false. The SDK's session manager
(`mcp/server/streamable_http_manager.py`) reaches its new-session branch only when
the header is ABSENT, so a client holding a stale id can never get a fresh one. And
the client library turns a 404-with-a-session-id into an ordinary JSON-RPC error
(`"Session terminated"`) WITHOUT tearing the transport down, so the harness keeps
reporting the connector as connected while every call fails identically, forever.
Measured in that session: `session_connectors_status` said `connected,
tool_count: 227` while every tool answered `Session terminated`.

`initialize` is the method that CREATES a session, so a session id on it is
meaningless by definition. Dropping it is what restores the recovery path the
docstring already assumes.
"""

import json

import pytest

from telegram_mcp import runner


def _scope(method="POST", path="/mcp", headers=None):
    return {
        "type": "http",
        "method": method,
        "path": path,
        "headers": [(k.lower().encode(), v.encode()) for k, v in (headers or {}).items()],
    }


async def _drive(app, scope, body: bytes):
    """Run one request through an ASGI app, returning the scope it passed down."""
    seen = {}

    async def receive():
        return {"type": "http.request", "body": body, "more_body": False}

    async def send(message):
        seen.setdefault("sent", []).append(message)

    async def inner(inner_scope, inner_receive, inner_send):
        seen["scope"] = inner_scope
        # Drain, so a middleware that buffered the body proves it replays it.
        seen["body"] = (await inner_receive())["body"]

    wrapped = runner._recoverable_sessions(inner)
    await wrapped(scope, receive, send)
    return seen


def _header(scope, name):
    for key, value in scope["headers"]:
        if key == name.encode():
            return value.decode()
    return None


@pytest.mark.asyncio
async def test_initialize_loses_a_stale_session_id():
    """The whole fix: the one method that mints a session never carries one."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize"}).encode()
    seen = await _drive(
        None,
        _scope(headers={"mcp-session-id": "deadbeef", "content-type": "application/json"}),
        body,
    )

    assert _header(seen["scope"], "mcp-session-id") is None
    assert seen["body"] == body, "the body must reach the app unchanged after buffering"


@pytest.mark.asyncio
async def test_every_other_call_keeps_its_session_id():
    """The 404 for an ordinary call is the signal a restart happened. It stays."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).encode()
    seen = await _drive(None, _scope(headers={"mcp-session-id": "deadbeef"}), body)

    assert _header(seen["scope"], "mcp-session-id") == "deadbeef"


@pytest.mark.asyncio
async def test_a_body_that_is_not_json_is_passed_through_untouched():
    """A transport-level guard must never be the thing that rejects a request."""
    seen = await _drive(None, _scope(headers={"mcp-session-id": "deadbeef"}), b"not json at all")

    assert _header(seen["scope"], "mcp-session-id") == "deadbeef"
    assert seen["body"] == b"not json at all"


@pytest.mark.asyncio
async def test_a_batch_containing_initialize_also_drops_it():
    """A JSON-RPC batch is a list; an initialize inside one means the same thing."""
    body = json.dumps([{"jsonrpc": "2.0", "id": 1, "method": "initialize"}]).encode()
    seen = await _drive(None, _scope(headers={"mcp-session-id": "deadbeef"}), body)

    assert _header(seen["scope"], "mcp-session-id") is None


@pytest.mark.asyncio
async def test_a_non_http_scope_is_not_touched():
    """Lifespan and websocket scopes have no headers to read."""
    seen = {}

    async def inner(scope, receive, send):
        seen["scope"] = scope

    await runner._recoverable_sessions(inner)({"type": "lifespan"}, None, None)
    assert seen["scope"] == {"type": "lifespan"}


def test_the_recovery_is_installed_without_taking_over_the_serve_call():
    """The regression that cost 22 minutes of a hung suite.

    The first version of this fix replaced `mcp.run_streamable_http_async` with a
    hand-built `uvicorn.Server(...).serve()`. `tests/test_runtime.py` patches that
    exact method to stop `_serve` binding a real port, so the patch stopped
    applying, the port was bound for real, and the full suite sat with no output
    and almost no CPU until it was killed.

    Wrapping the app BUILDER instead reaches the same served app and leaves that
    seam alone. This test pins both halves.
    """

    class _Server:
        def __init__(self):
            self.built = []

        def streamable_http_app(self, **kwargs):
            self.built.append(kwargs)
            return "the app"

        async def run_streamable_http_async(self, **kwargs):  # pragma: no cover
            raise AssertionError("_serve must keep calling this, patched or not")

    server = _Server()
    runner._install_session_recovery(server)

    # `is` on two bound methods is always False - each attribute access builds a
    # fresh object - so the claim is made the only way it can be: the instance
    # carries no attribute shadowing the class's method.
    assert "run_streamable_http_async" not in vars(server), "the serve call was taken over"
    wrapped = server.streamable_http_app(host="127.0.0.1")
    assert wrapped != "the app", "the builder returned the app unwrapped"
    assert server.built == [{"host": "127.0.0.1"}], "the builder's arguments were altered"


def test_installing_twice_wraps_once():
    """A second install would wrap the wrapper and read the body twice."""

    class _Server:
        def streamable_http_app(self, **kwargs):
            return "the app"

    server = _Server()
    runner._install_session_recovery(server)
    once = server.streamable_http_app
    runner._install_session_recovery(server)

    assert server.streamable_http_app is once


def test_a_server_that_builds_no_app_is_left_alone():
    """`_serve`'s own tests drive it with a double that serves nothing."""

    class _Bare:
        pass

    bare = _Bare()
    runner._install_session_recovery(bare)  # must not raise
    assert not hasattr(bare, "streamable_http_app")
