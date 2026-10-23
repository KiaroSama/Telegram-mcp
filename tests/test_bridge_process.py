"""Owned startup trees terminate independently of a competing service."""

import os
import subprocess
import sys
import time
from types import SimpleNamespace

import pytest

from telegram_mcp import bridge_process


@pytest.mark.skipif(os.name != "nt", reason="Windows native TCP owner table")
def test_native_listener_lookup_matches_real_bound_socket():
    import socket

    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        port = listener.getsockname()[1]
        assert bridge_process.windows_listener_pid(port) == os.getpid()
    assert bridge_process.windows_listener_pid(port) is None


@pytest.mark.skipif(os.name != "nt", reason="Windows scheduling inheritance")
def test_owned_launch_does_not_inherit_background_priority():
    code = """
import ctypes, subprocess, sys
from ctypes import wintypes as w
from telegram_mcp.bridge_process import launch_owned
k = ctypes.WinDLL('kernel32', use_last_error=True)
k.GetCurrentProcess.restype = w.HANDLE
k.SetPriorityClass.argtypes = [w.HANDLE, w.DWORD]
k.GetPriorityClass.argtypes = [w.HANDLE]
k.GetPriorityClass.restype = w.DWORD
assert k.SetPriorityClass(k.GetCurrentProcess(), 0x4000)
child = launch_owned([sys.executable, '-c',
    'import ctypes; from ctypes import wintypes as w; '
    'k=ctypes.WinDLL("kernel32"); k.GetCurrentProcess.restype=w.HANDLE; '
    'k.GetPriorityClass.argtypes=[w.HANDLE]; '
    'print(k.GetPriorityClass(k.GetCurrentProcess()), flush=True)'],
    stdout=subprocess.PIPE)
try:
    assert child.process.wait(timeout=10) == 0
    assert child.process.stdout.read().strip() == b'32', 'Inherited background priority'
finally:
    child.cleanup(timeout=5)
"""
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True,
        encoding="utf-8",
        timeout=20,
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    assert result.returncode == 0, result.stderr


def _wait_file(path, timeout=3):
    deadline = time.monotonic() + timeout
    while True:
        if path.exists():
            value = path.read_text(encoding="utf-8")
            if value.isdecimal():
                return int(value)
        if time.monotonic() >= deadline:
            raise AssertionError("owned child did not signal readiness")
        time.sleep(0.01)


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


def test_cleanup_failure_retains_boundary_until_confirmed_retry(monkeypatch):
    process = SimpleNamespace(pid=123, poll=lambda: None, wait=lambda **kw: None)
    owned = bridge_process.OwnedProcess(process)
    monkeypatch.setattr(bridge_process.os, "name", "posix")
    releases = []
    monkeypatch.setattr(owned, "_release", lambda: releases.append("released"))
    monkeypatch.setattr(bridge_process.signal, "SIGKILL", 9, raising=False)
    monkeypatch.setattr(bridge_process.os, "killpg", lambda *_a: None, raising=False)
    active = iter([True, False])
    monkeypatch.setattr(bridge_process, "_active_posix", lambda _pid: next(active))
    clock = iter([0, 1, 2])
    monkeypatch.setattr(bridge_process.time, "monotonic", lambda: next(clock))
    with pytest.raises(bridge_process.ProcessOwnershipError, match="time limit"):
        owned.cleanup(timeout=0.5)
    assert releases == []
    assert not owned.released
    monkeypatch.setattr(bridge_process.time, "monotonic", lambda: 2)
    owned.cleanup(timeout=0.5)
    assert releases == ["released"]


def test_bootstrap_handoff_retains_inner_generation_boundary(tmp_path):
    ready = tmp_path / "nested.pid"
    stop = tmp_path / "stop"
    code = (
        "import sys,time;from pathlib import Path;"
        "from telegram_mcp.bridge_process import launch_owned;"
        "child=launch_owned([sys.executable,'-c','import time;time.sleep(60)']);"
        f"Path({str(ready)!r}).write_text(str(child.pid),encoding='utf-8');"
        f"stop=Path({str(stop)!r});"
        "exec('while not stop.exists():\\n time.sleep(0.01)');"
        "child.cleanup(timeout=5)"
    )
    bootstrap = bridge_process.launch_owned([sys.executable, "-c", code])
    pid = None
    try:
        pid = _wait_file(ready, timeout=10)
        assert bootstrap.owns_descendant(pid)
        bootstrap.detach()
        assert _alive(bootstrap.pid) and _alive(pid)
        if os.name == "nt":
            # Retained inner job's last handle closes on supervisor death.
            import ctypes
            from ctypes import wintypes

            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.OpenProcess.restype = wintypes.HANDLE
            kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
            handle = kernel.OpenProcess(0x100001, False, bootstrap.pid)
            assert handle
            try:
                kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
                assert kernel.TerminateProcess(handle, 1)
                kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
                assert kernel.WaitForSingleObject(handle, 5000) == 0
            finally:
                kernel.CloseHandle.argtypes = [wintypes.HANDLE]
                kernel.CloseHandle(handle)
        else:
            stop.write_text("stop", encoding="utf-8")
            bootstrap.process.wait(timeout=5)
        deadline = time.monotonic() + 5
        while _alive(pid) or _alive(bootstrap.pid):
            assert time.monotonic() < deadline
            time.sleep(0.01)
    finally:
        stop.write_text("stop", encoding="utf-8")
        if not bootstrap.released:
            bootstrap.cleanup(timeout=5)
        if os.name != "nt":
            bootstrap.process.wait(timeout=5)


def test_abnormal_supervisor_exit_does_not_certify_inner_retirement(monkeypatch):
    monkeypatch.setattr(bridge_process.os, "name", "posix")
    process = SimpleNamespace(pid=123, poll=lambda: -9, wait=lambda **kw: -9)
    owned = bridge_process.OwnedProcess(process)
    monkeypatch.setattr(owned, "cleanup", lambda **kw: pytest.fail("cannot certify inner exit"))
    with pytest.raises(bridge_process.ProcessOwnershipError, match="unconfirmed"):
        owned.cleanup_supervisor(timeout=1)
    assert not owned.released


@pytest.mark.skipif(os.name == "nt", reason="Linux owner-mediated separate process groups")
def test_failed_bootstrap_shutdown_retires_separate_generation(tmp_path):
    ready = tmp_path / "generation.pid"
    code = (
        "import sys,time,signal,threading;from pathlib import Path;"
        "from telegram_mcp.bridge_process import launch_owned;"
        "stop=threading.Event();signal.signal(signal.SIGTERM,lambda *_a:stop.set());"
        "child=launch_owned([sys.executable,'-c','import time;time.sleep(60)']);"
        f"Path({str(ready)!r}).write_text(str(child.pid),encoding='utf-8');"
        "stop.wait(30);child.cleanup(timeout=5)"
    )
    bootstrap = bridge_process.launch_owned([sys.executable, "-c", code])
    try:
        pid = _wait_file(ready, timeout=10)
        assert not bootstrap.owns(pid)
        assert bootstrap.owns_descendant(pid)
        bootstrap.cleanup_supervisor(timeout=10)
        assert bootstrap.released
        assert not _alive(pid)
    finally:
        bootstrap.cleanup_supervisor(timeout=10)


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
