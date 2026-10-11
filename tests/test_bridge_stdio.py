"""Real inherited pipe behavior; child processes never access Telegram."""

import os
from pathlib import Path
import subprocess
import sys

import anyio
import pytest

_CODE = """
import sys
from contextlib import asynccontextmanager
import anyio
from telegram_mcp import bridge
from mcp.shared.message import SessionMessage
from mcp_types import jsonrpc_message_adapter
@asynccontextmanager
async def remote(*args, **kwargs):
    send, read = anyio.create_memory_object_stream(1)
    write, receive = anyio.create_memory_object_stream(1)
    await send.send(SessionMessage(jsonrpc_message_adapter.validate_python({
        "jsonrpc":"2.0", "id":1, "result":{"content":[{"type":"text","text":"x"*1048576}]}})))
    yield read, write
async def main():
    bridge.streamable_http_client = remote
    with anyio.move_on_after(0.2):
        await bridge.relay("http://127.0.0.1:12345/mcp")
anyio.run(main)
print("blocked-output-reaped", file=sys.stderr)
"""


def test_blocked_stdout_pipe_cancellation_reaps_process(tmp_path):
    root = Path(__file__).resolve().parents[1]
    env = {key: value for key, value in os.environ.items() if not key.startswith("TELEGRAM_")}
    env["PYTHONPATH"] = str(root)
    process = subprocess.Popen(
        [sys.executable, "-c", _CODE],
        cwd=tmp_path,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    try:
        # Keep both pipes open and NEVER read output until the child has exited.
        process.wait(timeout=4)
        assert process.returncode == 0, process.stderr.read().decode("utf-8")
        assert process.stderr.read().decode("utf-8").splitlines() == ["blocked-output-reaped"]
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        process.stdin.close()
        process.stdout.close()
        process.stderr.close()


def test_sdk_pipe_frame_roundtrip_preserves_utf8_and_restores_output_mode(tmp_path):
    root = Path(__file__).resolve().parents[1]
    code = """
import anyio, os, sys
from telegram_mcp.bridge_stdio import stdio_server
def mode():
    if os.name != "nt":
        return os.get_blocking(sys.stdout.fileno())
    import ctypes, msvcrt
    from ctypes import wintypes
    state = wintypes.DWORD()
    fn = ctypes.WinDLL("kernel32", use_last_error=True).GetNamedPipeHandleStateW
    fn.argtypes = [wintypes.HANDLE, wintypes.LPDWORD, wintypes.LPDWORD,
                   wintypes.LPDWORD, wintypes.LPDWORD, wintypes.LPWSTR, wintypes.DWORD]
    fn.restype = wintypes.BOOL
    assert fn(msvcrt.get_osfhandle(sys.stdout.fileno()), ctypes.byref(state), None, None, None, None, 0)
    return state.value
before = mode()
async def main():
    async with stdio_server() as (read, write):
        message = await read.receive()
        await write.send(message)
        await write.aclose()
        async for _ in read:
            pass
anyio.run(main)
assert mode() == before
"""
    env = {key: value for key, value in os.environ.items() if not key.startswith("TELEGRAM_")}
    env["PYTHONPATH"] = str(root)
    payload = '{"jsonrpc":"2.0","id":1,"result":{"text":"سلام"}}\n'.encode("utf-8")
    result = subprocess.run(
        [sys.executable, "-c", code],
        cwd=tmp_path,
        env=env,
        input=payload,
        capture_output=True,
        timeout=4,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    assert result.returncode == 0, result.stderr.decode("utf-8")
    assert result.stdout == payload
    assert result.stderr == b""


@pytest.mark.skipif(os.name != "nt", reason="Windows CRT ENOSPC compatibility")
def test_nonblocking_pipe_write_does_not_depend_on_crt_errno_mapping(monkeypatch):
    import errno
    from telegram_mcp.bridge_stdio import _PipeOutput

    read_fd, write_fd = os.pipe()
    writer = _PipeOutput(write_fd)
    try:

        def old_crt_write(*_args):
            raise OSError(errno.ENOSPC, "Synthetic CPython 3.11 full pipe")

        monkeypatch.setattr(os, "write", old_crt_write)

        async def write():
            await writer.write("known-output")

        anyio.run(write)
        assert os.read(read_fd, 100) == b"known-output"
    finally:
        writer.restore()
        os.close(read_fd)
        os.close(write_fd)
