"""Start or attach to this checkout's shared service, never another login."""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import time
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from dotenv import dotenv_values
from telegram_mcp.bridge_process import ProcessOwnershipError, launch_owned
from telegram_mcp.singleton import SessionLock, SessionLockError

START_SECONDS = 210.0
POLL_SECONDS = 0.25
_verified_services: dict[str, tuple[Path, int, tuple]] = {}
_unconfirmed_starts: list[tuple] = []


def verify_service(url: str) -> None:
    expected = _verified_services.get(url)
    if expected is None:
        raise BridgeStartupError("The shared service was not verified by this connector.")
    root, port, identity = expected
    if listener_identity(root, port) != identity:
        raise BridgeStartupError("Shared service ownership changed; initialization was refused.")


def _remember_service(url: str, root: Path, port: int, identity: tuple) -> str:
    _verified_services[url] = (root, port, identity)
    return url


class BridgeStartupError(RuntimeError):
    """A fixed startup refusal safe to show on stderr."""


def checkout() -> Path:
    root = Path(__file__).resolve().parents[1]
    if not (root / "main.py").is_file() or not (root / "pyproject.toml").is_file():
        raise BridgeStartupError("Bridge requires an installed source checkout.")
    return root


def endpoint_for(root: Path) -> str:
    values = dotenv_values(root / ".env", encoding="utf-8", interpolate=False)
    host = os.environ.get("MCP_HOST", values.get("MCP_HOST") or "127.0.0.1")
    if host not in ("127.0.0.1", "localhost"):
        raise BridgeStartupError("Bridge requires a loopback MCP_HOST (127.0.0.1).")
    raw = os.environ.get("MCP_PORT", values.get("MCP_PORT") or "8765")
    if not isinstance(raw, str) or not raw.isdecimal() or not 1 <= int(raw) <= 65535:
        raise BridgeStartupError("Bridge MCP port must be an integer from 1 to 65535.")
    return f"http://127.0.0.1:{int(raw)}/mcp"


def _port_open(port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.5):
            return True
    except OSError:
        return False


def _windows_argv(command: str) -> list[str]:
    import ctypes
    from ctypes import wintypes

    count = ctypes.c_int()
    parse = ctypes.WinDLL("shell32", use_last_error=True).CommandLineToArgvW
    parse.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_int)]
    parse.restype = ctypes.POINTER(wintypes.LPWSTR)
    pointer = parse(command, ctypes.byref(count))
    if not pointer:
        raise ValueError("invalid process command")
    free = ctypes.WinDLL("kernel32", use_last_error=True).LocalFree
    free.argtypes = [ctypes.c_void_p]
    free.restype = ctypes.c_void_p
    try:
        return [pointer[index] for index in range(count.value)]
    finally:
        free(pointer)


def _verified_windows_chain(root: Path, chain: list[dict]) -> bool:
    if not chain:
        return False
    listener = chain[0]
    argv = _windows_argv(listener.get("command") or "")
    if len(argv) < 2 or argv[1].startswith("-"):
        return False
    expected = Path(sys.executable).resolve()
    listener_path = Path(listener.get("path") or "").resolve()
    interpreter_paths = {expected, Path(sys.base_prefix, "python.exe").resolve()}
    if listener_path not in interpreter_paths:
        return False
    script = Path(argv[1]).resolve()
    if script == root / "main.py":
        return True
    # The launcher runs its private tee with main.py as the fifth positional word.
    state = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    run = (state / "telegram-mcp/run").resolve()
    if (
        script.parent != run
        or not re.fullmatch(r"tee-\d+\.py", script.name)
        or len(argv) < 5
        or Path(argv[4]).resolve() != root / "main.py"
    ):
        return False
    environment = root / ".venv/Scripts/python.exe"
    has_environment = False
    has_launcher = False
    for parent in chain[1:]:
        words = _windows_argv(parent.get("command") or "")
        executable = Path(parent.get("path") or "")
        if executable == environment and len(words) >= 5:
            has_environment = has_environment or (
                Path(words[1]).resolve() == script and Path(words[4]).resolve() == root / "main.py"
            )
        if executable.name.lower() in ("pwsh.exe", "powershell.exe"):
            lowered = [word.lower() for word in words]
            if "-file" in lowered:
                index = lowered.index("-file") + 1
                has_launcher = has_launcher or (
                    index < len(words) and Path(words[index]).resolve() == root / "start-mcp.ps1"
                )
    return has_environment and has_launcher


