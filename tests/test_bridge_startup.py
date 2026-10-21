import pytest

from telegram_mcp import bridge_startup as mod


def test_cold_bootstrap_launches_supervisor_not_an_authenticated_server(tmp_path, monkeypatch):
    python = tmp_path / ".venv" / ("Scripts/python.exe" if mod.os.name == "nt" else "bin/python")
    python.parent.mkdir(parents=True)
    python.write_bytes(b"synthetic")
    calls = []
    monkeypatch.setattr(mod, "launch_owned", lambda args, **kw: calls.append((args, kw)))
    mod.launch_supervisor(tmp_path, 18765)
    args, options = calls[0]
    assert args == [str(python), "-m", "telegram_mcp.cli", "supervise"]
    assert options["cwd"] == tmp_path
    assert options["env"]["MCP_PORT"] == "18765"
    assert options["env"]["MCP_HOST"] == "127.0.0.1"


@pytest.mark.skipif(mod.os.name != "nt", reason="Windows native identity")
def test_native_identity_does_not_spawn_powershell(tmp_path, monkeypatch):
    import subprocess
    import sys
    import socket

    # The real listener lives in a child whose actual script proves this checkout.
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    script = tmp_path / "main.py"
    script.write_text(
        "import socket,sys; s=socket.socket(); s.bind(('127.0.0.1',int(sys.argv[1])));"
        "s.listen(); print('ready',flush=True); sys.stdin.read()",
        encoding="utf-8",
    )
    child = subprocess.Popen(
        [sys.executable, str(script), str(port)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    try:
        import time

        deadline = time.monotonic() + 20
        while not mod._port_open(port):
            assert child.poll() is None and time.monotonic() < deadline
            time.sleep(0.01)
        monkeypatch.setattr(
            mod.subprocess, "run", lambda *_a, **_k: pytest.fail("PowerShell ownership subprocess")
        )
        first = mod.listener_identity(tmp_path, port)
        from telegram_mcp.windows_identity import process_chain

        with process_chain(first[0]) as chain:
            assert child.pid in {row["pid"] for row in chain}
        assert mod.listener_identity(tmp_path, port) == first
        with pytest.raises(mod.BridgeStartupError, match="ownership"):
            mod.listener_identity(tmp_path / "foreign", port)
    finally:
        child.stdin.close()
        try:
            child.wait(timeout=5)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=2)
            child.stdout.close()


@pytest.mark.skipif(mod.os.name != "nt", reason="Windows named launch reservation")
def test_manual_reservation_is_presence_not_exclusive_lifetime(tmp_path):
    import ctypes
    from ctypes import wintypes
    from telegram_mcp.launch_coordination import launch_reservation, reservation_name

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    kernel.CreateMutexW.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    name = reservation_name(tmp_path)
    first = kernel.CreateMutexW(None, False, name)
    second = None
    assert first
    try:
        # Two manual launchers can hold intent while the newer one retires the old.
        second = kernel.CreateMutexW(None, False, name)
        assert second
        with launch_reservation(tmp_path) as acquired:
            assert not acquired
        kernel.CloseHandle(first)
        first = None
        with launch_reservation(tmp_path) as acquired:
            assert not acquired
    finally:
        for handle in (first, second):
            if handle:
                kernel.CloseHandle(handle)
    with launch_reservation(tmp_path) as acquired:
        assert acquired
        # Creation and initial ownership must be atomic. Another thread's manual
        # opener cannot enter the gate before automatic process creation finishes.
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        import concurrent.futures

        def contender():
            handle = kernel.CreateMutexW(None, False, name)
            try:
                return kernel.WaitForSingleObject(handle, 0)
            finally:
                kernel.CloseHandle(handle)

        with concurrent.futures.ThreadPoolExecutor(1) as pool:
            assert pool.submit(contender).result(timeout=3) == 258


def test_only_loopback_transport_configuration_is_accepted(tmp_path, monkeypatch):
    monkeypatch.delenv("MCP_HOST", raising=False)
    monkeypatch.delenv("MCP_PORT", raising=False)
    (tmp_path / ".env").write_text(
        "\n".join(["MCP_HOST=0.0.0.0", "MCP_PORT=18765", ""]), encoding="utf-8"
    )
    with pytest.raises(mod.BridgeStartupError, match="loopback"):
        mod.endpoint_for(tmp_path)


def test_transport_configuration_is_anchored_to_checkout(tmp_path, monkeypatch):
    monkeypatch.delenv("MCP_HOST", raising=False)
    monkeypatch.delenv("MCP_PORT", raising=False)
    (tmp_path / ".env").write_text(
        "\n".join(["MCP_HOST=127.0.0.1", "MCP_PORT=18765", ""]), encoding="utf-8"
    )
    assert mod.endpoint_for(tmp_path) == "http://127.0.0.1:18765/mcp"


def test_bad_port_is_refused_without_starting(tmp_path, monkeypatch):
    monkeypatch.setenv("MCP_PORT", "0")
    with pytest.raises(mod.BridgeStartupError, match="port"):
        mod.endpoint_for(tmp_path)


def test_cold_start_is_elected_once_and_attaches_to_winner(tmp_path, monkeypatch):
    identities = iter([None, None, (12, "creation"), (12, "creation")])
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: next(identities))
    monkeypatch.setattr(mod, "ready", lambda _url: True)
    calls = []

    class Lease:
        def __init__(self, *_a, **_kw):
            pass

        def acquire(self, **_kw):
            assert _kw["grace_seconds"] == mod.START_SECONDS
            calls.append("acquire")

        def release(self):
            calls.append("release")

    class Child:
        def poll(self):
            return None

        def owns_descendant(self, pid):
            assert pid == 12
            return True

        def detach(self):
            calls.append("detach")

    monkeypatch.setattr(mod, "SessionLock", Lease)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *_a: calls.append("launch") or Child())
    assert mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")
    assert calls == ["acquire", "launch", "detach", "release"]


