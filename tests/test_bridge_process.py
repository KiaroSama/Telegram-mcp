"""Owned startup trees terminate independently of a competing service."""

import os
import subprocess
import sys
import time

from telegram_mcp import bridge_process


def _wait_file(path, timeout=3):
    deadline = time.monotonic() + timeout
    while not path.exists():
        if time.monotonic() >= deadline:
            raise AssertionError("owned child did not signal readiness")
        time.sleep(0.01)
    return int(path.read_text(encoding="utf-8"))


def _alive(pid):
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        handle = kernel.OpenProcess(0x100000, False, pid)
        if not handle:
            return False
        try:
            return kernel.WaitForSingleObject(handle, 0) == 258
        finally:
            kernel.CloseHandle(handle)
    from pathlib import Path

    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        return stat[stat.rfind(")") + 2 :].split()[0] not in ("Z", "X")
    except FileNotFoundError:
        return False


def test_cleanup_reaps_descendant_after_parent_exits_and_leaves_winner(tmp_path):
    ready = tmp_path / "grandchild.pid"
    child = (
        "import os,time;from pathlib import Path;Path(%r).write_text(str(os.getpid()),encoding='utf-8');time.sleep(60)"
        % str(ready)
    )
    parent = (
        "import subprocess,sys;subprocess.Popen([sys.executable,'-c',%r],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)"
        % child
    )
    winner = subprocess.Popen(
        [sys.executable, "-c", "import time;time.sleep(60)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    owned = bridge_process.launch_owned([sys.executable, "-c", parent])
    try:
        pid = _wait_file(ready)
        deadline = time.monotonic() + 3
        while owned.poll() is None:
            assert time.monotonic() < deadline
            time.sleep(0.01)
        assert _alive(pid)
        assert owned.owns(pid)
        assert not owned.owns(winner.pid)
        owned.cleanup(timeout=3)
        assert not _alive(pid)
        assert winner.poll() is None
        owned.cleanup(timeout=3)
    finally:
        owned.cleanup(timeout=3)
        winner.kill()
        winner.wait(timeout=2)


def test_detach_keeps_published_service_alive_and_releases_wrapper(tmp_path):
    ready = tmp_path / "published.pid"
    code = (
        "import os,time;from pathlib import Path;Path(%r).write_text(str(os.getpid()),encoding='utf-8');time.sleep(60)"
        % str(ready)
    )
    owned = bridge_process.launch_owned([sys.executable, "-c", code])
    pid = published_pid = owned.pid
    try:
        published_pid = _wait_file(ready)
        assert _alive(published_pid)
        owned.detach()
        assert _alive(pid)
        owned.cleanup(timeout=3)
        assert _alive(pid)
    finally:
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/PID", str(pid), "/T", "/F"],
                capture_output=True,
                timeout=3,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
        else:
            import signal

            os.killpg(pid, signal.SIGKILL)
            # The wrapper deliberately released ownership, but this test still
            # owns its direct child and must reap it rather than leave a zombie.
            owned.process.wait(timeout=2)
        deadline = time.monotonic() + 2
        while _alive(pid) or _alive(published_pid):
            assert time.monotonic() < deadline
            time.sleep(0.01)
