"""Cancellable, byte-bounded pipe I/O for the SDK stdio transport.

SDK AsyncFile waits for blocked workers even after cancellation. Desktop pipes
can stay open or stop draining on failure, so the bridge uses thread-free pipe
I/O. SDK parsing and output serialization stay intact.
"""

from contextlib import asynccontextmanager
import os
import sys

import anyio
from mcp.server.stdio import stdio_server as sdk_stdio_server

MAX_FRAME_BYTES = 32 * 1024 * 1024


class StdioError(RuntimeError):
    """Controlled pipe error that contains no input bytes."""


class _PipeInput:
    def __init__(self, fd):
        self.fd = fd
        self.stopped = False
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes
            import msvcrt

            self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            self.peek = self.kernel.PeekNamedPipe
            self.peek.argtypes = [
                wintypes.HANDLE,
                wintypes.LPVOID,
                wintypes.DWORD,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
            ]
            self.peek.restype = wintypes.BOOL
            get_type = self.kernel.GetFileType
            get_type.argtypes = [wintypes.HANDLE]
            get_type.restype = wintypes.DWORD
            self.handle = msvcrt.get_osfhandle(fd)
            if get_type(self.handle) != 3:  # FILE_TYPE_PIPE
                raise StdioError("Bridge stdin must be a pipe.")
        else:
            os.set_blocking(fd, False)

    async def chunk(self):
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes

            while not self.stopped:
                available = wintypes.DWORD()
                # This adapter is the ONLY reader on this handle. No concurrent
                # synchronous I/O can hold its pipe lock while PeekNamedPipe runs.
                if not self.peek(self.handle, None, 0, None, ctypes.byref(available), None):
                    if ctypes.get_last_error() in (109, 232):  # broken/no-data pipe
                        return b""
                    raise StdioError("Bridge stdin pipe could not be inspected.")
                if available.value:
                    return os.read(self.fd, min(available.value, 65536))
                await anyio.sleep(0.01)  # Cancellable readiness poll for anonymous Win32 pipes.
            return b""
        while not self.stopped:
            try:
                return os.read(self.fd, 65536)
            except BlockingIOError:
                with anyio.move_on_after(0.01):
                    await anyio.wait_readable(self.fd)
        return b""

    async def __aiter__(self):
        pending = bytearray()
        try:
            while True:
                chunk = await self.chunk()
                if not chunk:
                    if pending:
                        yield pending.decode("utf-8")
                    return
                pending.extend(chunk)
                while (newline := pending.find(b"\n")) != -1:
                    if newline > MAX_FRAME_BYTES:
                        raise StdioError("Bridge stdin frame exceeds the 32 MiB limit.")
                    line = bytes(pending[: newline + 1])
                    del pending[: newline + 1]
                    yield line.decode("utf-8")
                if len(pending) > MAX_FRAME_BYTES:
                    raise StdioError("Bridge stdin frame exceeds the 32 MiB limit.")
                await anyio.lowlevel.checkpoint()
        except (UnicodeError, OSError):
            raise StdioError("Bridge stdin contained an invalid frame or failed.") from None


class _PipeOutput:
    def __init__(self, fd):
        self.fd, self.stopped = fd, False
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes
            import msvcrt

            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            get_state = kernel.GetNamedPipeHandleStateW
            get_state.argtypes = [
                wintypes.HANDLE,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
                wintypes.LPWSTR,
                wintypes.DWORD,
            ]
            get_state.restype = wintypes.BOOL
            self.set_state = kernel.SetNamedPipeHandleState
            self.set_state.argtypes = [
                wintypes.HANDLE,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
                wintypes.LPDWORD,
            ]
            self.set_state.restype = wintypes.BOOL
            self.handle = msvcrt.get_osfhandle(fd)
            previous = wintypes.DWORD()
            if not get_state(self.handle, ctypes.byref(previous), None, None, None, None, 0):
                raise StdioError("Bridge stdout pipe mode could not be inspected.")
            self.previous = previous.value
            self._set_windows_mode(self.previous | 1)  # PIPE_NOWAIT, preserving read mode.
        else:
            self.previous = os.get_blocking(fd)
            os.set_blocking(fd, False)

    def _set_windows_mode(self, mode):
        import ctypes
        from ctypes import wintypes

        value = wintypes.DWORD(mode)
        if not self.set_state(self.handle, ctypes.byref(value), None, None):
            raise StdioError("Bridge stdout pipe mode could not be changed.")

    async def write(self, text):
        data = text.encode("utf-8")
        offset = 0
        try:
            while offset < len(data):
                if self.stopped:
                    raise anyio.ClosedResourceError
                try:
                    written = os.write(self.fd, data[offset : offset + 65536])
                except BlockingIOError:
                    written = 0
                if written:
                    offset += written
                elif os.name == "nt":
                    await anyio.sleep(0.01)
                else:
                    with anyio.move_on_after(0.01):
                        await anyio.wait_writable(self.fd)
                await anyio.lowlevel.checkpoint()
        except OSError:
            raise StdioError("Bridge stdout pipe write failed.") from None
        return len(text)

    async def flush(self):
        await anyio.lowlevel.checkpoint()

    def restore(self):
        if os.name == "nt":
            self._set_windows_mode(self.previous)
        else:
            os.set_blocking(self.fd, self.previous)


@asynccontextmanager
async def stdio_server():
    fd = os.dup(sys.stdin.fileno())
    output_fd = None
    writer = None
    previous_blocking = None
    try:
        output_fd = os.dup(sys.stdout.fileno())
        if os.name != "nt":
            previous_blocking = os.get_blocking(fd)
        reader, writer = _PipeInput(fd), _PipeOutput(output_fd)
        async with sdk_stdio_server(stdin=reader, stdout=writer) as streams:
            try:
                yield streams
            finally:
                # Explicitly stop both SDK tasks even under the bridge's shield.
                reader.stopped = writer.stopped = True
                await streams[1].aclose()
    finally:
        if writer is not None:
            writer.restore()
        if output_fd is not None:
            os.close(output_fd)
        if previous_blocking is not None:
            os.set_blocking(fd, previous_blocking)
        os.close(fd)
