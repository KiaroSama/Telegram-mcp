"""Private, per-run account-console diagnostics; authentication values never enter it."""

from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
import os
import sys

from telegram_mcp.owner_only import restrict_to_owner_strict as restrict_to_owner


@contextmanager
def session_log(component):
    handle = None
    try:
        base = os.getenv("XDG_STATE_HOME") or Path.home() / ".local" / "state"
        folder = Path(base) / "telegram-mcp" / "logs"
        folder.mkdir(parents=True, exist_ok=True)
        if not restrict_to_owner(folder):
            raise OSError("Private log directory is unavailable.")
        stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d_%H-%M-%S_UTC")
        for n in range(100):
            suffix = "" if n == 0 else f"-{n}"
            try:
                path = folder / f"{component}_{stamp}{suffix}.log"
                handle = path.open("x", encoding="utf-8")
                if not restrict_to_owner(path):
                    raise OSError("Private log file is unavailable.")
                break
            except FileExistsError:
                continue
        if handle is None:
            raise OSError("Log collision limit reached.")
    except Exception:
        if handle is not None:
            handle.close()
            handle = None
        print("Private file logging is unavailable; diagnostics use stderr.", file=sys.stderr)

    def event(level, message):
        # Callers pass fixed stage/status text only, never exceptions or input.
        now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
        line = f"[{now}] [{level}] [{component}] {message}\n"
        if handle is not None:
            try:
                handle.write(line)
                handle.flush()
                return
            except Exception:
                pass
        print(line.rstrip(), file=sys.stderr)

    event("INFO", "Run started")
    try:
        yield event
    except BaseException as error:
        if not isinstance(error, SystemExit) or error.code not in (None, 0):
            event("ERROR", "Run failed or was cancelled")
        raise
    finally:
        event("INFO", "Run ended")
        if handle is not None:
            handle.close()
