#!/usr/bin/env python3
"""Refuse a credential anywhere in reachable history, not only at the tip.

A clean tip says nothing about a commit three months back, and a public
repository publishes both. ``scripts/secret_scan.py`` reads the working tree;
this reads every object ``git cat-file --batch-all-objects`` can produce.

It used to live inline in the workflow, and inline is how it came to fail OPEN:
neither git call's return code was checked, so a run against an unreadable
directory scanned zero blobs and announced a clean history. That is fixed, and
three more things were wrong with the fix:

* **Memory was bounded in intent only.** Reading the dump record by record
  instead of all at once still called ``read(size)`` for each BODY, so one
  32 MiB blob was one 32 MiB allocation. Bodies are now read in fixed chunks and
  matched as they go, with an overlap wide enough that a credential lying across
  a chunk boundary is still one contiguous run inside some window - without it,
  a secret is missed because of where the divisions happened to fall.
* **stderr was read after stdout.** That deadlocks the moment git fills its
  stderr pipe: the child blocks writing, this blocks reading the dump, and
  neither moves again. A thread drains stderr concurrently.
* **An early end looked like a complete scan.** A stream that simply stops after
  one of two objects produced no error and no finding. Every object the
  enumeration named must appear in the dump, or this says it did not cover
  history.

**Reachable by default, and that is a decision rather than an oversight.**
``--batch-all-objects`` produces UNREACHABLE objects too - an amended commit's
old blob, something staged and never committed, whatever a deleted branch left
behind. Those are NOT published: a clone fetches refs, so an object no ref
reaches never leaves this machine, and ``git gc`` eventually removes it. Failing
the repository's push gate on one would be failing it for something the
repository does not contain.

They are still worth knowing about, because an object that is unreachable today
is one ``git reflog`` or one ``--mirror`` push away from not being, so
``--include-unreachable`` scans them as a local audit and says which findings
came from where. The default - what CI runs - answers the question the gate is
for: is there a credential in what this repository PUBLISHES.

Reporting stays sanitized: the path, the abbreviated object id and the KIND of
credential, never the matched bytes.
"""

from __future__ import annotations

import io
import re
import subprocess
import sys
import threading
import time
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

# A `<oid> <type> <size>` line is well under a hundred bytes. Anything longer is
# not a header, and reading until a newline that may never come is how a parser
# turns a malformed stream into an allocation.
_MAX_HEADER_BYTES = 4096

# No single object is read past this. Git's own limit is far higher; this is the
# point past which a declared size is more likely to be a corrupted stream than
# a real blob, and reading it would be believing that stream about how much
# memory to use.
_MAX_OBJECT_BYTES = 2 << 30

# Ceilings for the scan itself. A repository this does not finish reading is a
# repository whose history it cannot vouch for, and saying so beats hanging.
_SCAN_DEADLINE_SECONDS = 900.0
_STDERR_LIMIT_BYTES = 1 << 20


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
    result = run(
        ["git", "rev-list", "--objects", "--all"],
        capture_output=True,
        cwd=cwd,
        timeout=_SCAN_DEADLINE_SECONDS,
    )
    _check(result, "git rev-list --objects --all")
    names: Dict[str, str] = {}
    for line in (result.stdout or b"").decode("utf-8", "replace").splitlines():
        oid, _, path = line.partition(" ")
        if oid:
            names[oid] = path.strip() or names.get(oid, "")
    return names


def _scan_stream(stream: BinaryIO, size: int, oid: str, match: bool) -> List[str]:
    """Read ``size`` bytes in chunks, matching as they go. Never holds them all.

    The overlap is what makes chunking safe. Without it a credential split
    across a boundary is two fragments, neither of which matches, and the scan
    reports clean because of where the divisions happened to fall.
    """
    found: List[str] = []
    carry = b""
    remaining = size
    while remaining > 0:
        piece = stream.read(min(_CHUNK, remaining))
        if not piece:
            raise ScanError(
                f"object {oid[:12]} was truncated: {size - remaining} of {size} byte(s) arrived"
            )
        remaining -= len(piece)
        if not match:
            continue
        window = carry + piece
        for name, pattern in COMPILED.items():
            if name not in found and pattern.search(window):
                found.append(name)
        carry = window[-_OVERLAP:]
    return found


def iter_batch_records(stream: BinaryIO) -> Iterator[Tuple[str, str, List[str]]]:
    """Parse ``git cat-file --batch-all-objects --batch``, a chunk at a time.

    Yields ``(oid, kind, credential_kinds)`` - the MATCHES rather than the body,
    because the body is never held whole.

    An unparsable header, an implausible size, a body shorter than its declared
    size, or a missing record terminator raises instead of ending the loop: a
    stream that stopped early left objects unscanned, and that was reported as a
    clean history.
    """
    while True:
        header = stream.readline(_MAX_HEADER_BYTES + 1)
        if not header:
            return
        if len(header) > _MAX_HEADER_BYTES:
            raise ScanError(f"record header over {_MAX_HEADER_BYTES} bytes: {header[:80]!r}")
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
        if size < 0 or size > _MAX_OBJECT_BYTES:
            raise ScanError(
                f"object {oid[:12]} declares {size} bytes, outside anything this scanner "
                f"will read (ceiling {_MAX_OBJECT_BYTES})"
            )
        found = _scan_stream(stream, size, oid, match=(kind == "blob"))
        if stream.read(1) != b"\n":
            raise ScanError(f"object {oid[:12]} was not terminated by a newline")
        yield oid, kind, found


