"""One feed consumer at a time, and who owns one that will not stop.

Split from ``events`` because starting and stopping the consumer is a different
question from what the consumer does, and the two were interleaved in a way that
let three of them run at once.

The consumer holds the feed file open and CONSUMES settled bursts: a burst it
takes is one `wait_for_settled_message` will never see. So "how many consumers
are there" is not bookkeeping, it is whether events get answered twice, once, or
not at all. Three ways there came to be more than one:

* ``enable`` awaited the stop and then ignored its answer. A stop that timed out
  returned False and the replacement started anyway, so the old consumer and the
  new one raced for the same bursts.
* ``disable`` cleared the registered task BEFORE the stop finished. If the stop
  then timed out, the still-running task belonged to nobody: nothing could wait
  for it, and the next ``enable`` saw a clean slate and started a third.
* Nothing serialized the transitions, so two concurrent ``enable`` calls both
  passed the "is it running" check and both started a task.

What replaces it:

* **One transition at a time.** Every start and stop runs under one lock.
* **Ownership is recorded before the await, never after.** A task whose stop
  times out - or whose stop is interrupted by the CALLER being cancelled - stays
  held as the stopping task rather than being dropped.
* **No replacement until the previous consumer has confirmed it stopped.**
  ``stopping`` is a refusal, not a state to start on top of.
* **The state is reported as it is**: ``stopped``, ``running``, ``stopping`` or
  ``failed``.
"""

import asyncio
import base64
import logging
import os
import shlex
from typing import Any, Awaitable, Callable, Dict, Optional

from telegram_mcp.safe_log import log_event
from telegram_mcp.tools import events_store as store

# The consumer this process considers current, and one it has asked to stop but
# has not yet seen stop. The second is the point: a task nobody holds is a task
# nobody can wait for.
_task: Optional[asyncio.Task] = None
_stopping: Optional[asyncio.Task] = None
_settle_ms: int = 6000
_autostart_done: bool = False

# Constructed at import: since 3.10 an asyncio.Lock binds to no loop until it is
# awaited, and this package requires 3.11.
_transition = asyncio.Lock()

# How long a cancelled consumer gets to actually stop before the caller is told
# it did not. Cancellation is a request, and returning before it lands leaves the
# task holding the feed file open under a caller who believes it is closed.
_FEED_STOP_TIMEOUT_SECONDS = 5.0


def _reap() -> None:
    """Let go of a stopping task that has finally stopped."""
    global _stopping
    if _stopping is not None and _stopping.done():
        _stopping = None


def feed_enabled() -> bool:
    """Whether a consumer is currently registered and running."""
    return _task is not None and not _task.done()


def feed_state() -> str:
    """``stopped`` | ``running`` | ``stopping`` | ``failed``.

    ``stopping`` outranks the rest: while a cancelled consumer is still alive it
    can still take a burst, and a caller told "stopped" would believe otherwise.
    """
    _reap()
    if _stopping is not None:
        return "stopping"
    if _task is None:
        return "stopped"
    if not _task.done():
        return "running"
    if _task.cancelled():
        return "stopped"
    return "failed" if _task.exception() is not None else "stopped"


def failure() -> Optional[str]:
    """Why the consumer ended, when it ended by raising."""
    if _task is None or not _task.done() or _task.cancelled():
        return None
    error = _task.exception()
    return None if error is None else f"{type(error).__name__}: {error}"


def settle_ms() -> int:
    return _settle_ms


def autostart_done() -> bool:
    return _autostart_done


def _spawn(make_coro: Callable[[int], Awaitable[None]], settle: int) -> None:
    global _task, _settle_ms, _autostart_done
    _settle_ms = settle
    # Any explicit or implicit start consumes the env autostart, so a later
    # disable cannot be resurrected by the next incoming message.
    _autostart_done = True
    _task = asyncio.get_running_loop().create_task(make_coro(settle))


