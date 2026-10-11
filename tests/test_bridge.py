"""Transport-only bridge seams; synthetic streams never contact Telegram."""

import io
import json
import logging
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx2
import os
from pathlib import Path
import subprocess
import sys
from contextlib import asynccontextmanager

import anyio
import pytest
from mcp.shared.message import SessionMessage
from mcp_types import jsonrpc_message_adapter


@pytest.fixture
def anyio_backend():
    return "asyncio"


def frame(value):
    return SessionMessage(jsonrpc_message_adapter.validate_python(value))


@pytest.mark.anyio
async def test_relay_preserves_handshake_callbacks_and_eof(monkeypatch):
    from telegram_mcp import bridge

    incoming_send, incoming = anyio.create_memory_object_stream(0)
    outgoing, outgoing_recv = anyio.create_memory_object_stream(0)
    remote_send, remote = anyio.create_memory_object_stream(0)
    requests, requests_recv = anyio.create_memory_object_stream(0)
    closed = []

    @asynccontextmanager
    async def stdio():
        try:
            yield incoming, outgoing
        finally:
            closed.append("stdio")

    @asynccontextmanager
    async def http(url, **kwargs):
        assert url == "http://127.0.0.1:12345/mcp"
        assert kwargs["max_sse_event_size"] == 32 * 1024 * 1024
        try:
            yield remote, requests
        finally:
            await anyio.lowlevel.checkpoint()
            closed.append("http")

    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", http)
    init = {
        "jsonrpc": "2.0",
        "id": "init",
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {"elicitation": {"form": {}}},
            "clientInfo": {"name": "test", "version": "1"},
        },
    }
    answer = {
        "jsonrpc": "2.0",
        "id": "init",
        "result": {
            "protocolVersion": "2025-11-25",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "test", "version": "1"},
        },
    }
    callback = {"jsonrpc": "2.0", "id": 7, "method": "roots/list", "params": {}}
    callback_answer = {"jsonrpc": "2.0", "id": 7, "result": {"roots": []}}

    async def peer():
        first = await requests_recv.receive()
        assert first.message.model_dump(exclude_unset=True) == init
        await remote_send.send(frame(answer))
        await remote_send.send(frame(callback))
        last = await requests_recv.receive()
        assert last.message.model_dump(exclude_unset=True) == callback_answer
        assert last.metadata.headers["mcp-protocol-version"] == "2025-11-25"

    async def desktop():
        await incoming_send.send(frame(init))
        assert (await outgoing_recv.receive()).message.model_dump(exclude_unset=True) == answer
        assert (await outgoing_recv.receive()).message.model_dump(exclude_unset=True) == callback
        await incoming_send.send(frame(callback_answer))
        await incoming_send.aclose()

    with anyio.fail_after(3):
        async with anyio.create_task_group() as group:
            group.start_soon(peer)
            group.start_soon(desktop)
            await bridge.relay("http://127.0.0.1:12345/mcp")
    assert closed == ["http", "stdio"]


