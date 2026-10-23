"""Lifetime supervision uses fixed clocks; native descendants are tested separately."""

import os
from types import SimpleNamespace

import pytest

from telegram_mcp import service_supervisor as mod
from telegram_mcp.bridge_process import ProcessOwnershipError


@pytest.mark.skipif(os.name != "nt", reason="Windows scheduling inheritance")
def test_supervisor_normalizes_scheduled_background_priority():
    import subprocess
    import sys

    code = """
import ctypes
from ctypes import wintypes as w
from telegram_mcp import service_supervisor as mod
k = ctypes.WinDLL('kernel32', use_last_error=True)
k.GetCurrentProcess.restype = w.HANDLE
k.SetPriorityClass.argtypes = [w.HANDLE, w.DWORD]
k.GetPriorityClass.argtypes = [w.HANDLE]
k.GetPriorityClass.restype = w.DWORD
assert k.SetPriorityClass(k.GetCurrentProcess(), 0x4000)
mod._normal_priority()
assert k.GetPriorityClass(k.GetCurrentProcess()) == 32
"""
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True,
        encoding="utf-8",
        timeout=15,
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.skipif(os.name != "nt", reason="Windows scheduling API")
@pytest.mark.parametrize("set_result,readback", [(False, 32), (True, 0x4000)])
def test_priority_failure_is_not_reported_as_ready(monkeypatch, set_result, readback):
    import ctypes

    class Function:
        def __init__(self, result):
            self.result = result

        def __call__(self, *_args):
            return self.result

    kernel = SimpleNamespace(
        GetCurrentProcess=Function(1),
        SetPriorityClass=Function(set_result),
        GetPriorityClass=Function(readback),
    )
    monkeypatch.setattr(ctypes, "WinDLL", lambda *_a, **_kw: kernel)
    with pytest.raises(OSError, match="could not be verified"):
        mod._normal_priority()


def test_pending_manual_launcher_blocks_replacement_before_listener(tmp_path, monkeypatch):
    from contextlib import contextmanager

    stop = ClockStop(200)
    monkeypatch.setattr(mod.time, "monotonic", lambda: stop.now)
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)
    launches = []
    attempts = []

    @contextmanager
    def reservation(_root):
        attempts.append(stop.now)
        yield stop.now >= 60

    def launch(*_a):
        assert stop.now >= 60, "Supervisor raced a pending manual launcher"
        launches.append(stop.now)
        stop.stopped = True
        return SimpleNamespace(cleanup=lambda **_kw: None)

    monkeypatch.setattr(mod, "launch_reservation", reservation, raising=False)
    monkeypatch.setattr(mod, "launch", launch)
    mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", lambda *_a: None, stop)
    assert launches == [60]
    assert len(attempts) == 2


def test_unconfirmed_cleanup_cannot_launch_overlapping_generation(tmp_path, monkeypatch):
    calls = []
    stop = SimpleNamespace(is_set=lambda: False)
    clock = [0.0]

    def wait(seconds):
        clock[0] += seconds
        return False

    stop.wait = wait
    child = SimpleNamespace(poll=lambda: 1)

    def cleanup(timeout):
        calls.append("cleanup")
        if calls.count("cleanup") == 1:
            raise ProcessOwnershipError("synthetic failure")
        if calls.count("cleanup") == 2:
            calls.append("confirmed")
            stop.is_set = lambda: True

    child.cleanup = cleanup
    monkeypatch.setattr(mod, "launch", lambda *_a: calls.append("launch") or child)
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)
    monkeypatch.setattr(mod.time, "monotonic", lambda: clock[0])
    mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", lambda *_a: None, stop)
    assert calls == ["launch", "cleanup", "cleanup", "confirmed"]


class ClockStop:
    def __init__(self, end):
        self.now, self.end = 0.0, end
        self.stopped = False

    def is_set(self):
        return self.stopped or self.now >= self.end

    def wait(self, seconds):
        self.now += seconds
        return self.is_set()


