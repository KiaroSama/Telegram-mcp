"""Raw stdio/HTTP relay; this process never owns a Telegram connection."""

from contextlib import contextmanager
from dataclasses import replace
import ipaddress
import logging
from urllib.parse import urlsplit

import anyio
import httpx2
from mcp.client.streamable_http import streamable_http_client
from mcp.shared.inbound import MCP_PROTOCOL_VERSION_HEADER
from mcp.shared.message import ClientMessageMetadata, SessionMessage
from mcp_types import JSONRPCRequest, JSONRPCResponse, jsonrpc_message_adapter

from telegram_mcp.bridge_stdio import stdio_server
from telegram_mcp.session_log import run_log

MAX_FRAME_BYTES = 32 * 1024 * 1024
CLEANUP_SECONDS = 5


class BridgeError(RuntimeError):
    """A controlled transport failure without protocol payloads."""


def _validate_url(url):
    try:
        parsed = urlsplit(url)
        host = ipaddress.ip_address(parsed.hostname or "")
        valid = (
            parsed.scheme == "http"
            and host.is_loopback
            and parsed.port is not None
            and not parsed.username
            and not parsed.password
            and not parsed.query
            and not parsed.fragment
        )
    except (ValueError, TypeError):
        valid = False
    if not valid:
        raise BridgeError("Bridge requires a verified loopback HTTP endpoint.")


@contextmanager
def _quiet_dependencies(fail):
    # SDK DEBUG frames and parser tracebacks contain private message bodies.
    class Quiet(logging.Filter):
        def filter(self, record):
            if not record.name.startswith(("mcp.", "httpx2", "httpcore")):
                return True
            error = record.exc_info[1] if record.exc_info else None
            unsupported_get = (
                isinstance(error, httpx2.HTTPStatusError)
                and error.response.status_code == 405
                and error.request.method == "GET"
            )
            if (
                record.name.startswith("mcp.client.streamable_http")
                and not unsupported_get
                and (record.exc_info or record.levelno >= logging.ERROR)
            ):
                fail("Bridge HTTP transport failed; outstanding calls were not replayed.")
            return False

    quiet = Quiet()
    names = {"mcp", "mcp.client.streamable_http", "httpx2", "httpcore"}
    names.update(
        name
        for name in logging.Logger.manager.loggerDict
        if name.startswith(("mcp.", "httpx2.", "httpcore."))
    )
    saved = []
    handlers = set(logging.getLogger().handlers)
    for name in names:
        logger = logging.getLogger(name)
        saved.append((logger, logger.level))
        handlers.update(logger.handlers)
        logger.addFilter(quiet)
        logger.setLevel(
            logging.DEBUG
            if name == "mcp.client.streamable_http"
            else max(logger.level, logging.WARNING)
        )
    handlers.add(logging.lastResort)
    for handler in handlers:
        if handler is not None:
            handler.addFilter(quiet)
    try:
        yield
    finally:
        for handler in handlers:
            if handler is not None:
                handler.removeFilter(quiet)
        for logger, level in saved:
            logger.removeFilter(quiet)
            logger.setLevel(level)


def _checked(item):
    if not isinstance(item, SessionMessage):
        raise BridgeError("Bridge received an invalid MCP frame.")
    if (
        len(item.message.model_dump_json(by_alias=True, exclude_unset=True).encode("utf-8"))
        > MAX_FRAME_BYTES
    ):
        raise BridgeError("Bridge MCP frame exceeds the 32 MiB limit.")
    return item


class _BoundedBody(httpx2.AsyncByteStream):
    def __init__(self, source, fail, *, limited):
        self.source, self.fail, self.limited = source, fail, limited

    async def __aiter__(self):
        size = 0
        try:
            async for chunk in self.source:
                size += len(chunk)
                if self.limited and size > MAX_FRAME_BYTES:
                    raise BridgeError("Bridge HTTP response exceeds the 32 MiB limit.")
                yield chunk
        except Exception:
            self.fail("Bridge HTTP response failed; outstanding calls were not replayed.")
            raise BridgeError("Bridge HTTP response failed.") from None

    async def aclose(self):
        await self.source.aclose()


