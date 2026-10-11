"""Own a cold-start process tree until publication or bounded failure cleanup."""

import math
import os
from pathlib import Path
import signal
import subprocess
import time

# Existing packaged, lazy-imported native helper: no runtime or Telegram imports.
from telegram_mcp.visual.bounded_process import _contain, _resume, _win32


class ProcessOwnershipError(RuntimeError):
    """Controlled ownership failure; command/environment values are not reported."""


def _active_windows(job):
    ctypes, kernel, _limits, _thread = _win32()
    from ctypes import wintypes

    class Accounting(ctypes.Structure):
        _fields_ = [
            ("TotalUserTime", ctypes.c_int64),
            ("TotalKernelTime", ctypes.c_int64),
            ("ThisPeriodTotalUserTime", ctypes.c_int64),
            ("ThisPeriodTotalKernelTime", ctypes.c_int64),
            ("TotalPageFaultCount", wintypes.DWORD),
            ("TotalProcesses", wintypes.DWORD),
            ("ActiveProcesses", wintypes.DWORD),
            ("TotalTerminatedProcesses", wintypes.DWORD),
        ]

    query = kernel.QueryInformationJobObject
    query.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.LPDWORD,
    ]
    query.restype = wintypes.BOOL
    value = Accounting()
    if not query(job, 1, ctypes.byref(value), ctypes.sizeof(value), None):
        raise ProcessOwnershipError("Owned startup job could not be inspected.")
    return value.ActiveProcesses


def _windows_members(job):
    ctypes, kernel, _limits, _thread = _win32()
    from ctypes import wintypes

    # Retain observed members as extra exit evidence. TerminateJobObject plus
    # zero active members is the native tree boundary: this snapshot cannot cover
    # a member spawned between enumeration and termination.
    count = 64
    for _ in range(8):

        class Members(ctypes.Structure):
            _fields_ = [
                ("assigned", wintypes.DWORD),
                ("listed", wintypes.DWORD),
                ("pids", ctypes.c_size_t * count),
            ]

        members = Members()
        if kernel.QueryInformationJobObject(
            job, 3, ctypes.byref(members), ctypes.sizeof(members), None
        ):
            break
        if ctypes.get_last_error() != 234:
            raise ProcessOwnershipError("Owned startup members could not be inspected.")
        count *= 2
    else:
        raise ProcessOwnershipError("Owned startup tree exceeded its process limit.")
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    check = kernel.IsProcessInJob
    check.argtypes = [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)]
    check.restype = wintypes.BOOL
    handles = []
    try:
        for pid in members.pids[: members.listed]:
            handle = kernel.OpenProcess(0x101000, False, pid)
            if handle:
                contained = wintypes.BOOL()
                if not check(handle, job, ctypes.byref(contained)):
                    kernel.CloseHandle(handle)
                    raise ProcessOwnershipError(
                        "Owned startup member identity could not be verified."
                    )
                if contained.value:
                    handles.append(handle)
                else:
                    kernel.CloseHandle(handle)  # PID was reused outside our job.
            elif ctypes.get_last_error() != 87:  # Already exited before OpenProcess.
                raise ProcessOwnershipError("Owned startup member could not be held for cleanup.")
        return handles
    except Exception:
        for handle in handles:
            kernel.CloseHandle(handle)
        raise


def _active_posix(pgid):
    proc = Path("/proc")
    if not proc.is_dir():
        raise ProcessOwnershipError("Owned startup cleanup requires Linux process evidence.")
    try:
        for entry in proc.iterdir():
            if not entry.name.isdecimal():
                continue
            try:
                stat = (entry / "stat").read_text(encoding="utf-8")
            except FileNotFoundError:
                continue
            fields = stat[stat.rfind(")") + 2 :].split()
            if len(fields) < 3:
                raise ValueError("incomplete process evidence")
            if int(fields[2]) == pgid and fields[0] not in ("Z", "X"):
                return True
        return False
    except (OSError, ValueError):
        raise ProcessOwnershipError(
            "Owned startup process group could not be inspected."
        ) from None