def test_cli_bridge_dispatch_does_not_import_runtime(tmp_path):
    root = Path(__file__).resolve().parents[1]
    code = """
import importlib.abc, sys, types
class Guard(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname in ("main", "telegram_mcp.runtime", "telegram_mcp.runner",
                        "telegram_mcp.settings", "telegram_mcp.connection", "telegram_mcp.tools"):
            raise AssertionError("eager runtime import")
sys.meta_path.insert(0, Guard())
import telegram_mcp.bridge
startup = types.ModuleType("telegram_mcp.bridge_startup")
startup.ensure_service = lambda: "http://127.0.0.1:12345/mcp"
startup.verify_service = lambda url: None
sys.modules[startup.__name__] = startup
telegram_mcp.bridge.run = lambda url, **kwargs: print("relay-dispatched")
from telegram_mcp.cli import main
sys.argv = ["telegram-mcp", "bridge"]
main()
"""
    env = {k: v for k, v in os.environ.items() if not k.startswith("TELEGRAM_")}
    env["PYTHONPATH"] = str(root)
    result = subprocess.run(
        [sys.executable, "-c", code],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=10,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "relay-dispatched\n"


@pytest.mark.anyio
async def test_real_http_handshake_headers_large_result_and_session_delete(monkeypatch):
    from telegram_mcp import bridge

    seen = []
    deleted = threading.Event()
    callback_done = threading.Event()
    large = "x" * (2 * 1024 * 1024)

    class Server(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args):
            pass

        def reply(self, status, value=b"", kind="application/json", **headers):
            body = value if isinstance(value, bytes) else json.dumps(value).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            for key, value in headers.items():
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            seen.append((request, dict(self.headers)))
            if request.get("method") == "initialize":
                self.reply(
                    200,
                    {
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "synthetic", "version": "1"},
                        },
                    },
                    **{"Mcp-Session-Id": "synthetic-bridge"},
                )
            elif request.get("method") == "tools/call":
                body = (
                    "data: "
                    + json.dumps(
                        {
                            "jsonrpc": "2.0",
                            "id": request["id"],
                            "result": {
                                "content": [{"type": "text", "text": large}],
                                "isError": True,
                            },
                        }
                    )
                    + "\n\n"
                )
                self.reply(200, body.encode("utf-8"), "text/event-stream")
            else:
                if "result" in request:
                    assert request["id"] == 7
                    callback_done.set()
                self.reply(202)

        def do_GET(self):
            seen.append(("GET", dict(self.headers)))
            callback = {"jsonrpc": "2.0", "id": 7, "method": "roots/list"}
            self.reply(
                200,
                ("data: " + json.dumps(callback) + "\n\n").encode("utf-8"),
                "text/event-stream",
            )

        def do_DELETE(self):
            seen.append(("DELETE", dict(self.headers)))
            deleted.set()
            self.reply(204)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Server)
    server.daemon_threads = True
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    incoming_send, incoming = anyio.create_memory_object_stream(0)
    outgoing, output = anyio.create_memory_object_stream(0)

    @asynccontextmanager
    async def stdio():
        yield incoming, outgoing

    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setenv("HTTP_PROXY", "http://127.0.0.1:1")
    monkeypatch.setenv("ALL_PROXY", "http://127.0.0.1:1")

    async def desktop():
        await incoming_send.send(
            frame(
                {
                    "jsonrpc": "2.0",
                    "id": "init",
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {"roots": {"listChanged": True}},
                        "clientInfo": {"name": "Desktop", "version": "1"},
                    },
                }
            )
        )
        assert (await output.receive()).message.id == "init"
        await incoming_send.send(frame({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        await incoming_send.send(
            frame(
                {
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": "tools/call",
                    "params": {"name": "synthetic"},
                }
            )
        )
        messages = [(await output.receive()).message, (await output.receive()).message]
        callback = next(
            message for message in messages if getattr(message, "method", None) == "roots/list"
        )
        assert callback.id == 7
        await incoming_send.send(frame({"jsonrpc": "2.0", "id": 7, "result": {"roots": []}}))
        reply = next(
            message for message in messages if getattr(message, "result", None) is not None
        )
        assert reply.id == 7
        assert reply.result["isError"] is True
        assert reply.result["content"][0]["text"] == large
        assert await anyio.to_thread.run_sync(callback_done.wait, 1)
        await incoming_send.aclose()

    try:
        with anyio.fail_after(5):
            async with anyio.create_task_group() as group:
                group.start_soon(desktop)
                await bridge.relay(f"http://127.0.0.1:{server.server_port}/mcp")
        assert deleted.is_set()
        for value, headers in seen:
            if value in ("GET", "DELETE") if isinstance(value, str) else False:
                assert {key.lower(): val for key, val in headers.items()}[
                    "mcp-protocol-version"
                ] == "2025-11-25"
        post, headers = next(
            (value, headers)
            for value, headers in seen
            if isinstance(value, dict) and value.get("method") == "tools/call"
        )
        assert {key.lower(): val for key, val in headers.items()}[
            "mcp-protocol-version"
        ] == "2025-11-25"
        assert headers["Mcp-Method"] == "tools/call"
        assert headers["mcp-session-id"] == "synthetic-bridge"
        assert (
            sum(
                isinstance(value, dict) and value.get("method") == "initialize"
                for value, _ in seen
            )
            == 1
        )
        assert (
            next(headers for value, headers in seen if value == "DELETE")["mcp-session-id"]
            == "synthetic-bridge"
        )
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)
        assert not worker.is_alive()


@pytest.mark.anyio
@pytest.mark.parametrize("method", ["POST", "GET", "DELETE"])
async def test_redirect_boundary_rejects_every_http_method(monkeypatch, method):
    from telegram_mcp import bridge

    hit = []
    source_send, source = anyio.create_memory_object_stream(0)
    sink, _sink_read = anyio.create_memory_object_stream(0)

    @asynccontextmanager
    async def stdio():
        yield source, sink

    @asynccontextmanager
    async def transport(url, *, http_client, **_kwargs):
        async def redirect(request):
            hit.append(request.url.path)
            return httpx2.Response(307, headers={"Location": "/unverified"})

        http_client._transport = httpx2.MockTransport(redirect)
        with pytest.raises(bridge.BridgeError, match="redirect"):
            await http_client.request(method, url)
        yield source, sink

    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", transport)
    with anyio.fail_after(2):
        with pytest.raises(bridge.BridgeError, match="redirect"):
            await bridge.relay("http://127.0.0.1:12345/mcp")
    assert hit == ["/mcp"]
    await source_send.aclose()


@pytest.mark.anyio
@pytest.mark.parametrize("invalid", [ValueError("private-frame-canary"), "oversized"])
async def test_invalid_stdio_fails_without_payload_logging(monkeypatch, caplog, invalid):
    from telegram_mcp import bridge

    send, read = anyio.create_memory_object_stream(1)
    target, target_read = anyio.create_memory_object_stream(1)
    closed = []

    @asynccontextmanager
    async def stdio():
        yield read, target

    @asynccontextmanager
    async def transport(_url, **_kwargs):
        try:
            yield target_read, target
        finally:
            closed.append(True)

    if invalid == "oversized":
        monkeypatch.setattr(bridge, "MAX_FRAME_BYTES", 32)
        invalid = frame({"jsonrpc": "2.0", "id": 1, "method": "private-frame-canary"})
    await send.send(invalid)
    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", transport)
    with anyio.fail_after(2):
        with pytest.raises(bridge.BridgeError) as error:
            await bridge.relay("http://127.0.0.1:12345/mcp")
    assert "private-frame-canary" not in str(error.value)
    assert "private-frame-canary" not in caplog.text
    assert closed == [True]


def test_cli_legacy_dispatch_imports_compatibility_main(monkeypatch):
    import types
    from telegram_mcp import cli

    calls = []
    module = types.ModuleType("main")
    module.main = lambda: calls.append("legacy")
    monkeypatch.setitem(sys.modules, "main", module)
    monkeypatch.setattr(sys, "argv", ["telegram-mcp", "--help"])
    cli.main()
    assert calls == ["legacy"]


def test_dependency_logging_restores_levels_and_filters():
    from telegram_mcp import bridge

    logger = logging.getLogger("mcp.client.streamable_http")
    old_level, old_filters = logger.level, list(logger.filters)
    capture = io.StringIO()
    handler = logging.StreamHandler(capture)
    logger.addHandler(handler)
    logger.setLevel(logging.DEBUG)
    try:
        with bridge._quiet_dependencies(lambda _text: None):
            logger.error("private-frame-canary", exc_info=True)
        assert capture.getvalue() == ""
        assert logger.level == logging.DEBUG
        assert logger.filters == old_filters
    finally:
        logger.removeHandler(handler)
        logger.setLevel(old_level)


@pytest.mark.anyio
@pytest.mark.parametrize(
    "url",
    [
        "http://example.com/mcp",
        "http://127.0.0.1:1/mcp?token=x",
        "http://user:secret@127.0.0.1:1/mcp",
        "https://127.0.0.1:1/mcp",
    ],
)
async def test_relay_refuses_unverified_endpoint(url):
    from telegram_mcp import bridge

    with pytest.raises(bridge.BridgeError, match="verified loopback"):
        await bridge.relay(url)


@pytest.mark.anyio
async def test_external_cancellation_shields_cleanup_and_reaps_pumps(monkeypatch):
    from telegram_mcp import bridge

    ready, finished = anyio.Event(), anyio.Event()
    closed, scopes = [], []
    send, read = anyio.create_memory_object_stream(0)
    write, receive = anyio.create_memory_object_stream(0)

    @asynccontextmanager
    async def stdio():
        try:
            yield read, write
        finally:
            closed.append("stdio")

    @asynccontextmanager
    async def transport(_url, **_kwargs):
        try:
            ready.set()
            yield receive, write
        finally:
            await anyio.lowlevel.checkpoint()
            closed.append("http")

    async def connector():
        with anyio.CancelScope() as scope:
            scopes.append(scope)
            try:
                await bridge.relay("http://127.0.0.1:12345/mcp")
            finally:
                finished.set()

    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", transport)
    with anyio.fail_after(2):
        async with anyio.create_task_group() as group:
            group.start_soon(connector)
            await ready.wait()
            scopes[0].cancel()
            await finished.wait()
    assert closed == ["http", "stdio"]
    await send.aclose()


@pytest.mark.anyio
async def test_replaced_listener_refused_before_initialize_leaves_process(monkeypatch):
    from telegram_mcp import bridge

    send, read = anyio.create_memory_object_stream(1)
    remote_send, remote_read = anyio.create_memory_object_stream(1)
    output, output_read = anyio.create_memory_object_stream(1)
    seen = []

    @asynccontextmanager
    async def stdio():
        yield read, output

    @asynccontextmanager
    async def transport(_url, **_kwargs):
        yield output_read, remote_send

    def verify():
        seen.append("verified")
        raise RuntimeError("private-owner-canary")

    await send.send(
        frame(
            {
                "jsonrpc": "2.0",
                "id": "init",
                "method": "initialize",
                "params": {"capabilities": {"roots": {}}},
            }
        )
    )
    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", transport)
    with anyio.fail_after(2):
        with pytest.raises(bridge.BridgeError) as error:
            await bridge.relay("http://127.0.0.1:12345/mcp", verify_endpoint=verify)
    assert "private-owner-canary" not in str(error.value)
    assert seen == ["verified"]
    with pytest.raises(anyio.WouldBlock):
        remote_read.receive_nowait()


@pytest.mark.anyio
async def test_sdk_stdio_eof_exits_without_hanging_writer(monkeypatch):
    from telegram_mcp import bridge
    from mcp.server.stdio import stdio_server as sdk_stdio

    empty_input = anyio.wrap_file(io.StringIO(""))
    wire_output = io.StringIO()
    closed = []
    upstream_send, upstream_read = anyio.create_memory_object_stream(0)
    downstream_write, downstream_read = anyio.create_memory_object_stream(0)

    @asynccontextmanager
    async def stdio():
        async with sdk_stdio(stdin=empty_input, stdout=anyio.wrap_file(wire_output)) as streams:
            yield streams

    @asynccontextmanager
    async def transport(_url, **_kwargs):
        try:
            yield upstream_read, downstream_write
        finally:
            await anyio.lowlevel.checkpoint()
            closed.append(True)

    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", transport)
    with anyio.fail_after(2):
        await bridge.relay("http://127.0.0.1:12345/mcp")
    assert closed == [True]
    assert wire_output.getvalue() == ""
    await upstream_send.aclose()
    await downstream_read.aclose()


@pytest.mark.parametrize("scenario", ["http_failure", "cancel"])
def test_real_open_stdin_pipe_is_reaped_on_failure_and_cancel(tmp_path, scenario):
    root = Path(__file__).resolve().parents[1]
    code = """
import sys
from contextlib import asynccontextmanager
import anyio
from telegram_mcp import bridge
from telegram_mcp.bridge_stdio import stdio_server
@asynccontextmanager
async def remote(*args, **kwargs):
    send, read = anyio.create_memory_object_stream(0)
    write, receive = anyio.create_memory_object_stream(0)
    if sys.argv[1] == "http_failure":
        await send.aclose()
    try:
        yield read, write
    finally:
        await anyio.lowlevel.checkpoint()
bridge.streamable_http_client = remote
async def main():
    if sys.argv[1] == "cancel":
        with anyio.move_on_after(0.1):
            await bridge.relay("http://127.0.0.1:12345/mcp")
    else:
        try:
            await bridge.relay("http://127.0.0.1:12345/mcp")
        except bridge.BridgeError:
            pass
anyio.run(main)
print("pipe-reaped", file=sys.stderr)
"""
    env = {key: value for key, value in os.environ.items() if not key.startswith("TELEGRAM_")}
    env["PYTHONPATH"] = str(root)
    process = subprocess.Popen(
        [sys.executable, "-c", code, scenario],
        cwd=tmp_path,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    try:
        # Deliberately keep stdin open: communicate() would close it and hide the bug.
        process.wait(timeout=4)
        assert process.returncode == 0, process.stderr.read().decode("utf-8")
        assert process.stderr.read().decode("utf-8").splitlines() == ["pipe-reaped"]
        assert process.stdout.read() == b""
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()


@pytest.mark.anyio
async def test_modern_cancel_aborts_post_without_forwarding_cancel_notification(monkeypatch):
    from telegram_mcp import bridge
    from mcp.client.streamable_http import streamable_http_client as sdk_http
    from mcp_types.version import MODERN_PROTOCOL_VERSIONS

    version = next(iter(MODERN_PROTOCOL_VERSIONS))
    started, aborted = anyio.Event(), anyio.Event()
    methods = []
    send, read = anyio.create_memory_object_stream(0)
    write, output = anyio.create_memory_object_stream(0)

    async def server(request):
        if request.method == "DELETE":
            return httpx2.Response(204)
        body = json.loads(request.content)
        methods.append(body.get("method"))
        if body.get("method") == "initialize":
            return httpx2.Response(
                200,
                json={
                    "jsonrpc": "2.0",
                    "id": "init",
                    "result": {
                        "protocolVersion": version,
                        "capabilities": {},
                        "serverInfo": {"name": "synthetic", "version": "1"},
                    },
                },
                headers={"Mcp-Session-Id": "synthetic"},
            )
        if body.get("method") == "tools/call":
            started.set()
            try:
                await anyio.sleep_forever()
            finally:
                aborted.set()
        return httpx2.Response(200, json={"jsonrpc": "2.0", "id": "ping", "result": {}})

    @asynccontextmanager
    async def transport(url, *, http_client, **kwargs):
        http_client._transport = httpx2.MockTransport(server)
        async with sdk_http(url, http_client=http_client, **kwargs) as streams:
            yield streams

    @asynccontextmanager
    async def stdio():
        yield read, write

    async def desktop():
        await send.send(frame({"jsonrpc": "2.0", "id": "init", "method": "initialize"}))
        await output.receive()
        await send.send(frame({"jsonrpc": "2.0", "id": 7, "method": "tools/call"}))
        await started.wait()
        await send.send(
            frame(
                {"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}
            )
        )
        await aborted.wait()
        await send.send(frame({"jsonrpc": "2.0", "id": "ping", "method": "ping"}))
        assert (await output.receive()).message.id == "ping"
        await send.aclose()

    monkeypatch.setattr(bridge, "stdio_server", stdio)
    monkeypatch.setattr(bridge, "streamable_http_client", transport)
    with anyio.fail_after(2):
        async with anyio.create_task_group() as group:
            group.start_soon(desktop)
            await bridge.relay("http://127.0.0.1:12345/mcp")
    assert methods == ["initialize", "tools/call", "ping"]
