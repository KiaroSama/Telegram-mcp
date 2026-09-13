"""A CI leg must prove its label, not assert it.

Run 34667633840 job 103482685575 was named "Python 3.11", installed 3.11.16,
and then let `uv run` resolve the project's own pinned 3.13 - reporting 2614
passed for an interpreter the leg never touched. Two of the four advertised
versions were therefore untested while the matrix showed green across the board.

These tests cover the two ways that goes quiet: the wrong interpreter, and a
leg that collected no case at all.
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

import ci_record  # noqa: E402


def _junit(path: Path, tests, failures=0, errors=0, skipped=0) -> Path:
    path.write_text(
        f'<?xml version="1.0"?><testsuites><testsuite name="pytest" tests="{tests}" '
        f'failures="{failures}" errors="{errors}" skipped="{skipped}"/></testsuites>',
        encoding="utf-8",
    )
    return path


def test_the_leg_it_is_actually_running_passes(capsys):
    running = ".".join(str(part) for part in sys.version_info[:2])

    assert ci_record.main(["--label", "self", "--require", running]) == 0
    assert running in capsys.readouterr().out


def test_a_leg_running_another_interpreter_fails(capsys):
    """The defect itself: labelled one version, running another."""
    assert ci_record.main(["--label", "py2.7", "--require", "2.7"]) == 1
    assert "MISMATCH" in capsys.readouterr().out


def test_counts_and_skips_are_reported(tmp_path, capsys):
    report = _junit(tmp_path / "junit.xml", tests=2614, skipped=67)

    assert ci_record.main(["--junit", str(report)]) == 0
    assert "2614 run" in capsys.readouterr().out


def test_a_leg_that_collected_nothing_fails(tmp_path, capsys):
    """Zero cases is not a pass; it is the suite failing to be found."""
    report = _junit(tmp_path / "junit.xml", tests=0)

    assert ci_record.main(["--junit", str(report)]) == 1
    assert "proves nothing" in capsys.readouterr().out


def test_an_unreadable_report_fails_rather_than_counting_zero(tmp_path, capsys):
    broken = tmp_path / "junit.xml"
    broken.write_text("not xml at all", encoding="utf-8")

    assert ci_record.main(["--junit", str(broken)]) == 1
    assert "could not read" in capsys.readouterr().out


def test_a_missing_report_fails(tmp_path, capsys):
    assert ci_record.main(["--junit", str(tmp_path / "absent.xml")]) == 1
    assert "could not read" in capsys.readouterr().out


def test_a_bare_testsuite_root_is_counted(tmp_path):
    """pytest emits <testsuites> today and <testsuite> in older versions; a
    record that silently counted zero for one of them is the failure mode here."""
    report = tmp_path / "junit.xml"
    report.write_text(
        '<testsuite name="pytest" tests="7" failures="1" errors="0" skipped="2"/>',
        encoding="utf-8",
    )

    assert ci_record.case_counts(report) == (7, 1, 0, 2)


def test_a_report_with_no_suite_raises(tmp_path):
    report = tmp_path / "junit.xml"
    report.write_text("<testsuites/>", encoding="utf-8")

    with pytest.raises(ValueError, match="no testsuite"):
        ci_record.case_counts(report)


def test_the_lock_digest_distinguishes_dependency_sets(tmp_path):
    one = tmp_path / "a.lock"
    one.write_text("version = 1\n", encoding="utf-8")
    two = tmp_path / "b.lock"
    two.write_text("version = 2\n", encoding="utf-8")

    assert ci_record.lock_digest(one) != ci_record.lock_digest(two)
    assert ci_record.lock_digest(tmp_path / "absent.lock") == "absent"


def test_the_record_reaches_the_step_summary(tmp_path, monkeypatch):
    summary = tmp_path / "summary.md"
    monkeypatch.setenv("GITHUB_STEP_SUMMARY", str(summary))

    ci_record.main(["--label", "written-down"])

    assert "written-down" in summary.read_text(encoding="utf-8")
