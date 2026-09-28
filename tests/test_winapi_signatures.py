"""The ctypes signature table under every capture, bound for real on Windows (plan 009).

An unbound Win32 function returns a sign-extended 32-bit int where a 64-bit handle
belongs (winapi.py's own docstring records the measured value). Every capture test
replaces `_win32`, so nothing else ever binds the real table. Collected everywhere and
skipped off Windows, so every CI leg counts the same cases.
"""

import ctypes
import sys

import pytest

from telegram_mcp.visual import winapi

windows_only = pytest.mark.skipif(sys.platform != "win32", reason="Win32 libraries")

# Every function whose answer is a handle: these must come back pointer-sized.
HANDLE_RETURNING = (
    "GetForegroundWindow",
    "GetWindowDC",
    "CreateCompatibleDC",
    "CreateCompatibleBitmap",
    "SelectObject",
    "OpenProcess",
)
# And a sample from each library whose argument types matter as much.
BOUND = HANDLE_RETURNING + (
    "EnumWindows",
    "GetWindowRect",
    "PrintWindow",
    "ReleaseDC",
    "GetDIBits",
    "DeleteObject",
    "QueryFullProcessImageNameW",
    "CloseHandle",
)


def _function(name):
    user32, gdi32, kernel32 = winapi._win32()
    for library in (user32, gdi32, kernel32):
        if hasattr(library, name):
            return getattr(library, name)
    raise AssertionError(f"{name} is not in any bound library")


@windows_only
@pytest.mark.parametrize("name", BOUND)
def test_every_listed_function_is_bound_with_argument_types(name):
    assert _function(name).argtypes is not None, f"{name} is unbound: ctypes would guess c_int"


@windows_only
@pytest.mark.parametrize("name", HANDLE_RETURNING)
def test_handle_returning_functions_return_pointer_sized_values(name):
    restype = _function(name).restype
    assert ctypes.sizeof(restype) == ctypes.sizeof(ctypes.c_void_p), f"{name} -> {restype}"


@windows_only
def test_the_libraries_are_singletons():
    assert winapi._win32() is winapi._win32()
