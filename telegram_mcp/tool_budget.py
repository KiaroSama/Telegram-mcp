"""A ceiling on one MCP tool call, so a wedged request fails instead of hanging.

Adopted from the upstream project (chigwell/telegram-mcp, "bound MCP tool calls"),
adapted rather than copied: that version swaps the `CallToolRequest` handler on
`FastMCP`, and this server runs the MCP SDK's `MCPServer`, where the supported
seam is the middleware chain. Middleware is also the better fit - it sees the
result whatever produced it, rather than only the one handler a swap replaced.

The problem it solves is real here too. Every individual Telegram call this
server makes is bounded somewhere, but the TOOL CALL as a whole was not: a
request that wedges below all of those - a reconnect that never settles, a
native library that stops answering - left the client waiting with no diagnostic
until its own idle timeout killed the connection. The client then reports a
transport failure for something that was a stalled operation, which sends
whoever is debugging it to the wrong place entirely.

The default sits just above the 50-second ceiling the two event-wait tools use,
so `wait_for_new_message` and `wait_for_settled_message` still return their own
"nothing arrived" answer rather than being cut off by this. Set
`TELEGRAM_TOOL_TIMEOUT_SECONDS` to 0 for a deliberately unbounded session.
"""

from __future__ import annotations

import asyncio
import os
from typing import Optional

# Just above `events.wait_for_*`'s own 50s default, so those tools answer for
# themselves. Below the one minute most clients give up at, so the caller gets
# this message rather than a dead connection.
TOOL_TIMEOUT_SECONDS_DEFAULT: float = 55.0


def tool_timeout_seconds(value: Optional[str] = None) -> Optional[float]:
    """The ceiling for one tool call, or ``None`` for deliberately unbounded.

    An unparseable value falls back to the default rather than raising: this
    runs on every call, and a typo in an environment variable must not make the
    server refuse to answer anything at all.
    """
    raw = os.getenv("TELEGRAM_TOOL_TIMEOUT_SECONDS") if value is None else value
    if not raw:
        return TOOL_TIMEOUT_SECONDS_DEFAULT
    try:
        seconds = float(raw)
    except ValueError:
        return TOOL_TIMEOUT_SECONDS_DEFAULT
    if seconds != seconds or seconds in (float("inf"), float("-inf")):
        # `nan` compares false against everything, so it would read as
        # "unbounded" through the check below while looking like a number.
        return TOOL_TIMEOUT_SECONDS_DEFAULT
    return seconds if seconds > 0 else None


def timed_out_result(seconds: float):
    """The answer a caller gets instead of silence."""
    from mcp.types import CallToolResult, TextContent

    return CallToolResult(
        content=[
            TextContent(
                type="text",
                text=(
                    f"This tool call was stopped after {seconds:g}s because it had not "
                    "answered. Nothing here says whether the Telegram side completed: a "
                    "send or an edit may have happened. Check the chat before retrying, "
                    "and raise TELEGRAM_TOOL_TIMEOUT_SECONDS if the operation genuinely "
                    "needs longer."
                ),
            )
        ],
        is_error=True,
    )


class ToolCallBudget:
    """Middleware that bounds each tool call and reports the stop as an error."""

    async def __call__(self, ctx, call_next):
        seconds = tool_timeout_seconds()
        if seconds is None:
            return await call_next(ctx)
        try:
            return await asyncio.wait_for(call_next(ctx), timeout=seconds)
        except (asyncio.TimeoutError, TimeoutError):
            return timed_out_result(seconds)


def install(server) -> None:
    """Put the budget at the FRONT of the chain, exactly once.

    First, so the ceiling covers every other middleware as well as the tool: a
    budget that only wrapped the innermost handler would not bound work done on
    the way out.
    """
    if any(isinstance(m, ToolCallBudget) for m in server.middleware):
        return
    server.middleware.insert(0, ToolCallBudget())


__all__ = [
    "TOOL_TIMEOUT_SECONDS_DEFAULT",
    "ToolCallBudget",
    "install",
    "timed_out_result",
    "tool_timeout_seconds",
]
