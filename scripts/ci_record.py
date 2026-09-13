#!/usr/bin/env python3
"""What a CI leg ACTUALLY ran, as opposed to what its label says.

A job named "Python 3.11" installed 3.11 and then let ``uv run`` pick the
project-pinned 3.13 out of ``.python-version``; the matrix reported four legs
and proved two interpreters. The label is not evidence, so each leg records the
interpreter it ran on, the commit, the lockfile it resolved from, and how many
cases actually executed.

Two modes, because the two facts are known at different moments:

``--require <X.Y>``
    Before pytest, from inside the resolved environment: fail unless this really
    is the interpreter the leg is named after. A leg that cannot prove its label
    is worth less than no leg at all, because it also hides the gap.

``--junit <path>``
    After pytest: case counts and skips out of the report. A leg that "passed"
    by collecting nothing is the other way this goes quiet.

``--compare <dir>``
    After every leg: all the records together. A DROPPED TEST FILE is identical
    to success in every number except how many cases were collected - nothing
    goes red, the total is simply short, and `41 passed` beside one worker
    startup error reads exactly like `42 passed`. Every leg runs the same suite
    on the same commit, so they must agree on the collected count; skips may
    differ (the Windows leg skips fewer, having the renderer) and do not.

    Cross-leg agreement is used rather than a stored baseline on purpose: it
    needs no state carried between runs and no number to maintain, so adding
    tests never breaks it and losing a file on one leg always does.

Written to stdout always, and appended to ``$GITHUB_STEP_SUMMARY`` when GitHub
provides one.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import List, Optional, Tuple


def lock_digest(path: Path) -> str:
    """Which dependency set this leg resolved from, in twelve characters."""
    if not path.is_file():
        return "absent"
    return hashlib.sha256(path.read_bytes()).hexdigest()[:12]


def case_counts(junit: Path) -> Tuple[int, int, int, int]:
    """``(total, failures, errors, skipped)`` from a JUnit report.

    Raises rather than defaulting to zero: a report that cannot be read is not a
    run with no failures.
    """
    root = ET.parse(junit).getroot()
    suites = [root] if root.tag == "testsuite" else list(root.iter("testsuite"))
    if not suites:
        raise ValueError(f"{junit} contains no testsuite element")
    total = sum(int(s.get("tests", 0)) for s in suites)
    failures = sum(int(s.get("failures", 0)) for s in suites)
    errors = sum(int(s.get("errors", 0)) for s in suites)
    skipped = sum(int(s.get("skipped", 0)) for s in suites)
    return total, failures, errors, skipped


def write_record(path: Path, record: dict) -> None:
    """One leg's record, for the comparison after every leg has run."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, indent=2, sort_keys=True), encoding="utf-8")


def compare_legs(directory: Path) -> Tuple[List[str], bool]:
    """Every leg must have collected the same number of cases.

    Returns the report lines and whether anything is wrong. A leg missing
    entirely is the coverage gate's job - its platform-exclusive modules drop to
    near zero and the combined floor fails - so this only refuses to compare one
    record against nothing.
    """
    records = []
    for path in sorted(directory.rglob("*.json")):
        try:
            records.append((path.name, json.loads(path.read_text(encoding="utf-8"))))
        except (OSError, ValueError) as error:
            return [f"- **could not read {path.name}: {error}**"], True

    if len(records) < 2:
        return (
            [f"- **only {len(records)} leg record(s) found: there is nothing to compare.**"],
            True,
        )

    lines = [
        "### Cases collected per leg",
        "",
        "| leg | python | collected | skipped |",
        "|---|---|---|---|",
    ]
    for _, rec in records:
        lines.append(
            f"| {rec.get('label')} | {rec.get('python')} | "
            f"{rec.get('collected')} | {rec.get('skipped')} |"
        )

    counts = {rec.get("collected") for _, rec in records}
    if len(counts) > 1:
        lines += [
            "",
            f"- **the legs disagree on how many cases exist: {sorted(counts)}.** "
            "They ran the same suite on the same commit, so a leg that collected "
            "fewer lost a file rather than passing it - which looks identical to "
            "success in every other number.",
        ]
        return lines, True

    lines += ["", f"- every leg collected {counts.pop()} case(s)."]
    return lines, False


def _emit(lines: List[str]) -> None:
    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(text + "\n\n")


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", default=os.environ.get("GITHUB_JOB", "local"))
    parser.add_argument("--require", help="the X.Y this leg claims to be running")
    parser.add_argument("--junit", type=Path, help="a JUnit report to count cases from")
    parser.add_argument("--lock", type=Path, default=Path("uv.lock"))
    parser.add_argument("--emit", type=Path, help="write this leg's record here")
    parser.add_argument("--compare", type=Path, help="compare every leg record under here")
    args = parser.parse_args(argv)

    if args.compare is not None:
        lines, failed = compare_legs(args.compare)
        _emit(lines)
        return 1 if failed else 0

    running = ".".join(str(part) for part in sys.version_info[:2])
    lines = [
        f"### CI leg `{args.label}`",
        "",
        f"- interpreter: **{running}** (`{platform.python_version()}`, {sys.implementation.name})",
        f"- platform: {platform.system()} {platform.machine()} "
        f"(runner `{os.environ.get('RUNNER_OS', 'local')}`)",
        f"- commit: `{os.environ.get('GITHUB_SHA', 'local')[:12]}`",
        f"- uv.lock: `{lock_digest(args.lock)}`",
    ]

    failed = False
    if args.require and running != args.require:
        lines.append(
            f"- **MISMATCH: this leg is labelled {args.require} and is running {running}.** "
            "The environment did not select the interpreter the matrix installed."
        )
        failed = True

    if args.junit:
        try:
            total, failures, errors, skipped = case_counts(args.junit)
        except Exception as error:
            lines.append(f"- **could not read {args.junit}: {error}**")
            failed = True
        else:
            lines.append(
                f"- cases: {total} run, {failures} failed, {errors} errored, {skipped} skipped"
            )
            if total == 0:
                lines.append("- **no case executed: this leg proves nothing.**")
                failed = True
            if args.emit is not None:
                write_record(
                    args.emit,
                    {
                        "label": args.label,
                        "python": platform.python_version(),
                        "platform": os.environ.get("RUNNER_OS", platform.system()),
                        "commit": os.environ.get("GITHUB_SHA", "local"),
                        "lock": lock_digest(args.lock),
                        "collected": total,
                        "failures": failures,
                        "errors": errors,
                        "skipped": skipped,
                    },
                )

    _emit(lines)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