def test_independent_winner_cleans_losing_launcher_without_detaching(tmp_path, monkeypatch):
    identities = iter([None, None, (24, "winner"), (24, "winner")])
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: next(identities))
    monkeypatch.setattr(mod, "ready", lambda _url: True)
    calls = []

    class Lease:
        def __init__(self, *_a, **_kw):
            pass

        def acquire(self, **_kw):
            pass

        def release(self):
            calls.append("release")

    class Child:
        def owns_descendant(self, pid):
            assert pid == 24
            return False

        def cleanup_supervisor(self, timeout):
            assert timeout == 30
            calls.append("cleanup")

        def detach(self):
            pytest.fail("must not publish the losing tree")

    monkeypatch.setattr(mod, "SessionLock", Lease)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *_a: Child())
    url = "http://127.0.0.1:18765/mcp"
    assert mod.ensure_service(root=tmp_path, url=url) == url
    assert calls == ["cleanup", "release"]


def test_failed_bootstrap_launch_transfers_cleanup_boundary(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)

    class Lease:
        def __init__(self, *_a, **_kw):
            pass

        def acquire(self, **_kw):
            pass

        def release(self):
            calls.append("release")

    child = type("Child", (), {"cleanup_supervisor": lambda self, **kw: calls.append("cleanup")})()

    def launch(*_a):
        raise mod.ProcessOwnershipError("unconfirmed bootstrap", owned=child)

    monkeypatch.setattr(mod, "SessionLock", Lease)
    monkeypatch.setattr(mod, "launch_supervisor", launch)
    with pytest.raises(mod.BridgeStartupError, match="ownership"):
        mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")
    assert calls == ["cleanup", "release"]


def test_identity_is_rechecked_before_private_initialization(tmp_path, monkeypatch):
    url = "http://127.0.0.1:18765/mcp"
    identities = iter([(12, "old"), (12, "old"), (12, "new")])
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: next(identities))
    monkeypatch.setattr(mod, "ready", lambda _url: True)
    mod.ensure_service(root=tmp_path, url=url)
    with pytest.raises(mod.BridgeStartupError, match="ownership changed"):
        mod.verify_service(url)


def test_changed_listener_identity_is_refused(tmp_path, monkeypatch):
    identities = iter([(12, "old"), (12, "new")])
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: next(identities))
    monkeypatch.setattr(mod, "ready", lambda _url: True)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *_a: pytest.fail("must not replace"))
    with pytest.raises(mod.BridgeStartupError, match="not ready"):
        mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")


def test_failed_cold_readiness_settles_only_its_owned_launcher(tmp_path, monkeypatch):
    monkeypatch.setattr(mod, "listener_identity", lambda *_a: None)
    monkeypatch.setattr(mod, "START_SECONDS", 0)
    closed = []

    class Lease:
        def __init__(self, *_a, **_kw):
            pass

        def acquire(self, **_kw):
            pass

        def release(self):
            closed.append("lease")

    child = object()
    monkeypatch.setattr(mod, "SessionLock", Lease)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *_a: child)
    monkeypatch.setattr(mod, "_settle_failed_start", lambda process: closed.append(process))
    with pytest.raises(mod.BridgeStartupError, match="timed out"):
        mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")
    assert closed == [child, "lease"]


@pytest.mark.parametrize("interrupted", [False, True])
def test_startup_cleanup_failure_retains_election(tmp_path, monkeypatch, interrupted):
    calls = []
    monkeypatch.setattr(mod, "START_SECONDS", 0)

    class Lease:
        def __init__(self, *_a, **_kw):
            pass

        def acquire(self, **_kw):
            pass

        def release(self):
            calls.append("release")

    class Child:
        def cleanup_supervisor(self, timeout):
            calls.append("cleanup")
            raise mod.ProcessOwnershipError("synthetic cleanup failure")

    def identity(*_args):
        if interrupted and calls == ["launch"]:
            raise KeyboardInterrupt
        return None

    monkeypatch.setattr(mod, "listener_identity", identity)
    monkeypatch.setattr(mod, "SessionLock", Lease)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *_a: calls.append("launch") or Child())
    if interrupted:
        monkeypatch.setattr(mod, "START_SECONDS", 1)
    with pytest.raises(mod.BridgeStartupError, match="cleanup is unconfirmed"):
        mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")
    assert calls == ["launch", "cleanup"]
    assert mod._unconfirmed_starts[-1][1].__class__ is Child
    mod._unconfirmed_starts.pop()


