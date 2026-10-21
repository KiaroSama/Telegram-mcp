"""Serialize Windows manual takeover and automatic generation creation."""

from contextlib import contextmanager
import hashlib
import os


def reservation_name(root):
    canonical = str(root.resolve()).replace("/", "\\").rstrip("\\").lower()
    digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()[:24]
    return "Local\\telegram-mcp-launch-" + digest


@contextmanager
def launch_reservation(root):
    if os.name != "nt":
        yield True
        return
    import ctypes
    from ctypes import wintypes

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    kernel.CreateMutexW.restype = wintypes.HANDLE
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    kernel.ReleaseMutex.argtypes = [wintypes.HANDLE]
    kernel.ReleaseMutex.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    ctypes.set_last_error(0)
    handle = kernel.CreateMutexW(None, True, reservation_name(root))
    existed = ctypes.get_last_error() == 183
    if not handle:
        raise OSError("Launch reservation could not be opened.")
    acquired = False
    try:
        if existed:
            yield False
            return
        acquired = True  # Initial ownership was acquired atomically with creation.
        yield True
    finally:
        if acquired:
            kernel.ReleaseMutex(handle)
        kernel.CloseHandle(handle)
