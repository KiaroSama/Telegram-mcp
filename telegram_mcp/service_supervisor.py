"""Lifetime ownership of the shared service, without Telegram runtime imports."""

import os
from pathlib import Path
import signal
import subprocess
import threading
import time
from urllib.parse import urlsplit

from telegram_mcp.bridge_process import ProcessOwnershipError, windows_listener_pid
from telegram_mcp.bridge_startup import (
    BridgeStartupError,
    checkout,
    endpoint_for,
    launch,
    listener_identity,
    ready,
)
from telegram_mcp.session_log import run_log
from telegram_mcp.singleton import SessionLock, SessionLockError

GRACE_SECONDS = 300.0
PROBE_SECONDS = 60.0
FAIL_LIMIT = 3
CLEANUP_SECONDS = 30.0
BACKOFF_MAX = 300.0


def _retire(child, event, stop):
    while True:
        try:
            child.cleanup(timeout=CLEANUP_SECONDS)
            event("INFO", "Owned generation retirement confirmed")
            return
        except (OSError, ProcessOwnershipError, subprocess.SubprocessError):
            event("ERROR", "Owned cleanup unconfirmed; replacement remains blocked")
            # Even shutdown must retain the boundary until confirmed. An already
            # set event cannot pace this retry, so use a bounded interval then.
            if stop.is_set():
                time.sleep(PROBE_SECONDS)
            else:
                stop.wait(PROBE_SECONDS)


def watch(root, url, event, stop):
    port = urlsplit(url).port
    rapid = 0
    while not stop.is_set():
        try:
            occupant = listener_identity(root, port)
        except BridgeStartupError:
            event("WARNING", "Listener ownership unknown; no generation launched")
            stop.wait(PROBE_SECONDS)
            continue
        if occupant is not None:
            event("INFO", "Separately owned listener present; waiting without adoption")
            stop.wait(PROBE_SECONDS)
            continue
        child = None
        born = time.monotonic()
        try:
            child = launch(root, port)
            event("INFO", "Owned generation started")
            misses = 0
            probe_at = born + GRACE_SECONDS
            while not stop.is_set():
                if child.poll() is not None:
                    event("WARNING", "Launcher exited; descendant retirement required")
                    break
                now = time.monotonic()
                if now >= probe_at:
                    try:
                        identity = (
                            (windows_listener_pid(port),)
                            if os.name == "nt"
                            else listener_identity(root, port)
                        )
                        reason = "listener_missing"
                        healthy = False
                        if identity is not None and identity[0] is not None:
                            reason = "outside_owned_generation"
                            if child.owns(identity[0]):
                                reason = "http_unresponsive"
                                if ready(url, timeout=10):
                                    reason = "listener_changed"
                                    current = (
                                        (windows_listener_pid(port),)
                                        if os.name == "nt"
                                        else listener_identity(root, port)
                                    )
                                    healthy = current == identity and child.owns(current[0])
                        if not healthy:
                            event("WARNING", f"Health probe reason={reason}")
                    except (BridgeStartupError, ProcessOwnershipError, OSError):
                        healthy = False
                        event("WARNING", "Health probe reason=ownership_inspection_failed")
                    misses = 0 if healthy else misses + 1
                    event("INFO" if healthy else "WARNING", f"Health probe misses={misses}")
                    if misses >= FAIL_LIMIT:
                        event("WARNING", "Owned service unresponsive; retiring generation")
                        break
                    probe_at = time.monotonic() + PROBE_SECONDS
                stop.wait(min(5.0, max(0.01, probe_at - time.monotonic())))
        except ProcessOwnershipError as error:
            child = error.owned
            event("ERROR", "Owned generation launch cleanup requires confirmation")
            if child is None:
                raise
        except BridgeStartupError:
            event("ERROR", "Owned generation launch failed")
        finally:
            if child is not None:
                _retire(child, event, stop)
        if stop.is_set():
            break
        uptime = time.monotonic() - born
        rapid = rapid + 1 if uptime < GRACE_SECONDS else 0
        delay = min(BACKOFF_MAX, 5.0 * 2 ** min(rapid, 6))
        event("INFO", f"Restart backoff seconds={int(delay)}")
        stop.wait(delay)


def run():
    root = checkout()
    url = endpoint_for(root)
    port = urlsplit(url).port
    state = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    lease = SessionLock(
        f"service-supervisor:{os.path.normcase(str(root.resolve()))}:{port}",
        lock_dir=state / "telegram-mcp/run",
    )
    stop = threading.Event()
    handlers = {}
    with run_log("supervise") as event:
        try:
            lease.acquire(grace_seconds=0, poll_interval=0.25)
        except SessionLockError:
            event("INFO", "Another supervisor owns this checkout and endpoint")
            return 75
        try:
            for number in (signal.SIGINT, signal.SIGTERM):
                handlers[number] = signal.signal(number, lambda *_a: stop.set())
            event("INFO", "Supervisor elected")
            watch(root, url, event, stop)
            event("INFO", "Supervisor shutdown confirmed")
            return 0
        finally:
            for number, handler in handlers.items():
                signal.signal(number, handler)
            lease.release()