class OwnedProcess:
    def __init__(self, process, job=None):
        self.process, self.job = process, job
        self.pid = process.pid
        self.released = False

    def poll(self):
        if self.released:
            return self.process.returncode
        return self.process.poll()

    def owns(self, pid: int) -> bool:
        """Whether the verified listener belongs to this launch's ownership tree."""
        if self.released or not isinstance(pid, int) or pid <= 0:
            raise ProcessOwnershipError("Owned startup membership cannot be verified.")
        if os.name != "nt":
            try:
                return os.getpgid(pid) == self.pid
            except ProcessLookupError:
                return False
            except OSError:
                raise ProcessOwnershipError(
                    "Owned startup membership cannot be verified."
                ) from None
        ctypes, kernel, _limits, _thread = _win32()
        from ctypes import wintypes

        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        check = kernel.IsProcessInJob
        check.argtypes = [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)]
        check.restype = wintypes.BOOL
        handle = kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            if ctypes.get_last_error() == 87:
                return False
            raise ProcessOwnershipError("Owned startup listener cannot be inspected.")
        try:
            contained = wintypes.BOOL()
            if not check(handle, self.job, ctypes.byref(contained)):
                raise ProcessOwnershipError("Owned startup membership cannot be verified.")
            return bool(contained.value)
        finally:
            kernel.CloseHandle(handle)

    def _release(self):
        if self.job is not None:
            _ctypes, kernel, _limits, _thread = _win32()
            if not kernel.CloseHandle(self.job):
                raise ProcessOwnershipError("Owned startup job handle could not be closed.")
            self.job = None
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            if stream is not None:
                stream.close()
        if os.name == "nt":
            self.process._handle.Close()
            # Popen no longer owns a usable wait handle. Avoid its destructor
            # polling that closed handle; this does not assert a child exit code.
            self.process._child_created = False
        self.released = True

    def detach(self):
        """Release ownership without killing the now-published shared service."""
        if self.released:
            return
        if self.job is not None:
            ctypes, kernel, limits_type, _thread = _win32()
            limits = limits_type()
            # Clear only our fresh job's kill-on-close flag; the process remains
            # in its job, but closing this handle no longer terminates it.
            if not kernel.SetInformationJobObject(
                self.job, 9, ctypes.byref(limits), ctypes.sizeof(limits)
            ):
                raise ProcessOwnershipError("Owned startup job could not be detached safely.")
        self._release()

    def cleanup(self, timeout=5.0):
        """Terminate only this launch tree, including a child whose parent exited."""
        if self.released:
            return
        if not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or timeout <= 0:
            raise ProcessOwnershipError(
                "Owned startup cleanup requires a finite positive timeout."
            )
        deadline = time.monotonic() + timeout
        held = []
        try:
            if self.job is not None:
                _ctypes, kernel, _limits, _thread = _win32()
                held = _windows_members(self.job)
                if not kernel.TerminateJobObject(self.job, 1):
                    raise ProcessOwnershipError("Owned startup job termination failed.")

                def active():
                    return _active_windows(self.job) or any(
                        kernel.WaitForSingleObject(handle, 0) != 0 for handle in held
                    )

            else:
                try:
                    os.killpg(self.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

                def active():
                    return _active_posix(self.pid)

            while True:
                self.process.poll()  # Reap the root even while descendants finish.
                if not active():
                    break
                if time.monotonic() >= deadline:
                    raise ProcessOwnershipError(
                        "Owned startup tree cleanup exceeded its time limit."
                    )
                time.sleep(min(0.01, max(0, deadline - time.monotonic())))
            self.process.wait(timeout=max(0.01, deadline - time.monotonic()))
        finally:
            if held:
                for handle in held:
                    kernel.CloseHandle(handle)
            self._release()


def launch_owned(args, **options) -> OwnedProcess:
    """Start suspended/job-contained on Windows, in an owned session on POSIX."""
    if options.get("shell") or not isinstance(args, (list, tuple)) or not args:
        raise ProcessOwnershipError("Owned startup requires an argument list, not a shell.")
    options.setdefault("stdin", subprocess.DEVNULL)
    options.setdefault("stdout", subprocess.DEVNULL)
    options.setdefault("stderr", subprocess.DEVNULL)
    if os.name == "nt":
        options["creationflags"] = (
            options.get("creationflags", 0) | 4 | subprocess.CREATE_NO_WINDOW
        )
    else:
        if not Path("/proc").is_dir():
            raise ProcessOwnershipError("Owned startup requires Linux process evidence.")
        options["start_new_session"] = True
    try:
        process = subprocess.Popen(args, **options)
    except Exception:
        raise ProcessOwnershipError("Owned startup process could not be launched.") from None
    owned = OwnedProcess(process)
    if os.name == "nt":
        try:
            owned.job = _contain(process)
            if owned.job is None:
                raise ProcessOwnershipError("Owned startup process could not be contained.")
            if _active_windows(owned.job) != 1:
                raise ProcessOwnershipError("Owned startup containment could not be verified.")
            if not _resume(process):
                raise ProcessOwnershipError("Owned startup process could not be resumed.")
        except Exception:
            # A child not yet resumed cannot have spawned descendants.
            if owned.job is None:
                process.kill()
                process.wait(timeout=5)
                owned._release()
            else:
                owned.cleanup()
            raise ProcessOwnershipError("Owned startup containment or resume failed.") from None
    return owned
