"""Exports run as background jobs (spec 030 FR-014, owner 2026-10-01).

A Telegram Desktop export takes minutes - every message read, every file downloaded - while
one tool call is cut at 55 s (`tool_budget`) and most clients give up at a minute. So an
export tool starts a job here and answers at once; `export_status` reports its progress
(the files written so far, as Desktop's progress panel counts them) and its result, and
`cancel_export` stops it. Jobs live in this process only: a restart ends them.
"""

import asyncio
import itertools
import logging
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Awaitable, Callable, Dict, List, Optional

log = logging.getLogger("telegram_mcp.export_jobs")

_jobs: Dict[str, "Job"] = {}
_ids = itertools.count(1)


@dataclass
class Job:
    id: str
    kind: str
    chat: Any
    folder: str
    started: float
    task: Optional["asyncio.Task[None]"] = None
    result: Optional[dict] = None
    error: Optional[str] = None
    cancelled: bool = False


def start(kind: str, chat: Any, folder: str, work: Callable[[], Awaitable[dict]]) -> Job:
    """Run `work` in the background; the job is reported under its id."""
    job = Job(f"export-{next(_ids)}", kind, chat, folder, time.time())

    async def run() -> None:
        try:
            job.result = await work()
        except asyncio.CancelledError:
            job.cancelled = True
            raise
        except Exception as error:
            # The type and Telegram's own words; never a traceback or a file reference.
            job.error = f"{type(error).__name__}: {error}"
            log.warning("export %s failed: %s", job.id, type(error).__name__)

    job.task = asyncio.get_running_loop().create_task(run())
    _jobs[job.id] = job
    return job


def _files_in(folder: str) -> int:
    root = Path(folder)
    try:
        return sum(1 for p in root.rglob("*") if p.is_file()) if root.is_dir() else 0
    except OSError:
        return 0


def describe(job: Job) -> dict:
    if job.task is not None and not job.task.done():
        status = "running"
    elif job.cancelled:
        status = "cancelled"
    elif job.error is not None:
        status = "failed"
    else:
        status = "done"
    record = {
        "job_id": job.id,
        "kind": job.kind,
        "chat": job.chat,
        "status": status,
        "folder": job.folder,
        "elapsed_seconds": int(time.time() - job.started),
        "files_so_far": _files_in(job.folder),
    }
    if status == "done":
        record["result"] = job.result
    if status == "failed":
        record["error"] = job.error
    return record


def get(job_id: str) -> Optional[Job]:
    return _jobs.get(job_id)


def all_jobs() -> List[Job]:
    return list(_jobs.values())


def cancel(job_id: str) -> bool:
    job = _jobs.get(job_id)
    if job is None or job.task is None or job.task.done():
        return False
    job.cancelled = True  # a task cancelled before it ran never reaches its except
    job.task.cancel()
    return True


async def settle() -> None:
    """Wait for every job to end (tests, and a clean shutdown)."""
    tasks = [j.task for j in _jobs.values() if j.task is not None and not j.task.done()]
    if tasks:
        await asyncio.gather(*tasks, return_exceptions=True)
