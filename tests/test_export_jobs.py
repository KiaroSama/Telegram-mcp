"""Exports run as background jobs (spec 030 FR-014, owner 2026-10-01).

A Desktop export takes minutes; one tool call is cut at 55 s and most clients give up at a
minute. So the export tools start a job and answer at once; `export_status` reports it and
`cancel_export` stops it.
"""

import asyncio
import json

import pytest

from telegram_mcp import export_jobs
from telegram_mcp.tools import chat_export as tools


@pytest.fixture(autouse=True)
def _no_jobs_leak(monkeypatch):
    monkeypatch.setattr(export_jobs, "_jobs", {})


def _status(raw):
    return json.loads(raw)["results"]


@pytest.mark.asyncio
async def test_a_job_answers_at_once_and_reports_its_files_while_running(tmp_path):
    release = asyncio.Event()
    (tmp_path / "photos").mkdir()
    (tmp_path / "photos" / "a.jpg").write_bytes(b"x")

    async def work():
        await release.wait()
        return {"messages": 3}

    job = export_jobs.start("chat", "@news", tmp_path.as_posix() + "/", work)
    running = _status(await tools.export_status(job.id))

    assert running["status"] == "running" and running["files_so_far"] == 1
    release.set()
    await export_jobs.settle()
    done = _status(await tools.export_status(job.id))
    assert done["status"] == "done" and done["result"] == {"messages": 3}


@pytest.mark.asyncio
async def test_a_failed_job_says_why(tmp_path):
    async def work():
        raise RuntimeError("takeout refused twice")

    job = export_jobs.start("chat", "@news", tmp_path.as_posix(), work)
    await export_jobs.settle()

    failed = _status(await tools.export_status(job.id))
    assert failed["status"] == "failed" and "RuntimeError" in failed["error"]


@pytest.mark.asyncio
async def test_a_job_can_be_cancelled(tmp_path):
    async def work():
        await asyncio.sleep(3600)

    job = export_jobs.start("chat", "@news", tmp_path.as_posix(), work)
    said = _status(await tools.cancel_export(job.id))
    await export_jobs.settle()

    assert said["cancelled"] is True
    assert _status(await tools.export_status(job.id))["status"] == "cancelled"


@pytest.mark.asyncio
async def test_status_lists_every_job_and_names_an_unknown_one(tmp_path):
    async def work():
        return {}

    export_jobs.start("chat", "@news", tmp_path.as_posix(), work)
    await export_jobs.settle()

    assert len(_status(await tools.export_status())) == 1
    assert "No export" in await tools.export_status("export-999")