@pytest.mark.parametrize(
    "probes,retire_at",
    [
        ([False, False, False], 5),
        ([False, True, False, False, False], 7),
        ([True] * 8, 9),
    ],
)
def test_grace_threshold_reset_and_shutdown(tmp_path, monkeypatch, probes, retire_at):
    stop = ClockStop(9)
    monkeypatch.setattr(mod.time, "monotonic", lambda: stop.now)
    monkeypatch.setattr(mod, "GRACE_SECONDS", 3)
    monkeypatch.setattr(mod, "PROBE_SECONDS", 1)
    identities = [0]
    launched, retired = [], []
    health = iter(probes)
    child = SimpleNamespace(
        poll=lambda: None,
        owns=lambda pid: pid == 12,
        cleanup=lambda **kw: retired.append(stop.now),
    )

    def identity(*_a):
        identities[0] += 1
        return None if identities[0] == 1 else (12, "birth")

    def probe(_url, *, timeout):
        assert timeout == 10
        assert stop.now >= 3
        return next(health)

    monkeypatch.setattr(mod, "listener_identity", identity)
    monkeypatch.setattr(mod, "windows_listener_pid", lambda _port: 12)
    monkeypatch.setattr(mod, "ready", probe)
    monkeypatch.setattr(mod, "launch", lambda *_a: launched.append(stop.now) or child)
    mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", lambda *_a: None, stop)
    assert launched == [0]
    assert retired == [retire_at]


@pytest.mark.parametrize("foreign", [True, False])
def test_unowned_listener_is_never_adopted_or_killed(tmp_path, monkeypatch, foreign):
    stop = ClockStop(3)
    monkeypatch.setattr(mod, "PROBE_SECONDS", 1)

    def identity(*_a):
        if foreign:
            raise mod.BridgeStartupError("synthetic unknown listener")
        return 12, "separate owner"

    monkeypatch.setattr(mod, "listener_identity", identity)
    monkeypatch.setattr(mod, "launch", lambda *_a: pytest.fail("unowned replacement"))
    mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", lambda *_a: None, stop)


def test_backoff_is_capped_and_every_tree_retires_first(tmp_path, monkeypatch):
    stop = ClockStop(2000)
    monkeypatch.setattr(mod.time, "monotonic", lambda: stop.now)
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)
    pending = [False]
    delays = []

    def launch(*_a):
        assert not pending[0]
        pending[0] = True
        return SimpleNamespace(poll=lambda: 1, cleanup=lambda **kw: pending.__setitem__(0, False))

    def event(_level, message):
        if message.startswith("Restart backoff"):
            delays.append(int(message.rsplit("=", 1)[1]))

    monkeypatch.setattr(mod, "launch", launch)
    mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", event, stop)
    assert delays[:6] == [10, 20, 40, 80, 160, 300]
    assert max(delays) == 300
    assert not pending[0]


def test_lifetime_election_duplicate_and_private_logs(tmp_path, monkeypatch):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state with spaces"))
    monkeypatch.setattr(mod, "checkout", lambda: tmp_path)
    monkeypatch.setattr(mod, "endpoint_for", lambda _root: "http://127.0.0.1:18765/mcp")
    from telegram_mcp import session_log

    monkeypatch.setattr(session_log, "restrict_to_owner", lambda _p: True)
    keys = []
    calls = []

    class Lease:
        def __init__(self, key, **kw):
            keys.append(key)

        def acquire(self, **kw):
            assert kw["grace_seconds"] == 0
            if len(keys) == 2:
                raise mod.SessionLockError("sensitive canary must not reach log")

        def release(self):
            calls.append("release")

    monkeypatch.setattr(mod, "SessionLock", Lease)
    monkeypatch.setattr(mod.signal, "signal", lambda *_a: None)
    monkeypatch.setattr(mod, "_normal_priority", lambda: calls.append("priority"))
    monkeypatch.setattr(mod, "watch", lambda *_a: calls.append("watch"))
    assert mod.run() == 0
    assert mod.run() == 75
    assert keys[0] == keys[1]
    assert calls == ["priority", "watch", "release"]
    logs = list((tmp_path / "state with spaces/telegram-mcp/logs").glob("supervise_*.log"))
    assert len(logs) == 2
    text = "".join(p.read_text(encoding="utf-8") for p in logs)
    assert "Supervisor elected" in text and "Another supervisor" in text
    assert "sensitive canary" not in text
    for path in logs:
        path.rename(path.with_suffix(".closed"))


