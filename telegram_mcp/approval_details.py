"""Extra lines a tool can add to its own approval request (spec 033).

The approval already names the tool. A tool whose effect a bare name cannot show - which
admin rights are about to be granted, which permissions set - registers a function here
that turns the call's arguments into one more line, shown right under "Tool:" in every
channel (client dialog, approval bot, Saved Messages).

``register(tool, fn)``: ``fn(arguments)`` returns the line, or ``None``/"" for no line. It
may be sync or async. A function that raises or takes too long never blocks the question:
the owner is asked with a line saying the detail could not be listed.
"""

import asyncio
import inspect
import logging
from typing import Any, Callable, Dict, Optional

from telegram_mcp.safe_log import log_event

__all__ = ["detail_for", "register"]

_DETAILS: Dict[str, Callable[[Dict[str, Any]], Any]] = {}

# A detail may look a chat up; the approval must not wait on a stalled Telegram call.
_TIMEOUT_SECONDS = 10.0
_UNAVAILABLE = "details: could not be listed"
_MAX_CHARS = 500


def register(tool: str, fn: Callable[[Dict[str, Any]], Any]) -> None:
    _DETAILS[tool] = fn


async def detail_for(tool: str, arguments: Dict[str, Any]) -> str:
    """The extra approval line for this call, or "" when the tool registered none."""
    fn = _DETAILS.get(tool)
    if fn is None:
        return ""
    try:
        line: Optional[str] = fn(arguments)
        if inspect.isawaitable(line):
            line = await asyncio.wait_for(line, _TIMEOUT_SECONDS)
    except Exception as error:  # includes the timeout: still ask, say what is missing
        log_event(logging.WARNING, "approval_detail_failed", tool=tool, error=type(error).__name__)
        return _UNAVAILABLE
    # One bounded line: the approval text must stay under Telegram's message limit (review L1).
    text = " ".join(str(line).split()) if line else ""
    return text if len(text) <= _MAX_CHARS else text[: _MAX_CHARS - 3] + "..."