def credential_kinds(body: bytes) -> List[str]:
    """The credential KINDS a buffer carries. Never the matched bytes.

    For a caller that already holds the bytes. The scan itself goes through
    ``_scan_stream``, which never has them all at once.
    """
    return _scan_stream(io.BytesIO(body), len(body), "in-memory", match=True)


def scan_records(
    records: Iterable[Tuple[str, str, List[str]]],
    names: Dict[str, str],
    include_unreachable: bool = False,
) -> Tuple[List[str], int, set]:
    """Findings, the number of blobs read, and every object id the dump reported.

    The third value is the inventory: a dump that stops after one of two objects
    is indistinguishable from a complete one unless something compares what came
    back against what was asked for.
    """
    findings: List[str] = []
    scanned = 0
    seen = set()
    for oid, kind, kinds in records:
        seen.add(oid)
        if kind != "blob":
            continue
        reachable = oid in names
        if not reachable and not include_unreachable:
            # Not published: no ref reaches it, so no clone receives it. Counted
            # as seen, because the inventory check is about the stream being
            # complete, not about what was matched.
            continue
        scanned += 1
        where = names.get(oid) or "(unreachable object, not published)"
        for name in kinds:
            findings.append(f"{where} (object {oid[:12]}): {name}")
    return sorted(set(findings)), scanned, seen


def _drain(stream, into: List[bytes]) -> None:
    """Read stderr to the end, on its own thread.

    Concurrently, because the alternative deadlocks: git blocks writing into a
    full stderr pipe while this blocks reading stdout, and neither ever moves.
    """
    try:
        while True:
            piece = stream.read(65536)
            if not piece:
                return
            if sum(len(p) for p in into) < _STDERR_LIMIT_BYTES:
                into.append(piece)
    except Exception:
        return


def scan_history(
    cwd: Optional[str] = None,
    run=subprocess.run,
    popen=subprocess.Popen,
    include_unreachable: bool = False,
) -> Tuple[List[str], int]:
    """Findings and the number of blobs actually read. Raises on any failure.

    ``include_unreachable`` widens the scan from what the repository publishes
    to everything its object store happens to hold. See the module docstring for
    why the two are different questions.
    """
    names = object_names(run=run, cwd=cwd)
    process = popen(
        ["git", "cat-file", "--batch-all-objects", "--batch", "--unordered"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=cwd,
    )
    collected: List[bytes] = []
    drainer = None
    if process.stderr is not None:
        drainer = threading.Thread(target=_drain, args=(process.stderr, collected), daemon=True)
        drainer.start()

    started = time.monotonic()
    try:
        findings, scanned, seen = scan_records(
            iter_batch_records(process.stdout), names, include_unreachable
        )
    except BaseException:
        # The dump is long-running; abandoning it mid-stream would leave git
        # writing into a pipe nobody reads for the life of the job.
        _terminate(process)
        raise
    finally:
        if process.stdout is not None:
            process.stdout.close()

    if time.monotonic() - started > _SCAN_DEADLINE_SECONDS:
        _terminate(process)
        raise ScanError(f"the history scan exceeded {_SCAN_DEADLINE_SECONDS:.0f}s")

    try:
        code = process.wait(timeout=60)
    except subprocess.TimeoutExpired as error:
        _terminate(process)
        raise ScanError("git cat-file did not exit after its output ended") from error
    if drainer is not None:
        drainer.join(timeout=10)
    stderr = b"".join(collected)
    if process.stderr is not None:
        try:
            process.stderr.close()
        except Exception:
            pass
    if code != 0:
        detail = stderr.decode("utf-8", "replace").strip()
        raise ScanError(f"git cat-file failed (exit {code}): {detail or 'no output'}")

    # The inventory check. Enumeration found objects and the dump yielded no
    # blob, or yielded fewer objects than were named: either way the two halves
    # disagree, which is a broken scan wearing the shape of a clean one.
    if names and scanned == 0:
        raise ScanError(
            f"git listed {len(names)} reachable object(s) but the dump yielded no blob; "
            "the scan did not cover history"
        )
    unchecked = sorted(set(names) - seen)
    if unchecked:
        raise ScanError(
            f"{len(unchecked)} object(s) the enumeration named never appeared in the dump "
            f"(first: {unchecked[0][:12]}); the scan did not cover history"
        )
    return findings, scanned


def _terminate(process) -> None:
    """End the child and everything it is holding, without waiting forever."""
    try:
        process.kill()
    except Exception:
        pass
    try:
        process.wait(timeout=30)
    except Exception:
        pass


def main(argv: Optional[List[str]] = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    include_unreachable = "--include-unreachable" in argv
    try:
        findings, scanned = scan_history(include_unreachable=include_unreachable)
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