def _windows_identity(root: Path, port: int):
    shell = shutil.which("pwsh") or shutil.which("powershell")
    if shell is None:
        raise BridgeStartupError("PowerShell is required to verify listener ownership.")
    # EncodedCommand requires UTF-16LE; no text file uses this encoding.
    script = """
$ErrorActionPreference="Stop"
$root=$env:TELEGRAM_BRIDGE_CHECK_ROOT
$rows=@(Get-NetTCPConnection -State Listen -LocalPort PORT -ErrorAction SilentlyContinue)
$ids=@($rows | Select-Object -ExpandProperty OwningProcess -Unique)
if($ids.Count -ne 1){ throw "Listener identity ambiguous" }
$p=Get-CimInstance Win32_Process -Filter ("ProcessId="+$ids[0])
$chain=@(); $count=0
while($p -and $count -lt 8){
    $chain+=@{pid=$p.ProcessId; parent=$p.ParentProcessId; path=$p.ExecutablePath;
        created=$p.CreationDate.ToUniversalTime().ToString("o"); command=$p.CommandLine}
    $parent=Get-CimInstance Win32_Process -Filter ("ProcessId="+$p.ParentProcessId) -ErrorAction SilentlyContinue
    if($parent -and $parent.CreationDate -gt $p.CreationDate){break}
    $p=$parent; $count++
}
ConvertTo-Json -InputObject @($chain) -Compress
""".replace("PORT", str(port))
    env = dict(os.environ, TELEGRAM_BRIDGE_CHECK_ROOT=str(root))
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    try:
        result = subprocess.run(
            [shell, "-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=10,
            encoding="utf-8",
            check=True,
            env=env,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        chain = json.loads(result.stdout)
        if not _verified_windows_chain(root, chain):
            raise ValueError("ownership")
        return int(chain[0]["pid"]), chain[0]["created"]
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        raise BridgeStartupError("Unknown listener ownership; bridge refuses to attach.") from None


def _posix_identity(root: Path, port: int):
    if not Path("/proc").is_dir() or not shutil.which("ss"):
        raise BridgeStartupError("This platform cannot verify shared listener ownership.")
    try:
        result = subprocess.run(
            ["ss", "-ltnp", f"sport = :{port}"],
            capture_output=True,
            encoding="utf-8",
            timeout=5,
            check=True,
        )
        pids = set(re.findall(r"pid=(\d+)", result.stdout))
        if len(pids) != 1:
            raise ValueError("ambiguous")
        pid = int(pids.pop())
        proc = Path("/proc") / str(pid)
        words = [
            word.decode("utf-8")
            for word in proc.joinpath("cmdline").read_bytes().split(bytes([0]))
            if word
        ]
        if str(root / "main.py") not in words:
            raise ValueError("workspace")
        if proc.joinpath("exe").resolve() != Path(sys.executable).resolve():
            raise ValueError("interpreter")
        return pid, proc.joinpath("stat").read_text(encoding="utf-8").rsplit(")", 1)[1].split()[19]
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        raise BridgeStartupError("Unknown listener ownership; bridge refuses to attach.") from None


def listener_identity(root: Path, port: int):
    if not _port_open(port):
        return None
    return _windows_identity(root, port) if os.name == "nt" else _posix_identity(root, port)


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def ready(url: str) -> bool:
    try:
        opener = build_opener(ProxyHandler({}), _NoRedirect())
        try:
            with opener.open(Request(url, method="OPTIONS"), timeout=2) as response:
                return response.status in (200, 204, 400, 405)
        except HTTPError as error:
            return error.code in (400, 405)
    except OSError:
        return False


def launch(root: Path, port: int):
    env = dict(os.environ)
    env.update(
        MCP_TRANSPORT="http",
        MCP_HOST="127.0.0.1",
        MCP_PORT=str(port),
        UV_NO_SYNC="1",
        UV_OFFLINE="1",
        PYTHONUTF8="1",
    )
    python = root / ".venv" / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    env["UV_PROJECT_ENVIRONMENT"] = str(root / ".venv")
    env["UV_PYTHON"] = str(python)
    options = dict(
        cwd=root,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if os.name == "nt":
        shell = shutil.which("pwsh") or shutil.which("powershell")
        if shell is None or not (root / "start-mcp.ps1").is_file():
            raise BridgeStartupError("The existing PowerShell launcher is unavailable.")
        args = [
            shell,
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            str(root / "start-mcp.ps1"),
        ]
        options["creationflags"] = subprocess.CREATE_NO_WINDOW
    else:
        if not python.is_file():
            raise BridgeStartupError("Install the locked project environment before using bridge.")
        args = [str(python), str(root / "main.py")]
        options["start_new_session"] = True
    try:
        return launch_owned(args, **options)
    except ProcessOwnershipError as error:
        if error.owned is not None:
            raise
        raise BridgeStartupError("The shared service could not be started.") from None
    except OSError:
        raise BridgeStartupError("The shared service could not be started.") from None


def launch_supervisor(root: Path, port: int):
    python = root / ".venv" / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    if not python.is_file():
        raise BridgeStartupError("Install the locked environment before starting supervision.")
    env = dict(os.environ, MCP_HOST="127.0.0.1", MCP_PORT=str(port), PYTHONUTF8="1")
    try:
        return launch_owned(
            [str(python), "-m", "telegram_mcp.cli", "supervise"], cwd=root, env=env
        )
    except ProcessOwnershipError as error:
        if error.owned is not None:
            raise
        raise BridgeStartupError("The shared service supervisor could not be started.") from None
    except OSError:
        raise BridgeStartupError("The shared service supervisor could not be started.") from None


def _settle_failed_start(child) -> None:
    """Reap only this connector's owned tree, even after its launcher exited."""
    try:
        child.cleanup_supervisor(timeout=30)
    except (OSError, ProcessOwnershipError, subprocess.SubprocessError):
        raise BridgeStartupError(
            "Failed startup cleanup is unconfirmed; inspect the private service log."
        ) from None


def ensure_service(*, root: Path | None = None, url: str | None = None) -> str:
    root = checkout() if root is None else root.resolve()
    url = endpoint_for(root) if url is None else url
    parsed = urlsplit(url)
    if (
        parsed.scheme != "http"
        or parsed.hostname != "127.0.0.1"
        or parsed.path != "/mcp"
        or parsed.username
        or parsed.query
        or parsed.fragment
    ):
        raise BridgeStartupError("Bridge requires the verified loopback MCP endpoint.")
    try:
        port = parsed.port
    except ValueError:
        raise BridgeStartupError("Bridge requires a valid explicit MCP port.") from None
    if port is None:
        raise BridgeStartupError("Bridge requires an explicit MCP port.")
    identity = listener_identity(root, port)
    if identity is not None:
        if ready(url) and listener_identity(root, port) == identity:
            return _remember_service(url, root, port, identity)
        raise BridgeStartupError("The verified shared service is not ready; it was not replaced.")
    state = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    lease = SessionLock(f"bridge-start:{root}:{port}", lock_dir=state / "telegram-mcp/run")
    child = None
    published = False
    try:
        lease.acquire(grace_seconds=START_SECONDS, poll_interval=POLL_SECONDS)
        identity = listener_identity(root, port)
        child = None if identity else launch_supervisor(root, port)
        deadline = time.monotonic() + START_SECONDS
        while time.monotonic() < deadline:
            identity = listener_identity(root, port)
            if identity is not None and ready(url) and listener_identity(root, port) == identity:
                if child is not None:
                    if child.owns_descendant(identity[0]):
                        child.detach()
                    else:
                        _settle_failed_start(child)
                published = True
                return _remember_service(url, root, port, identity)
            if child is not None and child.poll() not in (None, 0, 75):
                raise BridgeStartupError("Shared service startup failed; inspect its private log.")
            time.sleep(POLL_SECONDS)
        raise BridgeStartupError(
            "Shared service readiness timed out; no replacement was attempted."
        )
    except ProcessOwnershipError as error:
        if error.owned is not None:
            child = error.owned
        raise BridgeStartupError(
            "Shared service process ownership could not be released safely."
        ) from None
    except SessionLockError:
        raise BridgeStartupError(
            "Another bridge is starting the shared service; retry attachment later."
        ) from None
    finally:
        try:
            if child is not None and not published:
                _settle_failed_start(child)
        except BaseException:
            _unconfirmed_starts.append((lease, child))
            raise
        else:
            lease.release()