async def relay(url: str, *, verify_endpoint=None) -> None:
    """Relay the caller's handshake and all subsequent frames without a session API."""
    _validate_url(url)
    failed = anyio.Event()
    failure = []

    def fail(text):
        if not failure:
            failure.append(text)
            failed.set()

    async def check_response(response):
        if 300 <= response.status_code < 400:
            fail("Bridge refused an HTTP redirect.")
            raise BridgeError("Bridge refused an HTTP redirect.")
        kind = response.headers.get("content-type", "").partition(";")[0].strip().lower()
        response.stream = _BoundedBody(response.stream, fail, limited=kind != "text/event-stream")
        if (
            response.request.method == "POST"
            and response.status_code not in (202, 204)
            and response.status_code < 400
            and kind not in ("application/json", "text/event-stream")
        ):
            fail("Bridge received an invalid HTTP response type.")
            raise BridgeError("Bridge received an invalid HTTP response type.")
        if (
            kind == "application/json"
            and response.request.method == "POST"
            and response.status_code != 202
        ):
            try:
                body = await response.aread()
                _checked(SessionMessage(jsonrpc_message_adapter.validate_json(body)))
            except Exception:
                fail("Bridge received an invalid HTTP MCP frame.")
                raise BridgeError("Bridge received an invalid HTTP MCP frame.") from None

    class GuardedClient(httpx2.AsyncClient):
        async def send(self, request, **kwargs):
            try:
                return await super().send(request, **kwargs)
            except Exception:
                fail("Bridge HTTP connection failed; outstanding calls were not replayed.")
                raise BridgeError("Bridge HTTP connection failed.") from None

    protocol = None
    initialization_id = None

    async def downstream(source, target):
        nonlocal protocol, initialization_id
        async for item in source:
            item = _checked(item)
            message = item.message
            if isinstance(message, JSONRPCResponse) and message.id == initialization_id:
                version = message.result.get("protocolVersion")
                if (
                    not isinstance(version, str)
                    or not version
                    or len(version) > 64
                    or not version.isascii()
                    or any(c not in "0123456789-" for c in version)
                ):
                    raise BridgeError("Bridge received an invalid negotiated protocol version.")
                protocol, initialization_id = version, None
            # SDK parser diagnostics can embed the rejected payload. Its log
            # filter marks the failure before the synthesized error is sent.
            if failure:
                raise BridgeError(failure[0])
            await target.send(item)
        raise BridgeError("Bridge HTTP connection closed; outstanding calls were not replayed.")

    async def upstream(source, target, group):
        nonlocal initialization_id, protocol
        async for item in source:
            item = _checked(item)
            message = item.message
            if isinstance(message, JSONRPCRequest) and message.method == "initialize":
                if verify_endpoint is not None:
                    await anyio.to_thread.run_sync(verify_endpoint)
                initialization_id, protocol = message.id, None
            headers = {}
            if protocol:
                headers[MCP_PROTOCOL_VERSION_HEADER] = protocol
            method = getattr(message, "method", None)
            if method is not None:
                if (
                    len(method) > 256
                    or not method.isascii()
                    or any(ord(c) < 33 or ord(c) > 126 for c in method)
                ):
                    raise BridgeError("Bridge received an invalid MCP method.")
                headers["Mcp-Method"] = method
            metadata = (
                item.metadata
                if isinstance(item.metadata, ClientMessageMetadata)
                else ClientMessageMetadata()
            )
            await target.send(replace(item, metadata=replace(metadata, headers=headers)))
        group.cancel_scope.cancel()

    async def watch_failure(group):
        await failed.wait()
        group.cancel_scope.cancel()

    try:
        with _quiet_dependencies(fail), anyio.CancelScope() as cleanup_scope:
            async with stdio_server() as (stdio_read, stdio_write):
                async with GuardedClient(
                    trust_env=False,
                    follow_redirects=False,
                    timeout=httpx2.Timeout(10, connect=5, read=300, write=10, pool=5),
                    event_hooks={"response": [check_response]},
                ) as client:
                    async with streamable_http_client(
                        url,
                        http_client=client,
                        max_sse_event_size=MAX_FRAME_BYTES,
                    ) as (http_read, http_write):
                        try:
                            async with anyio.create_task_group() as pumps:
                                pumps.start_soon(upstream, stdio_read, http_write, pumps)
                                pumps.start_soon(downstream, http_read, stdio_write)
                                pumps.start_soon(watch_failure, pumps)
                        finally:
                            # Shield an OUTER scope: pushing a scope during SDK task-group
                            # exit violates AnyIO's LIFO CancelScope stack.
                            cleanup_scope.shield = True
                            cleanup_scope.deadline = anyio.current_time() + CLEANUP_SECONDS
                            await stdio_write.aclose()
                            # stdio_server has no cancel-on-normal-exit; abort its reader
                            # when EOF came from HTTP or cancellation rather than stdin.
                            if failure:
                                raise BridgeError(failure[0])
        if cleanup_scope.cancel_called:
            raise BridgeError("Bridge session cleanup exceeded its time limit.")
    except Exception as error:
        if failure:
            raise BridgeError(failure[0]) from None
        if isinstance(error, BridgeError):
            raise
        raise BridgeError(
            "Bridge transport failed; outstanding calls were not replayed."
        ) from None
    if failure:
        raise BridgeError(failure[0])


def run(url: str, *, verify_endpoint=None) -> None:
    """Run the transport-only connector; stdout is reserved for MCP frames."""
    with run_log("bridge") as event:
        event("INFO", "Attaching to verified shared service")
        try:

            async def connect():
                await relay(url, verify_endpoint=verify_endpoint)

            anyio.run(connect)
        except KeyboardInterrupt:
            event("INFO", "Bridge cancelled")
        except Exception:
            event("ERROR", "Bridge transport failed; outstanding calls were not replayed")
            raise BridgeError(
                "Bridge transport failed; outstanding calls were not replayed."
            ) from None
        else:
            event("INFO", "Bridge session closed; shared service unchanged")
