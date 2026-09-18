"""The watcher follows the FILE, not a prefix that happens to match.

The rotation check compared the first `min(64, offset)` bytes and the length. A
replacement sharing both - which ordinary JSONL records do, because they begin
with the same key in the same order - passed every check and was invisible: the
watcher went on reading at the old offset, so the new file's first records were
skipped and, once it grew past that mark, the wrong bytes were emitted.

Widening the prefix does not fix it; it only moves the collision. What settles
identity on Windows is the file's own NTFS id, read from the handle already
open. Creation time cannot: NTFS tunneling hands a name recreated within about
fifteen seconds the OLD stamp, and a rotation takes far less than that.

These run the REAL command the tool hands to callers, in a REAL PowerShell, on
a REAL file. Skipped where PowerShell is absent - which is every non-Windows
host, and the reason the audit could not execute this one.
"""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

import pytest

from telegram_mcp.tools import feed_lifecycle

pytestmark = pytest.mark.skipif(
    os.name != "nt" or shutil.which("powershell") is None,
    reason="the watcher script is PowerShell, and this host has none",
)

# A record whose first 64 bytes are identical to the next one's, which is what
# every real JSONL feed looks like: the same keys, in the same order.
_PREFIX = '{"at": "2026-09-19T00:00:00Z", "account": "work", "chat_id": 12345'


def _record(marker: str) -> str:
    return json.dumps(
        {
            "at": "2026-09-19T00:00:00Z",
            "account": "work",
            "chat_id": 12345,
            "text": marker,
        }
    )


def _start(path: Path):
    command = feed_lifecycle.watch_command(path)
    # `watch_command` returns the full invocation; run it as the caller would.
    return subprocess.Popen(
        command,
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


class _Watcher:
    """A running watcher, read on its own thread so nothing here blocks.

    The readiness signal matters: the script compiles a small P/Invoke helper
    when it starts, which costs several seconds, so a fixed sleep before acting
    is a race that passes on a warm machine and fails on a cold CI runner - as
    it did. `wait_for` waits for output the watcher has actually produced.
    """

    def __init__(self, process):
        import threading

        self.process = process
        self.lines = []
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._pump, daemon=True)
        self._thread.start()

    def _pump(self):
        for line in self.process.stdout:
            line = line.strip()
            if line:
                with self._lock:
                    self.lines.append(line)

    def seen(self):
        with self._lock:
            return list(self.lines)

    def wait_for(self, marker: str, seconds: float = 40.0) -> bool:
        """Poll for a line containing `marker`, bounded. No blind sleeping."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if any(marker in line for line in self.seen()):
                return True
            time.sleep(0.2)
        return False


@pytest.fixture
def watched(tmp_path):
    path = tmp_path / "feed.jsonl"
    started = []
    yield path, started
    # The WHOLE tree. `shell=True` puts a cmd between this and PowerShell, so
    # killing the handle leaves the watcher polling for the life of the session -
    # which the guarded runner catches as a leak, correctly.
    for process in started:
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            capture_output=True,
            timeout=30,
        )
        try:
            process.wait(timeout=15)
        except Exception:
            process.kill()
        if process.stdout is not None:
            process.stdout.close()


def test_a_same_length_same_prefix_replacement_is_noticed(watched):
    """The counterexample. Both files start with the same 64 bytes and are the
    same length, so the prefix-and-length check saw no change at all."""
    path, started = watched
    first = _record("AAAA") + "\n"
    path.write_text(first, encoding="utf-8")
    assert first[:64] == (_record("BBBB") + "\n")[:64], "the fixture is not the collision"

    process = _start(path)
    started.append(process)
    watcher = _Watcher(process)
    assert watcher.wait_for("AAAA"), "the watcher never started reading the original file"

    # Rotation: the name is replaced by a NEW file of the same length whose first
    # 64 bytes are identical.
    replacement = _record("BBBB") + "\n"
    assert len(replacement) == len(first)
    path.unlink()
    path.write_text(replacement, encoding="utf-8")

    assert watcher.wait_for("BBBB"), "the replacement was invisible to the watcher"


def test_an_ordinary_append_is_not_mistaken_for_a_rotation(watched):
    """The other direction, which the identity check must not break: appending
    must NOT reset the offset and replay what was already emitted."""
    path, started = watched
    path.write_text(_record("FIRST") + "\n", encoding="utf-8")

    process = _start(path)
    started.append(process)
    watcher = _Watcher(process)
    assert watcher.wait_for("FIRST"), "the watcher never started reading"

    with path.open("a", encoding="utf-8") as handle:
        handle.write(_record("SECOND") + "\n")

    assert watcher.wait_for("SECOND"), "the append was not picked up"
    lines = watcher.seen()
    assert len([line for line in lines if "FIRST" in line]) == 1, "the first record replayed"


def test_a_truncation_restarts_from_the_beginning(watched):
    """A file emptied in place keeps its identity, so the LENGTH check is what
    catches it - both checks are still needed."""
    path, started = watched
    path.write_text(_record("BEFORE") + "\n", encoding="utf-8")

    process = _start(path)
    started.append(process)
    watcher = _Watcher(process)
    assert watcher.wait_for("BEFORE"), "the watcher never started reading"

    with path.open("w", encoding="utf-8") as handle:
        handle.write(_record("AFTER") + "\n")

    assert watcher.wait_for("AFTER"), "a truncation lost the new content"


def test_the_script_does_not_decide_identity_by_creation_time():
    """NTFS tunneling hands a name recreated within ~15s the OLD stamp, so a
    creation-time check reads a rotation as the same file. Stated as a source
    assertion because reproducing tunneling in a test is a race by nature."""
    script = feed_lifecycle.watch_script("C:/tmp/feed.jsonl")

    assert "CreationTime" not in script
    assert "queryfileid" in script.lower(), "identity is not read from the filesystem"
