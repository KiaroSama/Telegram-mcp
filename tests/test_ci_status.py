"""Manual CI reports each observed job, never inferred green."""

import importlib.util
import json

import pytest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SHA = "a" * 40
ENV = {
    "GITHUB_REPOSITORY": "example/project",
    "GITHUB_SHA": SHA,
    "GITHUB_RUN_ID": "12",
    "GITHUB_RUN_ATTEMPT": "2",
}


def reporter():
    path = REPO / "scripts/ci_status.py"
    assert path.is_file(), "Manual jobs need a commit-status reporter"
    spec = importlib.util.spec_from_file_location("ci_status", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_each_job_gets_pending_then_its_observed_result():
    module = reporter()
    writes = []
    run = {
        "head_sha": SHA,
        "run_attempt": 2,
        "event": "workflow_dispatch",
        "path": ".github/workflows/tests.yml",
    }
    jobs = [
        {
            "name": name,
            "run_id": 12,
            "run_attempt": 2,
            "head_sha": SHA,
            "status": "completed",
            "conclusion": outcome,
            "html_url": f"https://github.com/example/project/actions/runs/12/job/{i}",
        }
        for i, (name, outcome) in enumerate(
            [("Python 3.15", "success"), ("Coverage", "skipped")], 1
        )
    ]

    def request(method, path, payload=None):
        if method == "POST":
            assert path == f"/repos/example/project/statuses/{SHA}"
            writes.append(payload)
            return {}
        if "/attempts/2/jobs?" in path:
            return {"total_count": len(jobs), "jobs": jobs}
        return run

    inventory = {"tests.yml": ["Python 3.15", "Coverage"]}
    assert module.report(request, "begin", ENV, "tests.yml", inventory) == 0
    assert [s["state"] for s in writes] == ["pending", "pending"]
    writes.clear()
    assert module.report(request, "finish", ENV, "tests.yml", inventory) == 1
    assert [s["state"] for s in writes] == ["success", "error"]
    assert writes[0]["context"] == "CI/tests/Python 3.15"
    assert writes[0]["target_url"].endswith("/job/1")
    assert "attempt 2" in writes[0]["description"]
    jobs[1]["conclusion"] = "success"
    writes.clear()
    assert module.report(request, "finish", ENV, "tests.yml", inventory) == 0
    assert [s["state"] for s in writes] == ["success", "success"]


@pytest.mark.parametrize(
    "defect",
    [
        "missing",
        "wrong_sha",
        "wrong_run",
        "wrong_attempt",
        "duplicate",
        "cancelled",
        "failure",
        "unknown",
    ],
)
def test_incomplete_or_misattributed_evidence_never_goes_green(defect):
    module = reporter()
    writes = []
    job = {
        "name": "Test",
        "run_id": 12,
        "run_attempt": 2,
        "head_sha": SHA,
        "status": "completed",
        "conclusion": "success",
        "html_url": "https://github.com/example/project/actions/runs/12/job/1",
    }
    jobs = [job]
    if defect == "missing":
        jobs = []
    elif defect == "wrong_sha":
        job["head_sha"] = "b" * 40
    elif defect == "wrong_run":
        job["run_id"] = 13
    elif defect == "wrong_attempt":
        job["run_attempt"] = 1
    elif defect == "duplicate":
        jobs.append(dict(job))
    else:
        job["conclusion"] = defect

    def request(method, path, payload=None):
        if method == "POST":
            writes.append(payload)
            return {}
        if "/jobs?" in path:
            return {"total_count": len(jobs), "jobs": jobs}
        return {
            "head_sha": SHA,
            "run_attempt": 2,
            "event": "workflow_dispatch",
            "path": ".github/workflows/tests.yml",
        }

    assert module.report(request, "finish", ENV, "tests.yml", {"tests.yml": ["Test"]}) == 1
    assert writes[0]["state"] in {"error", "failure"}


def test_run_mismatch_and_api_error_refuse_before_success():
    module = reporter()
    writes = []

    def request(method, path, payload=None):
        if method == "POST":
            writes.append(payload)
        return {
            "head_sha": SHA,
            "run_attempt": 1,
            "event": "workflow_dispatch",
            "path": ".github/workflows/tests.yml",
        }

    with pytest.raises(ValueError, match="attempt"):
        module.report(request, "begin", ENV, "tests.yml", {"tests.yml": ["Test"]})
    assert not writes

    def unavailable(*args):
        raise OSError("API unavailable")

    with pytest.raises(OSError):
        module.report(unavailable, "begin", ENV, "tests.yml", {"tests.yml": ["Test"]})


def test_paginated_jobs_are_all_required_before_publishing():
    module = reporter()
    reads, writes = [], []

    def request(method, path, payload=None):
        if method == "POST":
            writes.append(payload)
            return {}
        if "/jobs?" not in path:
            return {
                "head_sha": SHA,
                "run_attempt": 2,
                "event": "workflow_dispatch",
                "path": ".github/workflows/tests.yml",
            }
        reads.append(path)
        index = len(reads)
        return {
            "total_count": 2,
            "jobs": [
                {
                    "name": f"Test {index}",
                    "head_sha": SHA,
                    "run_id": 12,
                    "run_attempt": 2,
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": f"https://github.com/example/project/actions/runs/12/job/{index}",
                }
            ],
        }

    assert (
        module.report(request, "finish", ENV, "tests.yml", {"tests.yml": ["Test 1", "Test 2"]})
        == 0
    )
    assert len(reads) == len(writes) == 2
    assert reads[-1].endswith("page=2")


def test_inventory_matches_every_expanded_workflow_job():
    import itertools
    import yaml

    inventory = json.loads((REPO / ".github/ci-status-jobs.json").read_text(encoding="utf-8"))
    for filename, expected in inventory.items():
        workflow = yaml.safe_load(
            (REPO / ".github/workflows" / filename).read_text(encoding="utf-8")
        )
        jobs = workflow["jobs"]
        assert jobs["status_finish"]["if"] == "always()"
        assert set(jobs["status_finish"]["needs"]) == set(jobs) - {"status_finish"}
        actual = []
        for key, job in jobs.items():
            if key.startswith("status_"):
                continue
            assert job.get("needs"), f"{filename}/{key} can start before pending reset"
            matrix = job.get("strategy", {}).get("matrix", {})
            for values in itertools.product(*matrix.values()) if matrix else [()]:
                name = job["name"]
                for axis, value in zip(matrix, values):
                    name = name.replace("${{ matrix." + axis + " }}", str(value))
                if matrix and "${{ matrix." not in job["name"]:
                    name += " (" + ", ".join(map(str, values)) + ")"
                actual.append(name)
        assert sorted(actual) == sorted(expected)
