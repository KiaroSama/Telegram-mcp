"""The history scan must fail CLOSED, and stream what it reads.

It lived inline in the workflow with neither git call's return code checked, so
a run that could not read the repository at all printed "no credential-shaped
content in any reachable blob" and exited 0. Fixing that left three more ways to
be wrong, and every one of them is a test here:

* **The body was read whole.** Record-at-a-time parsing still called
  `read(size)` per object, so a 32 MiB blob was a 32 MiB allocation.
* **stderr was read after stdout.** That deadlocks the instant git fills its
  stderr pipe - the child blocks writing, the scan blocks reading, neither moves.
* **An early end looked like a complete scan.** A stream that stops after one of
  two objects produced no error and no finding.

The clean-repository and synthetic-marker cases build a real git repository in a
temporary directory: the parser is tested against git's actual `--batch` output,
not against a fixture of what it is believed to look like.
"""

import io
import re
import shutil
import subprocess
import sys
import tracemalloc
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

import history_secret_scan as scan  # noqa: E402

pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="git is not installed")

# Shaped like the real thing and matched by the same pattern, but not a
# credential: sixteen upper-case characters after a marker this project never
# issues.
SYNTHETIC_MARKER = b"AKIA" + b"TESTONLY00000000"


def _repo(tmp_path: Path, files: dict) -> Path:
    root = tmp_path / "repo"
    root.mkdir()

    def git(*args):
        subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)

    git("init", "-q")
    git("config", "user.email", "test@example.invalid")
    git("config", "user.name", "test")
    git("config", "commit.gpgsign", "false")
    for name, content in files.items():
        (root / name).write_bytes(content)
    git("add", "-A")
    git("commit", "-q", "-m", "fixture")
    return root


def _record(oid: str, kind: str, body: bytes) -> bytes:
    return f"{oid} {kind} {len(body)}\n".encode() + body + b"\n"


# --- a real repository ---------------------------------------------------------


def test_a_clean_repository_passes_and_says_what_it_read(tmp_path):
    root = _repo(tmp_path, {"readme.txt": b"nothing here\n"})

    findings, scanned = scan.scan_history(cwd=str(root))

    assert findings == []
    assert scanned >= 1, "a repository with a committed file must yield at least one blob"


def test_a_credential_shape_in_history_is_found_by_path(tmp_path):
    root = _repo(tmp_path, {"config.txt": b"key = " + SYNTHETIC_MARKER + b"\n"})

    findings, _ = scan.scan_history(cwd=str(root))

    assert len(findings) == 1
    assert "config.txt" in findings[0] and "AWS access key" in findings[0]


def test_a_credential_in_an_older_commit_with_a_clean_tip_is_found(tmp_path):
    """The whole reason this exists: the tip being clean proves nothing."""
    root = _repo(tmp_path, {"leak.txt": b"key = " + SYNTHETIC_MARKER + b"\n"})
    (root / "leak.txt").unlink()
    subprocess.run(["git", "add", "-A"], cwd=root, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-q", "-m", "remove it"], cwd=root, check=True, capture_output=True
    )

    findings, _ = scan.scan_history(cwd=str(root))

    assert findings, "a blob removed at the tip is still reachable and still published"


def test_the_report_never_prints_the_matched_bytes(tmp_path):
    root = _repo(tmp_path, {"config.txt": b"key = " + SYNTHETIC_MARKER + b"\n"})

    findings, _ = scan.scan_history(cwd=str(root))

    assert SYNTHETIC_MARKER not in " ".join(findings).encode()


def test_a_directory_that_is_not_a_repository_fails_rather_than_reporting_clean(tmp_path):
    """The original defect. This used to exit 0 having scanned nothing."""
    with pytest.raises(scan.ScanError) as raised:
        scan.scan_history(cwd=str(tmp_path))

    assert "rev-list" in str(raised.value)


# --- what is published, and what merely sits in the object store ---------------


def test_an_unreachable_object_is_not_a_finding_by_default(tmp_path):
    """A clone fetches refs, so an object no ref reaches never leaves the
    machine. Failing the push gate on one fails it for something the repository
    does not contain."""
    root = _repo(tmp_path, {"readme.txt": b"nothing\n"})
    written = subprocess.run(
        ["git", "hash-object", "-w", "--stdin"],
        cwd=root,
        input=b"key = " + SYNTHETIC_MARKER + b"\n",
        capture_output=True,
        check=True,
    )
    assert written.stdout.strip(), "the fixture never created an unreachable object"

    findings, _ = scan.scan_history(cwd=str(root))

    assert findings == []


def test_the_audit_form_reports_it(tmp_path):
    root = _repo(tmp_path, {"readme.txt": b"nothing\n"})
    subprocess.run(
        ["git", "hash-object", "-w", "--stdin"],
        cwd=root,
        input=b"key = " + SYNTHETIC_MARKER + b"\n",
        capture_output=True,
        check=True,
    )

    findings, _ = scan.scan_history(cwd=str(root), include_unreachable=True)

    assert findings and "unreachable object, not published" in findings[0]


