#!/usr/bin/env python3
"""Expose manual Actions jobs in the commit rollup using observed evidence only."""

import argparse
import json
import os
import re
import sys
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

REPORTER_NAMES = {
    f"{phase} / Commit status publisher" for phase in ("CI status pending", "CI status final")
}


def report(request, phase, env, workflow, inventory):
    """Publish through an injectable GitHub REST transport; return actual outcome."""
    repo, sha = env["GITHUB_REPOSITORY"], env["GITHUB_SHA"]
    run_id, attempt = env["GITHUB_RUN_ID"], env["GITHUB_RUN_ATTEMPT"]
    if (
        not re.fullmatch(r"[\w.-]+/[\w.-]+", repo)
        or not re.fullmatch(r"[0-9a-f]{40}", sha)
        or not run_id.isdecimal()
        or not attempt.isdecimal()
        or int(run_id) < 1
        or int(attempt) < 1
        or phase not in {"begin", "finish"}
    ):
        raise ValueError("Invalid CI identity or phase")
    names = inventory[workflow]
    if not names or len(names) != len(set(names)):
        raise ValueError("Empty or duplicate job inventory")
    base = f"/repos/{repo}"
    run_path = f"{base}/actions/runs/{run_id}"
    run = request("GET", run_path)
    if (
        run["head_sha"] != sha
        or run["run_attempt"] != int(attempt)
        or run["event"] != "workflow_dispatch"
        or run["path"] != f".github/workflows/{workflow}"
    ):
        raise ValueError("Workflow run does not match this commit and attempt")
    run_url = f"https://github.com/{repo}/actions/runs/{run_id}"

    def publish(name, state, outcome, url):
        request(
            "POST",
            f"{base}/statuses/{sha}",
            {
                "context": f"CI/{Path(workflow).stem}/{name}",
                "state": state,
                "description": f"Run {run_id}, attempt {attempt}: {outcome}"[:140],
                "target_url": url,
            },
        )
        print(f"{name}: {state} ({outcome})", flush=True)

    if phase == "begin":
        for name in names:
            publish(name, "pending", "awaiting observed job result", run_url)
        return 0
    jobs = []
    for page in range(1, 11):
        response = request("GET", f"{run_path}/attempts/{attempt}/jobs?per_page=100&page={page}")
        batch = response["jobs"]
        jobs.extend(batch)
        if len(jobs) == response["total_count"]:
            break
        if not batch or len(jobs) > response["total_count"]:
            raise ValueError("Incomplete or inconsistent jobs response")
    else:
        raise ValueError("Jobs exceed bounded pagination")
    actual, unexpected = {}, False
    for job in jobs:
        name = job["name"]
        if name in REPORTER_NAMES:
            continue
        if name not in names or name in actual:
            unexpected = True
        actual[name] = job
    failed = unexpected
    for name in names:
        job = actual.get(name)
        state, outcome, url = "error", "missing job", run_url
        if job:
            url = job["html_url"]
            valid = (
                job["run_id"] == int(run_id)
                and job["head_sha"] == sha
                and job.get("run_attempt") == int(attempt)
                and re.fullmatch(re.escape(run_url) + r"/job/\d+", url)
            )
            outcome = job.get("conclusion") or job["status"]
            if valid and job["status"] == "completed":
                if outcome == "success" and not unexpected:
                    state = "success"
                elif outcome in {"failure", "timed_out"}:
                    state = "failure"
            if not valid:
                outcome, url = "job identity mismatch", run_url
            elif unexpected:
                outcome = "unexpected or duplicate validation job"
        publish(name, state, outcome, url)
        failed |= state != "success"
    return int(failed)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("begin", "finish"))
    parser.add_argument("workflow")
    args = parser.parse_args(argv)

    def request(method, path, payload=None):
        body = None if payload is None else json.dumps(payload).encode("utf-8")
        req = Request(
            "https://api.github.com" + path,
            data=body,
            method=method,
            headers={
                "Authorization": f"Bearer {os.environ['GH_TOKEN']}",
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
                "Content-Type": "application/json",
            },
        )
        with urlopen(req, timeout=15) as response:
            return json.load(response)

    try:
        inventory = json.loads(
            (Path(__file__).resolve().parents[1] / ".github/ci-status-jobs.json").read_text(
                encoding="utf-8"
            )
        )
        return report(request, args.phase, os.environ, args.workflow, inventory)
    except (HTTPError, URLError, OSError, ValueError, KeyError, TypeError) as exc:
        # Never print API bodies, headers, tokens or request payloads.
        print(f"Commit status reporting refused: {type(exc).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