def test_verified_running_service_is_reused(tmp_path, monkeypatch):
    monkeypatch.setattr(mod, "listener_identity", lambda root, port: (12, "creation"))
    monkeypatch.setattr(mod, "ready", lambda url: True)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *a: pytest.fail("must not launch"))
    assert (
        mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")
        == "http://127.0.0.1:18765/mcp"
    )


def test_unknown_occupant_is_never_replaced(tmp_path, monkeypatch):
    def unknown(root, port):
        raise mod.BridgeStartupError("Unknown listener ownership")

    monkeypatch.setattr(mod, "listener_identity", unknown)
    monkeypatch.setattr(mod, "launch_supervisor", lambda *a: pytest.fail("must not launch"))
    with pytest.raises(mod.BridgeStartupError, match="ownership"):
        mod.ensure_service(root=tmp_path, url="http://127.0.0.1:18765/mcp")


@pytest.mark.skipif(mod.os.name != "nt", reason="Windows command-line ownership contract")
@pytest.mark.parametrize("inert", ["-c", "-m"])
def test_inert_checkout_argument_does_not_prove_ownership(tmp_path, inert):
    command = mod.subprocess.list2cmdline(
        [mod.sys.executable, inert, "foreign_program", str(tmp_path / "main.py")]
    )
    assert not mod._verified_windows_chain(
        tmp_path, [{"path": mod.sys.executable, "command": command}]
    )


@pytest.mark.skipif(mod.os.name != "nt", reason="Windows command-line ownership contract")
def test_actual_checkout_script_is_verified(tmp_path):
    command = mod.subprocess.list2cmdline([mod.sys.executable, str(tmp_path / "main.py")])
    assert mod._verified_windows_chain(
        tmp_path, [{"path": mod.sys.executable, "command": command}]
    )


@pytest.mark.skipif(mod.os.name != "nt", reason="Windows hidden launcher contract")
def test_launch_uses_hidden_existing_launcher_without_sync(tmp_path, monkeypatch):
    (tmp_path / "start-mcp.ps1").write_text("# fixture", encoding="utf-8")
    monkeypatch.setattr(mod.shutil, "which", lambda name: "pwsh.exe")
    calls = []
    monkeypatch.setattr(
        mod, "launch_owned", lambda args, **kw: calls.append((args, kw)) or object()
    )
    mod.launch(tmp_path, 18765)
    args, options = calls[0]
    assert "-NonInteractive" in args
    assert str(tmp_path / "start-mcp.ps1") in args
    assert options["env"]["UV_NO_SYNC"] == "1"
    assert options["env"]["UV_OFFLINE"] == "1"
    assert options["env"]["MCP_HOST"] == "127.0.0.1"
    assert options["env"]["MCP_PORT"] == "18765"
    assert options["stdin"] == mod.subprocess.DEVNULL
    assert options["stdout"] == mod.subprocess.DEVNULL
    assert options["cwd"] == tmp_path


@pytest.mark.skipif(mod.os.name != "nt", reason="Windows command-line ownership contract")
@pytest.mark.parametrize("proof", ["both", "environment_only", "launcher_only", "neither"])
def test_supervisor_ancestors_preserve_only_proven_checkout_ownership(
    tmp_path, monkeypatch, proof
):
    root = tmp_path / "checkout with spaces"
    state = tmp_path / "state"
    monkeypatch.setenv("XDG_STATE_HOME", str(state))
    tee = state / "telegram-mcp/run/tee-123.py"
    target = root / "main.py"
    environment = root / ".venv/Scripts/python.exe"

    def process(executable, *arguments):
        return {
            "path": str(executable),
            "command": mod.subprocess.list2cmdline([str(executable), *map(str, arguments)]),
        }

    chain = [process(mod.sys.executable, tee, "synthetic.log", "1024", target)]
    if proof in ("both", "environment_only"):
        chain.append(process(environment, tee, "synthetic.log", "1024", target))
    if proof in ("both", "launcher_only"):
        chain.append(process("pwsh.exe", "-File", root / "start-mcp.ps1"))
    chain.extend(
        [
            process("pwsh.exe", "-File", state / "telegram-mcp/run/keepalive.ps1"),
            process(environment, root / "unrelated.py", "x", "y", target),
        ]
    )
    assert mod._verified_windows_chain(root, chain) is (proof == "both")