# --- streaming, and the memory it does not use ---------------------------------


def test_a_large_blob_is_not_read_into_memory_whole():
    """`read(size)` per record made a 32 MiB blob a 32 MiB allocation, which is
    what "bounded streaming" was supposed to have stopped."""
    size = 32 << 20
    body = b"a" * size
    stream = io.BytesIO(_record("abc123", "blob", body))

    tracemalloc.start()
    try:
        before = tracemalloc.get_traced_memory()[1]
        list(scan.iter_batch_records(stream))
        peak = tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()

    # A couple of chunks plus the overlap, generously. What must NOT happen is an
    # allocation on the order of the object itself.
    ceiling = 4 * scan._CHUNK
    assert peak - before < ceiling, f"peaked at {(peak - before) / (1 << 20):.1f} MiB"


def test_a_credential_across_a_chunk_boundary_is_still_found():
    """Chunking without an overlap misses a secret because of where the
    divisions happened to fall, which is the worst possible reason."""
    filler = b"a" * (scan._CHUNK - 8)
    body = filler + b"key = " + SYNTHETIC_MARKER + b"\n" + b"b" * 4096
    stream = io.BytesIO(_record("abc123", "blob", body))

    ((_oid, _kind, kinds),) = scan.iter_batch_records(stream)

    assert "AWS access key" in kinds


