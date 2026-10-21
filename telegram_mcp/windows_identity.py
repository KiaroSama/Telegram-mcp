"""Read a live Windows process chain without PowerShell or WMI."""

import ctypes
from ctypes import wintypes
from contextlib import contextmanager


class IdentityError(OSError):
    """Native identity evidence was unavailable or changed."""


def _api():
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    query = ctypes.WinDLL("ntdll").NtQueryInformationProcess
    query.argtypes = [
        wintypes.HANDLE,
        wintypes.ULONG,
        ctypes.c_void_p,
        wintypes.ULONG,
        ctypes.POINTER(wintypes.ULONG),
    ]
    query.restype = wintypes.LONG
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.CloseHandle.restype = wintypes.BOOL
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel.GetProcessTimes.restype = wintypes.BOOL
    kernel.QueryFullProcessImageNameW.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.LPWSTR,
        ctypes.POINTER(wintypes.DWORD),
    ]
    kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
    return kernel, query


@contextmanager
def process_chain(pid):
    """Keep identity handles alive through validation; callers recheck the listener."""
    kernel, query = _api()
    held = []
    try:
        chain = []
        seen = set()
        while pid and pid not in seen and len(chain) < 8:
            seen.add(pid)
            handle = kernel.OpenProcess(0x101000, False, pid)
            if not handle:
                if chain:
                    break
                raise IdentityError("Listener process could not be opened.")
            held.append(handle)
            row = _process(kernel, query, handle, pid)
            if chain and row["created"] > chain[-1]["created"]:
                break
            chain.append(row)
            pid = row["parent"]
        yield chain
        if not held or kernel.WaitForSingleObject(held[0], 0) != 258:
            raise IdentityError("Listener changed during ownership inspection.")
    finally:
        for handle in reversed(held):
            kernel.CloseHandle(handle)


def _process(kernel, query, handle, pid):
    class Basic(ctypes.Structure):
        _fields_ = [
            ("reserved", ctypes.c_void_p),
            ("peb", ctypes.c_void_p),
            ("reserved2", ctypes.c_void_p * 2),
            ("pid", ctypes.c_size_t),
            ("parent", ctypes.c_size_t),
        ]

    basic = Basic()
    returned = wintypes.ULONG()
    if query(handle, 0, ctypes.byref(basic), ctypes.sizeof(basic), ctypes.byref(returned)) < 0:
        raise IdentityError("Process ancestry could not be read.")
    stamps = [wintypes.FILETIME() for _ in range(4)]
    if not kernel.GetProcessTimes(handle, *map(ctypes.byref, stamps)):
        raise IdentityError("Process birth identity could not be read.")
    size = wintypes.DWORD(32768)
    path = ctypes.create_unicode_buffer(size.value)
    if not kernel.QueryFullProcessImageNameW(handle, 0, path, ctypes.byref(size)):
        raise IdentityError("Process executable could not be read.")
    return {
        "pid": pid,
        "parent": int(basic.parent),
        "path": path.value,
        "created": (stamps[0].dwHighDateTime << 32) | stamps[0].dwLowDateTime,
        "command": _command(query, handle),
    }


def _command(query, handle):
    class Unicode(ctypes.Structure):
        _fields_ = [
            ("length", wintypes.USHORT),
            ("maximum", wintypes.USHORT),
            ("buffer", ctypes.c_void_p),
        ]

    size = wintypes.ULONG()
    query(handle, 60, None, 0, ctypes.byref(size))
    for _ in range(3):
        if not ctypes.sizeof(Unicode) <= size.value <= 1024 * 1024:
            break
        buffer = ctypes.create_string_buffer(size.value)
        returned = wintypes.ULONG()
        status = query(handle, 60, buffer, len(buffer), ctypes.byref(returned))
        if status < 0:
            if returned.value > size.value:
                size.value = returned.value
                continue
            break
        value = Unicode.from_buffer(buffer)
        start = ctypes.addressof(buffer)
        pointer = value.buffer or 0
        if (
            value.length % 2
            or value.length > value.maximum
            or pointer < start + ctypes.sizeof(Unicode)
            or pointer + value.length > start + len(buffer)
        ):
            break
        return ctypes.wstring_at(pointer, value.length // 2)
    raise IdentityError("Process command line could not be read.")
