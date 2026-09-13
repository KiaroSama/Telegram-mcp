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

``--suites <file>``
    The half cross-leg agreement cannot reach: a suite deleted EVERYWHERE at
    once. Every leg then collects the same smaller number and agrees perfectly.
    So the count of tracked `tests/test_*.py` files is committed, and removing
    one fails until the removal is declared in the same commit - which is the
    only signal that separates a deliberate deletion from a lost file.

Alongside it, whenever a JUnit report is read, every tracked suite must have
contributed at least one case. A file that is still on disk and still tracked
but silently collects nothing - a bad import, a renamed class, a conftest filter
- is invisible to both counts, because it was never counted to begin with.

Written to stdout always, and appended to ``$GITHUB_STEP_SUMMARY`` when GitHub
provides one.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Dict, List, Optional, Tuple


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


# Listing tracked files is a local index read; a minute is far past any of it and
# still bounded, which is what `tests/test_secret_scan_gate.py` requires of every
# subprocess under `scripts/`.
_GIT_TIMEOUT_SECONDS = 60


class SuiteError(RuntimeError):
    """The tracked suite list could not be established - not the same as empty."""


def tracked_suites() -> List[str]:
    """Every `tests/test_*.py` git tracks, as the module path JUnit reports.

    From git rather than a directory walk: an untracked scratch file in `tests/`
    is not a suite this repository owns, and a tracked file missing from disk is
    a problem the walk would never see.
    """
    result = subprocess.run(
        ["git", "ls-files", "tests/test_*.py"],
        capture_output=True,
        text=True,
        timeout=_GIT_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        raise SuiteError(f"git ls-files failed: {result.stderr.strip() or 'no output'}")
    names = [
        line[: -len(".py")].replace("/", ".")
        for line in result.stdout.splitlines()
        if line.strip()
    ]
    if not names:
        raise SuiteError("git tracks no tests/test_*.py at all")
    return sorted(names)


def expected_suite_count(path: Path) -> int:
    """The committed count, ignoring the comment block that explains it."""
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            return int(line)
    raise SuiteError(f"{path} carries no number")


def silent_suites(junit: Path, tracked: List[str]) -> List[str]:
    """Tracked suites that contributed no case to this report.

    A classname is the module for a bare function and `module.Class` for a method,
    so a suite has spoken if any classname IS its module or starts with it plus a
    dot. Matching the whole string called `tests/test_sanitize.py` silent on the
    first CI run - 34 passing tests, every one inside a class - and a false
    positive is how a real finding gets scrolled past.
    """
    root = ET.parse(junit).getroot()
    spoke = {case.get("classname", "") for case in root.iter("testcase")}
    return [
        name
        for name in tracked
        if not any(said == name or said.startswith(name + ".") for said in spoke)
    ]


def write_record(path: Path, record: dict) -> None:
    """One leg's record, for the comparison after every leg has run."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, indent=2, sort_keys=True), encoding="utf-8")


def expected_legs(path: Path) -> Dict[str, str]:
    """The legs this workflow must produce, label -> expected X.Y.

    A committed manifest rather than "whatever turned up", because agreement
    between legs is only as strong as how many legs there are: two empty records
    agree perfectly, and so do two of six. `tests/test_ci_record.py` checks this
    file against the workflow's own matrix so the two cannot drift.
    """
    legs: Dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) != 2:
            raise ValueError(f"unparsable leg line: {line!r}")
        legs[parts[0]] = parts[1]
    if not legs:
        raise ValueError(f"{path} names no legs")
    return legs


def _valid_record(rec) -> Optional[str]:
    """Why this record cannot be believed, or ``None``.

    Every field is checked for SHAPE, not only presence. A record carrying
    `collected: 0`, or a negative skip count, or no commit at all, used to be
    compared against its siblings as though it meant something.
    """
    if not isinstance(rec, dict):
        return "it is not an object"
    label = rec.get("label")
    if not isinstance(label, str) or not label:
        return "it has no label"
    collected = rec.get("collected")
    if not isinstance(collected, int) or isinstance(collected, bool) or collected <= 0:
        return f"collected is {collected!r}, which is not a positive whole number"
    for name in ("failures", "errors", "skipped"):
        value = rec.get(name)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            return f"{name} is {value!r}, which is not a whole number of cases"
    for name in ("commit", "lock", "python", "platform"):
        if not isinstance(rec.get(name), str) or not rec.get(name):
            return f"it records no {name}"
    return None


def compare_legs(directory: Path, manifest: Optional[Path] = None) -> Tuple[List[str], bool]:
    """Hold the run to the legs it is supposed to have, and to one provenance.

    Agreement between whatever records happened to arrive is a weak claim, and
    every way it was weak is checked here now: two empty records agreed, two of
    six agreed, a duplicated label counted twice, and records from different
    commits or different lockfiles were compared against each other as though
    they described the same run.
    """
    records = []
    for path in sorted(directory.rglob("*.json")):
        try:
            records.append((path.name, json.loads(path.read_text(encoding="utf-8"))))
        except (OSError, ValueError) as error:
            return [f"- **could not read {path.name}: {error}**"], True

    problems: List[str] = []
    for name, rec in records:
        wrong = _valid_record(rec)
        if wrong is not None:
            problems.append(f"- **{name} cannot be believed: {wrong}.**")
    if problems:
        return problems, True

    if manifest is None and len(records) < 2:
        return (
            [f"- **only {len(records)} leg record(s) found: there is nothing to compare.**"],
            True,
        )

    labels = [rec["label"] for _, rec in records]
    duplicated = sorted({label for label in labels if labels.count(label) > 1})
    if duplicated:
        problems.append(
            f"- **two records claim the same leg: {duplicated}.** One of them is "
            "describing a run that is not this one."
        )

    if manifest is not None:
        try:
            wanted = expected_legs(manifest)
        except (OSError, ValueError) as error:
            return [f"- **the leg manifest could not be read: {error}**"], True
        missing = sorted(set(wanted) - set(labels))
        extra = sorted(set(labels) - set(wanted))
        if missing:
            problems.append(
                f"- **{len(missing)} expected leg(s) produced no record: {missing}.** "
                "A run missing legs is not a run that passed - the ones that did "
                "report agree with each other perfectly and prove nothing about these."
            )
        if extra:
            problems.append(f"- **unexpected leg(s) reported: {extra}.**")
        for _, rec in records:
            want = wanted.get(rec["label"])
            if want and not str(rec["python"]).startswith(want + "."):
                problems.append(
                    f"- **leg `{rec['label']}` is named after {want} and ran "
                    f"{rec['python']}.**"
                )

    for field, what in (("commit", "commit"), ("lock", "lockfile")):
        values = {rec[field] for _, rec in records}
        if len(values) > 1:
            problems.append(
                f"- **the legs describe different {what}s: {sorted(values)}.** They are "
                "not results about one run, so agreeing about anything means nothing."
            )

    lines = [
        "### Cases collected per leg",
        "",
        "| leg | python | platform | collected | skipped |",
        "|---|---|---|---|---|",
    ]
    for _, rec in sorted(records, key=lambda pair: pair[1]["label"]):
        lines.append(
            f"| {rec['label']} | {rec['python']} | {rec['platform']} | "
            f"{rec['collected']} | {rec['skipped']} |"
        )

    counts = {rec["collected"] for _, rec in records}
    if len(counts) > 1:
        problems.append(
            f"- **the legs disagree on how many cases exist: {sorted(counts)}.** "
            "They ran the same suite on the same commit, so a leg that collected "
            "fewer lost a file rather than passing it - which looks identical to "
            "success in every other number."
        )

    if problems:
        return lines + [""] + problems, True
    lines += ["", f"- {len(records)} leg(s), all collecting {counts.pop()} case(s)."]
    return lines, False


def check_suite_count(expected_file: Path) -> Tuple[List[str], bool]:
    """The count of tracked suites must match the committed number.

    Cross-leg agreement cannot see a suite deleted everywhere at once - every leg
    collects the same smaller number and agrees. This is the control that makes
    such a removal say so.
    """
    try:
        tracked = tracked_suites()
        expected = expected_suite_count(expected_file)
    except (SuiteError, OSError, ValueError) as error:
        return [f"- **could not check the tracked suite count: {error}**"], True

    if len(tracked) == expected:
        return [f"- {len(tracked)} tracked test file(s), as declared."], False

    verb = "removed" if len(tracked) < expected else "added"
    return (
        [
            f"- **{expected} test file(s) are declared in {expected_file}, and "
            f"{len(tracked)} are tracked: {abs(len(tracked) - expected)} were {verb}.** "
            "If that is deliberate, update that file in the SAME commit - it is the "
            "only thing separating a suite someone meant to drop from one that was lost."
        ],
        True,
    )


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
    parser.add_argument(
        "--legs", type=Path, help="the committed manifest of legs the run must produce"
    )
    parser.add_argument(
        "--suites", type=Path, help="the committed count of tracked test files to hold to"
    )
    parser.add_argument(
        "--all-suites",
        action="store_true",
        help="this report covers the WHOLE suite, so every tracked file must appear in it",
    )
    args = parser.parse_args(argv)

    if args.suites is not None:
        lines, failed = check_suite_count(args.suites)
        _emit(lines)
        if args.compare is None:
            return 1 if failed else 0
        more, worse = compare_legs(args.compare, args.legs)
        _emit(more)
        return 1 if (failed or worse) else 0

    if args.compare is not None:
        lines, failed = compare_legs(args.compare, args.legs)
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
            # Only against a whole-suite report. Asked for explicitly, because a
            # report from `pytest tests/test_one.py` legitimately mentions one
            # file, and treating that as 144 silent suites would be noise that
            # trains everyone to pass the flag off.
            try:
                quiet = silent_suites(args.junit, tracked_suites()) if args.all_suites else []
            except (SuiteError, OSError, ET.ParseError) as error:
                lines.append(f"- **could not establish the tracked suites: {error}**")
                failed = True
            else:
                if quiet:
                    shown = ", ".join(quiet[:8])
                    more = "" if len(quiet) <= 8 else f" (and {len(quiet) - 8} more)"
                    lines.append(
                        f"- **{len(quiet)} tracked suite(s) contributed no case: {shown}{more}.** "
                        "A file that is still tracked but collects nothing is invisible to "
                        "every count, because it was never counted to begin with."
                    )
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
