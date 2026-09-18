"""The process-global half of TDLib: one binary, one receive thread.

Everything here is per-PROCESS, not per-client, which is why it is not on
``TDLibClient``:

* ``tdjson`` is one shared library handle. It is loaded on first use, because a
  server whose operator never wanted secret chats never installed it and must
  still start.
* ``td_receive`` returns the next event for ANY client, tagged with its
  ``@client_id``. So there is exactly one reader thread for the whole process
  and it ROUTES; a per-client reader would steal each other's events.

Splitting it out also keeps the ownership honest: the clients register
themselves here and unregister on release, and nothing else may touch the
routing table.
"""

from __future__ import annotations

import atexit
import json
import logging
import threading
from typing import TYPE_CHECKING, Optional

from telegram_mcp.safe_log import log_event

if TYPE_CHECKING:  # pragma: no cover - typing only
    from telegram_mcp.tdlib import TDLibClient

__all__ = ["TDLibUnavailable", "tdjson_status", "stop_reader"]


class TDLibUnavailable(RuntimeError):
    """The `tdjson` binary is not installed.

    Raised instead of `ImportError` so a tool can answer with the install
    command rather than a traceback about a module nobody mentioned.
    """


def _tdjson():
    """The binary, imported on first use.

    Deferred so that importing this module -- which `tools/secret_chats.py` does
    unconditionally -- cannot break a server whose operator never wanted secret
    chats and never installed the dependency.
    """
    try:
        import tdjson
    except ImportError as exc:  # pragma: no cover - exercised by tdjson_status
        raise TDLibUnavailable(
            "Secret chats need Telegram's own library, which is not installed. "
            "Install it with: pip install tdjson"
        ) from exc
    return tdjson


def tdjson_status() -> dict:
    """Whether secret chats are available, and what to do if not.

    A tool that simply failed would leave the caller unable to tell an absent
    dependency from a broken login, which are fixed in completely different
    places.
    """
    try:
        td = _tdjson()
    except TDLibUnavailable as exc:
        return {"available": False, "reason": str(exc)}
    try:
        version = json.loads(
            td.td_execute(json.dumps({"@type": "getOption", "name": "version"}).encode()).decode()
        ).get("value")
    except Exception as exc:  # pragma: no cover - a broken build, not a missing one
        return {"available": False, "reason": f"tdjson is installed but unusable: {exc}"}
    return {"available": True, "tdlib_version": version}


# --------------------------------------------------------------------------
# The receive loop.
#
# `td_receive` is process-global, not per-client: one call returns the next
# event for ANY client, tagged with its `@client_id`. So there is exactly one
# reader thread for the process, and it routes. A thread rather than a task
# because the call blocks.
# --------------------------------------------------------------------------

_clients: dict[int, "TDLibClient"] = {}
_clients_lock = threading.Lock()
_reader: Optional[threading.Thread] = None

# Set to ask the reader to finish. It is a DAEMON thread blocked in a native
# call, and a daemon thread is killed where it stands at interpreter shutdown -
# inside `td_receive`, that is TDLib's C++ runtime being unwound from under
# itself, which ends the process with `terminate called without an active
# exception` and exit 250. The native lifecycle test found it; nothing had
# started a real client under pytest before.
_reader_stop = threading.Event()


def _quieten(td) -> None:
    """TDLib logs every request and response at its default verbosity.

    Left alone it writes the full text of each message to stderr, which for a
    secret chat means printing the plaintext this module exists to protect.
    """
    td.td_execute(json.dumps({"@type": "setLogVerbosityLevel", "new_verbosity_level": 1}).encode())


def _dispatch(event: dict) -> None:
    client_id = event.get("@client_id")
    with _clients_lock:
        client = _clients.get(client_id)
    if client is None:
        return
    client._handle(event)


def _reader_loop() -> None:  # pragma: no cover - a thread, driven by live TDLib
    td = _tdjson()
    while not _reader_stop.is_set():
        try:
            raw = td.td_receive(1.0)
        except Exception as exc:
            # Through log_event, not the logger: a raw handler would be free to
            # format the event that failed, and a secret chat's event carries
            # the plaintext this module exists to keep out of logs.
            log_event(logging.ERROR, "tdlib_receive_failed", error=exc)
            return
        if not raw:
            continue
        try:
            _dispatch(json.loads(raw.decode()))
        except Exception as exc:
            log_event(logging.ERROR, "tdlib_dispatch_failed", error=exc)


def _ensure_reader() -> None:
    global _reader
    with _clients_lock:
        if _reader is not None and _reader.is_alive():
            return
        _reader_stop.clear()
        _reader = threading.Thread(target=_reader_loop, name="tdlib-receive", daemon=True)
        _reader.start()


def stop_reader(timeout: float = 3.0) -> bool:
    """Ask the receive thread to finish, and wait for it to actually leave TDLib.

    `td_receive` returns on its own within a second, so the flag is seen quickly.
    Waiting matters more than asking: the point is that the thread is OUT of the
    native library before the process ends.
    """
    global _reader
    thread = _reader
    _reader_stop.set()
    if thread is None or not thread.is_alive():
        _reader = None
        return True
    thread.join(timeout)
    stopped = not thread.is_alive()
    if stopped:
        _reader = None
    return stopped


def _stop_reader_at_exit() -> None:  # pragma: no cover - runs at interpreter exit
    stop_reader()


# Registered once, at import. A process that exits with a client still open -
# a crash, a Ctrl+C, a test session - must not leave the daemon thread to be
# killed inside a native call.
atexit.register(_stop_reader_at_exit)
