"""Every tool call an agent makes, written down for later diagnosis (spec 018).

The server's own log (``log_setup``) is content-free by design. This one is the owner's
explicit exception: one JSON line per ``tools/call`` with the arguments AS SENT, how the
call ended, how long it took, the whole error and the first 1000 characters of the
result - enough to see exactly what an agent did when something went wrong.

Secrets never land: every value passes ``log_setup.redact`` (session strings, invite
links, bot tokens, configured secret values) and an argument whose name says it is a
secret is replaced outright. The file is owner-only and lives in the state directory,
never beside the installation.

One file per server run, ``agent-commands_YYYY-MM-DD_HH-mm-ss_UTC.log`` under
``<state dir>/command-logs``; files older than 30 days go when a new one is made.
Outermost in the middleware chain, so safeguard refusals and time-budget stops are
recorded with the time the agent actually waited. Writing can fail; the call cannot.
"""

from __future__ import annotations

import json
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

__all__ = ["CommandLog", "install", "log_directory"]

RESULT_HEAD = 1000
RETENTION_DAYS = 30
_PREFIX = "agent-commands_"
_REDACTED = "[REDACTED]"
_SECRET_NAMES = ("password", "token", "secret", "session", "api_hash", "phone_code")
_OUTCOME_PREFIXES = (
    ("SAFEGUARD:", "refused_by_safeguard"),
    ("This tool call was stopped after", "timed_out"),
)


def log_directory() -> Path:
    from telegram_mcp.settings import state_dir

    return state_dir() / "command-logs"


def _redact(value: Any, name: str = "") -> Any:
    if any(marker in name.lower() for marker in _SECRET_NAMES) and value not in (None, ""):
        return _REDACTED
    if isinstance(value, dict):
        return {k: _redact(v, str(k)) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_redact(v) for v in value]
    if isinstance(value, str):
        from telegram_mcp.log_setup import redact

        return redact(value)
    return value


def _result_text(result: Any) -> tuple:
    """``(is_error, text)`` of a CallToolResult, bare or wrapped in a ServerResult."""
    target = result if hasattr(result, "content") else getattr(result, "root", None)
    blocks = getattr(target, "content", None) or []
    text = "\n".join(getattr(b, "text", "") or f"<{getattr(b, 'type', 'block')}>" for b in blocks)
    return bool(getattr(target, "is_error", False)), text


def _client(ctx) -> Optional[str]:
    params = getattr(getattr(ctx, "session", None), "client_params", None)
    info = getattr(params, "client_info", None)
    if info is None:
        return None
    return " ".join(
        str(p) for p in (getattr(info, "name", None), getattr(info, "version", None)) if p
    )


class CommandLog:
    """Middleware: record each tool call as one JSON line, never affecting it."""

    def __init__(self, directory: Optional[Path] = None) -> None:
        self._directory = Path(directory) if directory is not None else None
        self._path: Optional[Path] = None
        self._lock = threading.Lock()
        self._warned = False

    def _open_path(self) -> Path:
        if self._path is None:
            from telegram_mcp.aliases import restrict_to_owner

            directory = self._directory or log_directory()
            directory.mkdir(parents=True, exist_ok=True)
            restrict_to_owner(directory)
            cutoff = time.time() - RETENTION_DAYS * 24 * 3600
            for old in directory.glob(f"{_PREFIX}*_UTC.log"):
                if old.stat().st_mtime < cutoff:
                    old.unlink(missing_ok=True)
            stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d_%H-%M-%S")
            path = directory / f"{_PREFIX}{stamp}_UTC.log"
            path.touch()
            restrict_to_owner(path)
            self._path = path
        return self._path

    def _write(self, record: dict) -> None:
        try:
            line = json.dumps(record, ensure_ascii=False, default=str)
            with self._lock:
                with open(self._open_path(), "a", encoding="utf-8") as out:
                    out.write(line + "\n")
        except Exception as error:  # the call already ran; only its record is lost
            if not self._warned:
                self._warned = True
                print(
                    f"WARNING: agent command log unavailable: {type(error).__name__}",
                    file=sys.stderr,
                )

    async def __call__(self, ctx, call_next):
        params = getattr(ctx, "params", None)
        if getattr(ctx, "method", None) != "tools/call" or not params:
            return await call_next(ctx)
        arguments = params.get("arguments") or {}
        record = {
            "time": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ"),
            "request_id": getattr(ctx, "request_id", None),
            "client": _client(ctx),
            "tool": params.get("name"),
            "account": arguments.get("account") if isinstance(arguments, dict) else None,
            "arguments": _redact(arguments),
        }
        started = time.perf_counter()
        try:
            result = await call_next(ctx)
        except BaseException as error:
            record["outcome"] = "exception"
            record["error"] = _redact(f"{type(error).__name__}: {error}")
            raise
        else:
            is_error, text = _result_text(result)
            text = _redact(text)
            if is_error:
                record["outcome"] = next(
                    (o for prefix, o in _OUTCOME_PREFIXES if text.startswith(prefix)), "error"
                )
                record["error"] = text
            else:
                record["outcome"] = "ok"
                record["result"] = text[:RESULT_HEAD]
            return result
        finally:
            record["duration_ms"] = int((time.perf_counter() - started) * 1000)
            self._write(record)


def install(server) -> None:
    """Put the recorder FIRST in the chain, exactly once - outside the safeguard."""
    if any(isinstance(m, CommandLog) for m in server.middleware):
        return
    server.middleware.insert(0, CommandLog())