async def _stop_current() -> bool:
    """Cancel the registered consumer and WAIT for it, bounded.

    ``task.cancel()`` schedules a cancellation; it does not perform one.
    Returning at that point told the caller the feed was off while the task was
    still running, still holding the feed file open and still consuming settled
    bursts that ``wait_for_settled_message`` was about to be told it could have.

    ``asyncio.wait`` rather than ``await task``: awaiting a cancelled task
    re-raises the CancelledError here, and swallowing that is how a cancellation
    aimed at the caller gets eaten by mistake. The task is moved to ``_stopping``
    BEFORE the await, so a caller cancelled mid-wait leaves it owned rather than
    orphaned.
    """
    global _task, _stopping
    task, _task = _task, None
    if task is None:
        return True
    _stopping = task
    task.cancel()
    done, _still_running = await asyncio.wait({task}, timeout=_FEED_STOP_TIMEOUT_SECONDS)
    if not done:
        log_event(logging.WARNING, "event-feed-stop-timeout", seconds=_FEED_STOP_TIMEOUT_SECONDS)
        return False
    _stopping = None
    return True


def start_now(make_coro: Callable[[int], Awaitable[None]], settle: Optional[int] = None) -> bool:
    """Start the consumer from synchronous code, refusing to double up.

    For the autostart path, which runs inside the Telethon handler and has no
    place to await a lock. It refuses rather than queues: a second consumer is
    the failure this module exists to prevent, and the autostart is a
    convenience that can simply not happen.
    """
    _reap()
    if feed_enabled() or _stopping is not None:
        return False
    _spawn(make_coro, _settle_ms if settle is None else settle)
    return True


async def enable(settle: int, make_coro: Callable[[int], Awaitable[None]]) -> Dict[str, Any]:
    """Make exactly one consumer run at ``settle`` milliseconds.

    ``{"ok": True, ...}`` when a consumer is running at that settle period -
    including when it already was. ``{"ok": False, "reason": ...}`` otherwise,
    and in that case nothing was started.
    """
    async with _transition:
        _reap()
        if _stopping is not None:
            return {"ok": False, "reason": "stopping"}
        if feed_enabled():
            if settle == _settle_ms:
                return {"ok": True, "reason": "already-running"}
            if not await _stop_current():
                # The old consumer is still alive and still taking bursts.
                # Starting the replacement here is what produced two.
                return {"ok": False, "reason": "stop-timeout"}
        _spawn(make_coro, settle)
        return {"ok": True, "reason": "started"}


async def disable() -> Dict[str, Any]:
    """Stop the consumer and confirm it stopped."""
    async with _transition:
        _reap()
        if _stopping is not None:
            return {"ok": False, "reason": "stopping"}
        if not feed_enabled():
            return {"ok": False, "reason": "not-enabled"}
        if not await _stop_current():
            return {"ok": False, "reason": "stop-timeout"}
        return {"ok": True, "reason": "stopped"}


# How often the Windows watcher looks for new bytes. `Get-Content -Wait` is the
# obvious answer and the wrong one: it follows the DESCRIPTOR, so after the
# `os.replace` in _rotate_feed_if_needed it goes on reading the rotated
# generation for ever and never sees another event. Following the NAME means
# re-opening it, which means polling; 500ms is far below a human-visible delay
# and costs one stat per interval.
_WATCH_POLL_MS = 500


def watch_script(path, contains: Optional[str] = None) -> str:
    """A rotation-aware PowerShell tail for ``path``, optionally filtered.

    Rotation is detected by CONTINUITY, not by length. Reading the length alone
    only caught a replacement shorter than the offset already read: a fresh
    generation that grew back past that mark inside one poll interval was seeked
    into, and everything it had written first was never emitted.

    So the first 64 bytes are read on every poll and compared with what they were
    last time; a change means a different file and the offset goes back to zero,
    whatever the new length is. The creation stamp cannot do this job on Windows:
    NTFS tunneling gives a name recreated within about fifteen seconds the OLD
    stamp, and fifteen seconds is far longer than a rotation takes. The bytes
    themselves have no such memory.

    Opened with FileShare.ReadWrite so watching never blocks the feed from
    writing or from replacing the file underneath.
    """
    quoted = str(path).replace("'", "''")
    emit = "$line" if contains is None else f"if($line -like '*{contains}*'){{$line}}"
    return (
        f"$p='{quoted}';$o=[long]0;$head='';"
        "while($true){"
        "if(Test-Path -LiteralPath $p){"
        "$len=(Get-Item -LiteralPath $p).Length;"
        "if($len -lt $o){$o=[long]0};"
        "if($len -gt $o){"
        "$f=[IO.File]::Open($p,[IO.FileMode]::Open,[IO.FileAccess]::Read,"
        "[IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete);"
        "try{"
        # Continuity, checked against the file's own first bytes. Creation time
        # cannot do this on Windows: NTFS tunneling hands a name recreated within
        # about fifteen seconds the OLD creation stamp, which is exactly the
        # interval a rotation happens in.
        "$b=New-Object byte[] 64;$n=$f.Read($b,0,64);"
        "$h=[Convert]::ToBase64String($b,0,$n);"
        "if($h -ne $head){$head=$h;$o=[long]0};"
        "[void]$f.Seek($o,[IO.SeekOrigin]::Begin);"
        "$r=New-Object IO.StreamReader($f,[Text.Encoding]::UTF8);"
        "while($null -ne ($line=$r.ReadLine())){" + emit + "};"
        "$o=$f.Position}finally{$f.Dispose()}}}"
        f"Start-Sleep -Milliseconds {_WATCH_POLL_MS}" + "}"
    )


