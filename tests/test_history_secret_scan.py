"""The history scan must fail CLOSED.

It lived inline in the workflow with neither git call's return code checked, so
a run that could not read the repository at all printed "no credential-shaped
content in any reachable blob" and exited 0. Every test here is about the
difference between "found nothing" and "could not look".

The clean-repository and synthetic-marker cases build a real git repository in a
temporary directory - the parser is tested against git's actual `--batch`
output, not against a fixture of what it is believed to look like.
"""

import io
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

import history_secret_scan as scan  # noqa: E402

pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="git is not installed")

# Shaped like the real thing and matched by the same pattern, but not a
# credential: 16 upper-case characters after a marker this project never issues.
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


def test_a_credential_deleted_from_the_tip_is_still_found(tmp_path):
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

    joined = " ".join(findings).encode()
    assert SYNTHETIC_MARKER not in joined, "the scanner republished the value it objected to"


def test_a_directory_that_is_not_a_repository_fails_rather_than_reporting_clean(tmp_path):
    """The exact defect. This used to exit 0 having scanned nothing."""
    with pytest.raises(scan.ScanError) as raised:
        scan.scan_history(cwd=str(tmp_path))

    assert "rev-list" in str(raised.value)


def test_a_failing_enumeration_is_an_error_not_an_empty_history():
    class _Failed:
        returncode = 128
        stdout = b""
        stderr = b"fatal: not a git repository"

    with pytest.raises(scan.ScanError, match="not a git repository"):
        scan.object_names(run=lambda *a, **k: _Failed())


def test_a_truncated_batch_record_raises():
    """The body is shorter than its declared size: the rest of history never
    arrived, and stopping there quietly is what turned a partial scan green."""
    stream = io.BytesIO(b"abc123 blob 50\nonly a few bytes")

    with pytest.raises(scan.ScanError, match="truncated"):
        list(scan.iter_batch_records(stream))


def test_a_record_without_its_terminator_raises():
    stream = io.BytesIO(b"abc123 blob 5\nhello")

    with pytest.raises(scan.ScanError, match="not terminated"):
        list(scan.iter_batch_records(stream))


def test_an_unparsable_header_raises():
    stream = io.BytesIO(b"garbage\n")

    with pytest.raises(scan.ScanError, match="unparsable"):
        list(scan.iter_batch_records(stream))


def test_a_missing_object_raises():
    stream = io.BytesIO(b"deadbeef missing\n")

    with pytest.raises(scan.ScanError, match="missing"):
        list(scan.iter_batch_records(stream))


def test_whole_records_are_parsed_back_exactly():
    stream = io.BytesIO(b"aaa blob 5\nhello\nbbb tree 3\nxyz\n")

    assert list(scan.iter_batch_records(stream)) == [
        ("aaa", "blob", b"hello"),
        ("bbb", "tree", b"xyz"),
    ]


def test_a_dump_that_yields_no_blob_against_a_listed_history_is_an_error(monkeypatch):
    """The two halves disagreeing is a broken scan, not a clean one."""

    class _Listed:
        returncode = 0
        stdout = b"abc123 config.txt\n"
        stderr = b""

    class _EmptyDump:
        stdout = io.BytesIO(b"")
        stderr = io.BytesIO(b"")

        def wait(self):
            return 0

        def kill(self):
            pass

    with pytest.raises(scan.ScanError, match="did not cover history"):
        scan.scan_history(run=lambda *a, **k: _Listed(), popen=lambda *a, **k: _EmptyDump())


def test_a_failing_dump_is_an_error(monkeypatch):
    class _Listed:
        returncode = 0
        stdout = b"abc123 config.txt\n"
        stderr = b""

    class _BrokenDump:
        stdout = io.BytesIO(b"abc123 blob 2\nhi\n")
        stderr = io.BytesIO(b"fatal: unable to read object")

        def wait(self):
            return 128

        def kill(self):
            pass

    with pytest.raises(scan.ScanError, match="unable to read object"):
        scan.scan_history(run=lambda *a, **k: _Listed(), popen=lambda *a, **k: _BrokenDump())


def test_a_credential_across_a_window_boundary_is_still_found():
    """The blob is chunked to bound memory; the overlap is what keeps a value
    lying across a boundary from falling between two windows."""
    filler = b"a" * (scan._CHUNK - 8)
    body = filler + b"key = " + SYNTHETIC_MARKER + b"\n" + b"b" * 4096

    assert "AWS access key" in scan.credential_kinds(body)


def test_the_entry_point_reports_a_broken_scan_differently_from_a_finding(monkeypatch, capsys):
    """Exit 2 for "could not check", 1 for "found something": a workflow that
    conflates them learns nothing from either."""
    monkeypatch.setattr(
        scan, "scan_history", lambda *a, **k: (_ for _ in ()).throw(scan.ScanError("no repo"))
    )

    assert scan.main([]) == 2
    assert "could not be completed" in capsys.readouterr().err
