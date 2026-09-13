#!/usr/bin/env python3
"""Refuse a credential anywhere in reachable history, not only at the tip.

A clean tip says nothing about a commit three months back, and a public
repository publishes both. ``scripts/secret_scan.py`` reads the working tree;
this reads every blob ``git rev-list --objects --all`` can reach.

It used to live inline in the workflow, and inline is how it came to fail OPEN.
Neither git call's return code was checked, so a run against an unreadable
directory or a non-repository got empty output, scanned zero blobs, printed "no
credential-shaped content in any reachable blob" and exited 0 - the most
reassuring sentence this program can produce, on the evidence of nothing. Three
rules follow from that, and they are why this is a file with tests:

* **A git command that fails is an error, never an empty result.** Both the
  enumeration and the dump are checked, and a scan that reached no blob at all
  in a repository that lists objects is itself a failure.
* **A record is parsed completely or not at all.** A truncated ``--batch``
  stream ended the old loop quietly at whatever byte it stopped on, leaving the
  remaining objects unscanned and unreported.
* **Memory is bounded.** The whole of history was read into one bytes object.
  Blobs are streamed and matched over a sliding window instead.

Reporting stays sanitized: the path, the abbreviated object id and the KIND of
credential, never the matched bytes.
"""

from __future__ import annotations

import re
import subprocess
import sys
from typing import BinaryIO, Dict, Iterable, Iterator, List, Optional, Tuple

# Shapes, not names: the variable a secret is assigned to is not what makes it
# one. Kept in step with the credential patterns in scripts/secret_scan.py.
PATTERNS = {
    "Telegram bot token": rb"\b\d{8,10}:[A-Za-z0-9_-]{30,45}\b",
    "Telethon session string": rb"\b1[A-Za-z0-9+/_-]{200,}\b",
    "private key block": rb"-----BEGIN [A-Z ]*PRIVATE KEY-----",
    "AWS access key": rb"\bAKIA[0-9A-Z]{16}\b",
    "GitHub token": rb"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
}
COMPILED = {name: re.compile(pattern) for name, pattern in PATTERNS.items()}

# One megabyte at a time, with an overlap wide enough that a credential lying
# across a boundary still forms one contiguous run inside some window. The
# longest shape above is the session string; 64 KiB is far past any of them.
_CHUNK = 1 << 20
_OVERLAP = 1 << 16


class ScanError(RuntimeError):
    """The scan could not be completed - which is not the same as finding nothing."""


def _check(result, what: str) -> None:
    if result.returncode != 0:
        detail = (result.stderr or b"").decode("utf-8", "replace").strip()
        raise ScanError(f"{what} failed (exit {result.returncode}): {detail or 'no output'}")


def object_names(run=subprocess.run, cwd: Optional[str] = None) -> Dict[str, str]:
    """Every reachable object id mapped to the path it was last listed under.

    The path is for the report only. An object with no name is still scanned: a
    blob whose path the tip no longer shows is exactly the interesting case.
    """
    result = run(["git", "rev-list", "--objects", "--all"], capture_output=True, cwd=cwd)
    _check(result, "git rev-list --objects --all")
    names: Dict[str, str] = {}
    for line in (result.stdout or b"").decode("utf-8", "replace").splitlines():
        oid, _, path = line.partition(" ")
        if oid:
            names[oid] = path.strip() or names.get(oid, "")
    return names


def iter_batch_records(stream: BinaryIO) -> Iterator[Tuple[str, str, bytes]]:
    """Parse ``git cat-file --batch-all-objects --batch`` into whole records.

    Yields ``(oid, kind, body)``. An unparsable header, a body shorter than its
    declared size, or a missing record terminator raises instead of ending the
    loop: a stream that stopped early left objects unscanned, and the old code
    reported that as a clean history.
    """
    while True:
        header = stream.readline()
        if not header:
            return
        fields = header.split()
        if len(fields) == 2 and fields[1] in (b"missing", b"ambiguous"):
            raise ScanError(f"git reported object {fields[0].decode()} as {fields[1].decode()}")
        if len(fields) < 3:
            raise ScanError(f"unparsable record header: {header[:120]!r}")
        oid, kind = fields[0].decode(), fields[1].decode()
        try:
            size = int(fields[2])
        except ValueError as error:
            raise ScanError(f"unparsable size in record header: {header[:120]!r}") from error
        body = stream.read(size)
        if len(body) != size:
            raise ScanError(
                f"object {oid[:12]} was truncated: {len(body)} of {size} byte(s) arrived"
            )
        if stream.read(1) != b"\n":
            raise ScanError(f"object {oid[:12]} was not terminated by a newline")
        yield oid, kind, body


def credential_kinds(body: bytes) -> List[str]:
    """The credential KINDS this blob carries. Never the matched bytes."""
    found: List[str] = []
    start = 0
    while True:
        window = body[start : start + _CHUNK]
        if not window:
            break
        for name, pattern in COMPILED.items():
            if name not in found and pattern.search(window):
                found.append(name)
        if len(found) == len(COMPILED) or start + _CHUNK >= len(body):
            break
        start += _CHUNK - _OVERLAP
    return found


def scan_records(
    records: Iterable[Tuple[str, str, bytes]], names: Dict[str, str]
) -> Tuple[List[str], int]:
    findings: List[str] = []
    scanned = 0
    for oid, kind, body in records:
        if kind != "blob":
            continue
        scanned += 1
        where = names.get(oid) or "(unnamed object)"
        for name in credential_kinds(body):
            findings.append(f"{where} (object {oid[:12]}): {name}")
    return sorted(set(findings)), scanned


def scan_history(
    cwd: Optional[str] = None, run=subprocess.run, popen=subprocess.Popen
) -> Tuple[List[str], int]:
    """Findings and the number of blobs actually read. Raises on any failure."""
    names = object_names(run=run, cwd=cwd)
    process = popen(
        ["git", "cat-file", "--batch-all-objects", "--batch", "--unordered"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=cwd,
    )
    try:
        findings, scanned = scan_records(iter_batch_records(process.stdout), names)
    except BaseException:
        # The dump is long-running; abandoning it mid-stream would leave git
        # writing into a pipe nobody reads for the life of the job.
        process.kill()
        process.wait()
        raise
    finally:
        if process.stdout is not None:
            process.stdout.close()
    stderr = process.stderr.read() if process.stderr is not None else b""
    if process.stderr is not None:
        process.stderr.close()
    code = process.wait()
    if code != 0:
        detail = stderr.decode("utf-8", "replace").strip()
        raise ScanError(f"git cat-file failed (exit {code}): {detail or 'no output'}")
    # Enumeration found objects and the dump yielded no blob: the two halves
    # disagree, which is a broken scan wearing the shape of a clean one.
    if names and scanned == 0:
        raise ScanError(
            f"git listed {len(names)} reachable object(s) but the dump yielded no blob; "
            "the scan did not cover history"
        )
    return findings, scanned


def main(argv: Optional[List[str]] = None) -> int:
    try:
        findings, scanned = scan_history()
    except ScanError as error:
        # Exit 2, not 1: "could not check" and "found a credential" are different
        # answers, and a workflow that conflates them learns nothing from either.
        print(f"history scan could not be completed: {error}", file=sys.stderr)
        return 2
    print(f"scanned {scanned} blob(s) across all refs")
    if findings:
        print("history carries credential-shaped content:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("no credential-shaped content in any reachable blob")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