def test_native_descendant_lease_retires_before_one_replacement(tmp_path, monkeypatch):
    import sys
    from telegram_mcp.bridge_process import launch_owned
    from telegram_mcp.singleton import SessionLock
    from test_bridge_process import _wait_file, _alive

    ready = tmp_path / "descendant.pid"
    lock_dir = tmp_path / "leases"
    code = (
        "import os,time;from pathlib import Path;"
        "from telegram_mcp.singleton import SessionLock;"
        f"lease=SessionLock('synthetic-service',lock_dir=Path({str(lock_dir)!r}));"
        "lease.acquire(grace_seconds=0);"
        f"Path({str(ready)!r}).write_text(str(os.getpid()),encoding='utf-8');time.sleep(60)"
    )
    parent = "import subprocess,sys;subprocess.Popen([sys.executable,'-c',%r])" % code
    unrelated = launch_owned([sys.executable, "-c", "import time;time.sleep(60)"])
    stop = ClockStop(100)
    generations = []
    first = [None]
    lease = SessionLock("synthetic-service", lock_dir=lock_dir)

    def launch(*_a):
        if not generations:
            owned = launch_owned([sys.executable, "-c", parent])
            first[0] = owned
            generations.append(owned)
            pid = _wait_file(ready, timeout=10)
            owned.process.wait(timeout=10)
            assert _alive(pid)
            assert owned.owns(pid)
            with pytest.raises(mod.SessionLockError):
                lease.acquire(grace_seconds=0)
            return owned
        assert first[0].released
        assert not _alive(int(ready.read_text(encoding="utf-8")))
        lease.acquire(grace_seconds=0)
        lease.release()
        assert unrelated.poll() is None
        replacement = launch_owned([sys.executable, "-c", "import time;time.sleep(60)"])
        generations.append(replacement)
        stop.stopped = True
        return replacement

    monkeypatch.setattr(mod, "launch", launch)
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)
    try:
        mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", lambda *_a: None, stop)
        assert len(generations) == 2
        assert all(p.released for p in generations)
        assert unrelated.poll() is None
    finally:
        lease.release()
        for owned in generations:
            owned.cleanup(timeout=10)
        unrelated.cleanup(timeout=10)


def test_failed_launch_boundary_is_retired_before_retry(tmp_path, monkeypatch):
    calls = []
    stop = ClockStop(100)
    child = SimpleNamespace(cleanup=lambda **kw: calls.append("confirmed"))

    def launch(*_a):
        calls.append("launch")
        if calls.count("launch") == 1:
            raise ProcessOwnershipError("controlled launch failure", owned=child)
        assert calls == ["launch", "confirmed", "launch"]
        stop.stopped = True
        return SimpleNamespace(cleanup=lambda **kw: calls.append("retired"))

    monkeypatch.setattr(mod, "launch", launch)
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)
    mod.watch(tmp_path, "http://127.0.0.1:18765/mcp", lambda *_a: None, stop)
    assert calls == ["launch", "confirmed", "launch", "retired"]


def test_cli_supervise_dispatch_is_transport_only(monkeypatch):
    import sys
    from telegram_mcp import cli, install_guard

    monkeypatch.setattr(install_guard, "assert_safe_distribution", lambda: None)
    monkeypatch.setattr(sys, "argv", ["telegram-mcp", "supervise"])
    monkeypatch.setattr(mod, "run", lambda: 75)
    with pytest.raises(SystemExit) as error:
        cli.main()
    assert error.value.code == 75
    monkeypatch.setattr(sys, "argv", ["telegram-mcp", "supervise", "extra"])
    with pytest.raises(SystemExit, match="Usage"):
        cli.main()
