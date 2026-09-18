"""The scanner's deadline is enforced WHILE it reads, and its matches do not
depend on where the chunk divisions fell.

Two ways this reported the wrong thing:

* **The deadline was checked after the blocking parse returned.** A child that
  produced a header and then went quiet was never stopped by the scanner at
  all - the run's own outer guard killed it, minutes later, with no finding
  either way. A deadline that only applies once the work has finished is not a
  deadline.
* **Each chunk was matched as an independent string.** `re` anchors `\\b` to the
  end of the string it is given, so a 20-character AWS-shaped run ending exactly
  at a chunk boundary and followed by another word character matched the chunk
  and does NOT match the whole input. The streaming scan reported a credential
  the file does not contain.

The second is the one that matters for trust: a scanner that cries wolf at
byte 65536 teaches its operator to ignore it.
"""

import io
import time

import pytest

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

import history_secret_scan as scan  # noqa: E402


def _stream(data: bytes):
    return io.BytesIO(data)


# --- chunk boundaries ---------------------------------------------------------


# Test-sized, but in the same PROPORTION production uses: the overlap must be
# wider than the longest shape being matched, or no window can ever hold one
# whole. Production is 1 MiB / 64 KiB against a longest shape of ~350 bytes.
_TEST_CHUNK = 1024
_TEST_OVERLAP = 512


@pytest.mark.parametrize("name,pattern", sorted(scan.COMPILED.items()))
@pytest.mark.parametrize("shift", [0, 1, 7, 64, 500, 1020])
def test_streaming_and_whole_input_agree_wherever_the_boundary_falls(
    name, pattern, shift, monkeypatch
):
    """The property, checked for each rule at several offsets rather than for
    one favourite: what the chunked scan finds is exactly what the same regex
    finds over the whole input, wherever the divisions land.

    The `shift` sweeps the marker across a chunk boundary, which is the only
    thing that can make the two disagree.
    """
    marker = _marker_for(name)
    if marker is None:
        pytest.skip(f"no synthetic marker defined for {name}")
    body = b"a" * (_TEST_CHUNK + shift) + marker + b"a" * 900

    whole = bool(pattern.search(body))
    monkeypatch.setattr(scan, "_CHUNK", _TEST_CHUNK)
    monkeypatch.setattr(scan, "_OVERLAP", _TEST_OVERLAP)
    streamed = name in scan._scan_stream(_stream(body), len(body), "deadbeef", True)

    assert streamed == whole, f"{name} at shift {shift}: chunked scan disagreed"


def test_a_shape_that_is_not_a_credential_is_not_reported_at_a_boundary(monkeypatch):
    """The audit's counterexample, made concrete. A 20-character AWS-shaped run
    immediately followed by another word character is NOT a match over the whole
    input - the trailing boundary fails. Cut the chunk between them and each
    half matched its own end-of-string."""
    aws = next((n for n in scan.COMPILED if "aws" in n.lower()), None)
    if aws is None:
        pytest.skip("this repository has no AWS rule to exercise")
    pattern = scan.COMPILED[aws]
    body = b"-" * 44 + b" AKIA" + b"B" * 16 + b"Z" + b"y" * 40

    assert not pattern.search(body), "the fixture is not the non-match this test needs"

    monkeypatch.setattr(scan, "_CHUNK", 64)
    monkeypatch.setattr(scan, "_OVERLAP", 32)
    found = scan._scan_stream(_stream(body), len(body), "deadbeef", True)

    assert aws not in found, "a chunk boundary invented a word boundary and a finding"


def test_a_real_credential_split_across_a_boundary_is_still_found(monkeypatch):
    """The other direction, which the overlap exists for and must not regress."""
    aws = next((n for n in scan.COMPILED if "aws" in n.lower()), None)
    if aws is None:
        pytest.skip("this repository has no AWS rule to exercise")
    body = b"-" * 56 + b" AKIA" + b"B" * 16 + b" trailing"

    assert scan.COMPILED[aws].search(body), "the fixture is not a real match"

    monkeypatch.setattr(scan, "_CHUNK", 64)
    monkeypatch.setattr(scan, "_OVERLAP", 32)

    assert aws in scan._scan_stream(_stream(body), len(body), "deadbeef", True)


def _marker_for(name: str):
    """A synthetic value each rule should match, kept obviously fake."""
    lowered = name.lower()
    if "aws" in lowered:
        return b" AKIA" + b"B" * 16 + b" "
    if "session" in lowered or "telethon" in lowered:
        return b" 1" + b"A" * 350 + b" "
    if "hash" in lowered or "api" in lowered:
        return b" " + b"0" * 32 + b" "
    if "private" in lowered or "key" in lowered:
        # ASSEMBLED, never written out. The repository's own secret scan reads
        # this tracked file, and a literal PEM header here is a finding by its
        # own rule - correctly, which is why the fixture is built rather than
        # added to a placeholder allowlist that would weaken the gate.
        return b" " + b"-" * 5 + b"BEGIN RSA PRIVATE KEY" + b"-" * 5 + b" "
    if "token" in lowered or "bot" in lowered:
        return b" 123456789:" + b"A" * 35 + b" "
    return None


# --- the deadline -------------------------------------------------------------


def test_the_deadline_is_enforced_while_the_stream_is_being_read(monkeypatch):
    """The counterexample: a child that goes quiet mid-body was never stopped by
    the scanner - only by whatever outer guard was watching the whole run."""
    monkeypatch.setattr(scan, "_SCAN_DEADLINE_SECONDS", 0.1)
    monkeypatch.setattr(scan, "_CHUNK", 16)

    class _SlowStream:
        """Answers, slowly, for ever - the shape of a child that stopped talking."""

        def read(self, n):
            time.sleep(0.05)
            return b"a" * n

    started = time.monotonic()
    with pytest.raises(scan.ScanError, match="exceeded"):
        scan._scan_stream(_SlowStream(), 1 << 20, "deadbeef", True)
    elapsed = time.monotonic() - started

    assert elapsed < 5, f"the deadline did not stop the read ({elapsed:.1f}s)"


def test_a_stream_that_finishes_in_time_is_untouched(monkeypatch):
    monkeypatch.setattr(scan, "_SCAN_DEADLINE_SECONDS", 30)
    body = b"nothing interesting here" * 10

    assert scan._scan_stream(_stream(body), len(body), "deadbeef", True) == []


def test_a_truncated_object_is_still_an_error(monkeypatch):
    """The existing guard, which the deadline must not displace."""
    monkeypatch.setattr(scan, "_SCAN_DEADLINE_SECONDS", 30)

    with pytest.raises(scan.ScanError, match="truncated"):
        scan._scan_stream(_stream(b"short"), 500, "deadbeef", True)