def test_a_credential_exactly_on_the_boundary_is_found():
    """Straddling it exactly: ten bytes in one chunk, ten in the next.

    The space matters and is not decoration. The pattern requires a word
    boundary before `AKIA`, so filler running straight into the marker is
    correctly no match at all - on any chunk size, which is why asserting it
    without the separator would have been asserting the wrong thing.
    """
    head = b"a" * (scan._CHUNK - len(SYNTHETIC_MARKER) // 2 - 1) + b" "
    assert len(head) == scan._CHUNK - 10, "the marker must span the boundary"
    body = head + SYNTHETIC_MARKER + b"\n"
    stream = io.BytesIO(_record("abc123", "blob", body))

    ((_oid, _kind, kinds),) = scan.iter_batch_records(stream)

    assert "AWS access key" in kinds


def test_a_tree_is_not_matched_but_is_still_consumed():
    stream = io.BytesIO(_record("aaa", "tree", b"xyz") + _record("bbb", "blob", b"hello"))

    records = list(scan.iter_batch_records(stream))

    assert [(oid, kind) for oid, kind, _ in records] == [("aaa", "tree"), ("bbb", "blob")]


# --- a stream that lies ---------------------------------------------------------


def test_a_truncated_body_raises():
    stream = io.BytesIO(b"abc123 blob 50\nonly a few bytes")

    with pytest.raises(scan.ScanError, match="truncated"):
        list(scan.iter_batch_records(stream))


def test_a_record_without_its_terminator_raises():
    stream = io.BytesIO(b"abc123 blob 5\nhello")

    with pytest.raises(scan.ScanError, match="not terminated"):
        list(scan.iter_batch_records(stream))


def test_an_unparsable_header_raises():
    with pytest.raises(scan.ScanError, match="unparsable"):
        list(scan.iter_batch_records(io.BytesIO(b"garbage\n")))


def test_a_header_that_never_ends_is_bounded():
    """Reading to a newline that may never come turns a malformed stream into an
    allocation."""
    stream = io.BytesIO(b"x" * (scan._MAX_HEADER_BYTES + 100))

    with pytest.raises(scan.ScanError, match="header over"):
        list(scan.iter_batch_records(stream))


def test_an_implausible_size_is_refused():
    stream = io.BytesIO(b"abc123 blob 99999999999999\n")

    with pytest.raises(scan.ScanError, match="outside anything this scanner"):
        list(scan.iter_batch_records(stream))


def test_a_missing_object_raises():
    with pytest.raises(scan.ScanError, match="missing"):
        list(scan.iter_batch_records(io.BytesIO(b"deadbeef missing\n")))


def test_a_failing_enumeration_is_an_error_not_an_empty_history():
    class _Failed:
        returncode = 128
        stdout = b""
        stderr = b"fatal: not a git repository"

    with pytest.raises(scan.ScanError, match="not a git repository"):
        scan.object_names(run=lambda *a, **k: _Failed())


# --- a stream that simply stops -------------------------------------------------


class _FakeProcess:
    def __init__(self, stdout: bytes, stderr: bytes = b"", code: int = 0):
        self.stdout = io.BytesIO(stdout)
        self.stderr = io.BytesIO(stderr)
        self.code = code
        self.killed = False

    def wait(self, timeout=None):
        return self.code

    def kill(self):
        self.killed = True


def _listing(*oids):
    class _Listed:
        returncode = 0
        stdout = b"".join(f"{oid} path{i}.txt\n".encode() for i, oid in enumerate(oids))
        stderr = b""

    return lambda *a, **k: _Listed()


def test_a_stream_that_ends_after_one_of_two_objects_is_an_incomplete_scan():
    """A successful EOF halfway through is indistinguishable from a complete
    scan unless something compares what came back against what was asked for."""
    dump = _record("aaa", "blob", b"harmless")

    with pytest.raises(scan.ScanError, match="did not cover history"):
        scan.scan_history(run=_listing("aaa", "bbb"), popen=lambda *a, **k: _FakeProcess(dump))


def test_a_complete_stream_passes_the_inventory_check():
    dump = _record("aaa", "blob", b"harmless") + _record("bbb", "blob", b"also harmless")

    findings, scanned = scan.scan_history(
        run=_listing("aaa", "bbb"), popen=lambda *a, **k: _FakeProcess(dump)
    )

    assert findings == [] and scanned == 2


def test_a_dump_that_yields_no_blob_against_a_listed_history_is_an_error():
    with pytest.raises(scan.ScanError, match="did not cover history"):
        scan.scan_history(run=_listing("aaa"), popen=lambda *a, **k: _FakeProcess(b""))


def test_a_failing_dump_is_an_error():
    dump = _record("aaa", "blob", b"hi")

    with pytest.raises(scan.ScanError, match="unable to read object"):
        scan.scan_history(
            run=_listing("aaa"),
            popen=lambda *a, **k: _FakeProcess(dump, b"fatal: unable to read object", code=128),
        )


def test_a_broken_stream_kills_the_child_rather_than_leaving_it_writing():
    process = _FakeProcess(b"garbage\n")

    with pytest.raises(scan.ScanError):
        scan.scan_history(run=_listing("aaa"), popen=lambda *a, **k: process)

    assert process.killed, "git was left writing into a pipe nobody reads"


def test_a_flood_of_stderr_does_not_deadlock_the_scan():
    """A REAL child with REAL pipes, because the deadlock is a property of pipes.

    An in-memory stand-in can never reproduce it: `BytesIO` does not block, so
    reading stdout to the end and stderr afterwards looks fine against a double
    and hangs forever against git. This child writes far more to stderr than any
    pipe buffer holds, which is exactly the condition that wedged it.
    """
    dump = _record("aaa", "blob", b"harmless")
    child = (
        "import sys;"
        "sys.stderr.buffer.write(b'warning: something\\n' * 200000);"
        "sys.stderr.buffer.flush();"
        f"sys.stdout.buffer.write({dump!r});"
        "sys.stdout.buffer.flush()"
    )

    def _spawn(*_a, **_k):
        return subprocess.Popen(
            [sys.executable, "-c", child],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    findings, scanned = scan.scan_history(run=_listing("aaa"), popen=_spawn)

    assert findings == [] and scanned == 1


# --- what the exit codes mean ---------------------------------------------------


def test_the_entry_point_reports_a_broken_scan_differently_from_a_finding(monkeypatch, capsys):
    """Exit 2 for "could not check", 1 for "found something": a workflow that
    conflates them learns nothing from either."""
    monkeypatch.setattr(
        scan,
        "scan_history",
        lambda *a, **k: (_ for _ in ()).throw(scan.ScanError("no repo")),
    )

    assert scan.main([]) == 2
    assert "could not be completed" in capsys.readouterr().err


def test_the_entry_point_returns_one_for_a_finding(monkeypatch, capsys):
    monkeypatch.setattr(scan, "scan_history", lambda *a, **k: (["somewhere: a token"], 3))

    assert scan.main([]) == 1
    assert "credential-shaped content" in capsys.readouterr().err


def test_the_entry_point_returns_zero_for_a_clean_history(monkeypatch, capsys):
    monkeypatch.setattr(scan, "scan_history", lambda *a, **k: ([], 3))

    assert scan.main([]) == 0
    assert "no credential-shaped content" in capsys.readouterr().out


def test_the_audit_flag_reaches_the_scan(monkeypatch):
    seen = {}

    def _record_call(include_unreachable=False, **kwargs):
        seen["include"] = include_unreachable
        return [], 0

    monkeypatch.setattr(scan, "scan_history", _record_call)

    scan.main(["--include-unreachable"])

    assert seen["include"] is True


def test_the_patterns_match_what_they_claim_to():
    """A pattern that matches nothing is a scanner that reports clean forever."""
    for name, pattern in scan.PATTERNS.items():
        assert re.compile(pattern), name
    assert scan.credential_kinds(b"key = " + SYNTHETIC_MARKER) == ["AWS access key"]
    assert scan.credential_kinds(b"nothing interesting here") == []