def watch_command(path, contains: Optional[str] = None) -> str:
    """The watcher to arm, in the shell this host actually has.

    `tail -F` and `grep --line-buffered` are not commands on a Windows host, so
    reporting them there described a monitor nobody could start.

    On Windows the script goes in as `-EncodedCommand`, base64 of UTF-16LE.
    Wrapping it in outer double quotes did not survive being pasted into
    PowerShell: `$p`, `$o` and the rest were expanded by THE SHELL THE USER RAN
    IT FROM before the child ever parsed them, so the watcher started with empty
    variables - or with whatever those names happened to hold in that session.
    Encoded, no shell has anything left to substitute. The readable form is
    published beside it as `watch_script`, because a command nobody can read is
    a command nobody should be asked to trust.
    """
    if os.name == "nt":
        encoded = base64.b64encode(watch_script(path, contains).encode("utf-16-le")).decode(
            "ascii"
        )
        return f"powershell -NoProfile -NonInteractive -EncodedCommand {encoded}"
    # -F survives rotation/truncation and waits for a not-yet-created file.
    follow = f"tail -n 0 -F {shlex.quote(str(path))}"
    if contains is None:
        return follow
    return f"{follow} | grep --line-buffered {shlex.quote(contains)}"


def incoming_feed_state(autostart_pending: bool = False) -> Dict[str, Any]:
    """Everything a caller needs to know about the feed, including its state.

    The retention pass runs here as well as on append: rotation at append time
    is the right moment while events arrive and no moment at all once they stop,
    and a status call is the other thing that happens to an idle server.
    """
    try:
        store.apply_retention()
    except OSError as error:
        # Reporting the state must not fail because the feed file is unwritable.
        log_event(logging.WARNING, "event-feed-retention-pass-failed", error=error)
    path = store.feed_file_path()
    max_bytes, max_age = store.feed_retention()
    max_pending, pending_ttl = store.pending_bounds()
    return {
        "enabled": feed_enabled(),
        "state": feed_state(),
        "consumer_error": failure(),
        "rotation_error": store.rotation_error(),
        "feed_file": str(path),
        "settle_ms": _settle_ms,
        "rotated_file": str(store._rotated_feed_path(path)),
        "max_bytes": max_bytes,
        "max_age_seconds": max_age,
        "retention_note": (
            "The feed rotates at max_bytes OR once its oldest record passes "
            "max_age_seconds, whichever comes first, and keeps one previous "
            "generation until that too is older than max_age_seconds. Age is read "
            "from the first record in the file, not from its mtime, which every "
            "append moves forward. Disk use is bounded by roughly twice max_bytes. "
            "watch_command follows the NAME rather than the open file, so it keeps "
            "reading across a rotation."
        ),
        "max_pending_chats": max_pending,
        "pending_ttl_seconds": pending_ttl,
        "pending_chats": len(store._pending_msgs),
        **store.overflow_state(),
        "watch_command": watch_command(path),
        "watch_script": watch_script(path),
        "watch_command_for_one_chat": watch_command(path, '"chat_id": <ID>'),
        "watch_script_for_one_chat": watch_script(path, '"chat_id": <ID>'),
        "watch_shell": "powershell" if os.name == "nt" else "sh",
        "autostart_pending": autostart_pending,
    }


__all__ = [
    "autostart_done",
    "disable",
    "enable",
    "failure",
    "feed_enabled",
    "feed_state",
    "incoming_feed_state",
    "settle_ms",
    "start_now",
    "watch_command",
    "watch_script",
]
