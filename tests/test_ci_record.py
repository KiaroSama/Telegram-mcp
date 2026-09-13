"""A CI leg must prove its label, not assert it.

Run 34667633840 job 103482685575 was named "Python 3.11", installed 3.11.16,
and then let `uv run` resolve the project's own pinned 3.13 - reporting 2614
passed for an interpreter the leg never touched. Two of the four advertised
versions were therefore untested while the matrix showed green across the board.

These tests cover the two ways that goes quiet: the wrong interpreter, and a
leg that collected no case at all.
"""

import json
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


def _record(directory, name, collected, skipped=0, label=None, python="3.13"):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{name}.json").write_text(
        json.dumps(
            {
                "label": label or name,
                "python": python,
                "collected": collected,
                "skipped": skipped,
            }
        ),
        encoding="utf-8",
    )


def test_legs_that_agree_on_the_case_count_pass(tmp_path, capsys):
    _record(tmp_path, "py3.11", 2834, skipped=67)
    _record(tmp_path, "py3.13", 2834, skipped=67)
    _record(tmp_path, "lottie-windows", 2834, skipped=23)

    assert ci_record.main(["--compare", str(tmp_path)]) == 0
    assert "every leg collected 2834" in capsys.readouterr().out


def test_one_leg_short_by_a_single_file_fails(tmp_path, capsys):
    """The defect this exists for. Nothing is red; the total is simply short, and
    `2831 passed` reads exactly like `2834 passed` unless something compares them."""
    _record(tmp_path, "py3.11", 2834)
    _record(tmp_path, "py3.13", 2831)
    _record(tmp_path, "lottie-windows", 2834)

    assert ci_record.main(["--compare", str(tmp_path)]) == 1
    said = capsys.readouterr().out
    assert "disagree on how many cases exist" in said
    assert "[2831, 2834]" in said


def test_skips_may_differ_between_legs(tmp_path):
    """The Windows lottie leg has the renderer, so it skips fewer. That is the
    suite working, not a lost file, and it must not fail the gate."""
    _record(tmp_path, "py3.11", 2834, skipped=67)
    _record(tmp_path, "lottie-windows", 2834, skipped=23)

    assert ci_record.main(["--compare", str(tmp_path)]) == 0


def test_a_single_record_is_not_a_comparison(tmp_path, capsys):
    _record(tmp_path, "py3.11", 2834)

    assert ci_record.main(["--compare", str(tmp_path)]) == 1
    assert "nothing to compare" in capsys.readouterr().out


def test_an_unreadable_record_fails_rather_than_being_skipped(tmp_path, capsys):
    _record(tmp_path, "py3.11", 2834)
    (tmp_path / "broken.json").write_text("not json", encoding="utf-8")

    assert ci_record.main(["--compare", str(tmp_path)]) == 1
    assert "could not read" in capsys.readouterr().out


def test_a_leg_writes_the_record_the_comparison_reads(tmp_path, monkeypatch):
    """The two halves must fit: what `--emit` writes is what `--compare` parses."""
    report = _junit(tmp_path / "junit.xml", tests=2834, skipped=67)
    monkeypatch.setenv("RUNNER_OS", "Linux")
    monkeypatch.setenv("GITHUB_SHA", "abc123def456")

    assert (
        ci_record.main(
            ["--label", "py3.13", "--junit", str(report), "--emit", str(tmp_path / "r/one.json")]
        )
        == 0
    )

    written = json.loads((tmp_path / "r/one.json").read_text(encoding="utf-8"))
    assert written["collected"] == 2834 and written["skipped"] == 67
    assert written["label"] == "py3.13" and written["platform"] == "Linux"

    _record(tmp_path / "r", "two", 2834)
    assert ci_record.main(["--compare", str(tmp_path / "r")]) == 0


def test_nothing_is_emitted_for_a_report_that_could_not_be_read(tmp_path):
    """A record written from a failed read would enter the comparison as a fact."""
    broken = tmp_path / "junit.xml"
    broken.write_text("not xml", encoding="utf-8")

    assert ci_record.main(["--junit", str(broken), "--emit", str(tmp_path / "r/one.json")]) == 1
    assert not (tmp_path / "r/one.json").exists()


def test_the_declared_suite_count_matches_what_git_tracks(tmp_path, capsys):
    """The half cross-leg agreement cannot reach: a suite deleted on EVERY leg.
    All of them then collect the same smaller number and agree perfectly."""
    declared = tmp_path / "expected.txt"
    declared.write_text("# a comment\n145\n", encoding="utf-8")

    lines, failed = ci_record.check_suite_count(declared)

    real = len(ci_record.tracked_suites())
    assert failed is (real != 145)
    assert str(real) in " ".join(lines)


def test_an_undeclared_removal_fails_and_says_what_to_do(tmp_path):
    declared = tmp_path / "expected.txt"
    declared.write_text(str(len(ci_record.tracked_suites()) + 3), encoding="utf-8")

    lines, failed = ci_record.check_suite_count(declared)

    assert failed
    said = " ".join(lines)
    assert "3 were removed" in said and "SAME commit" in said


def test_an_undeclared_addition_also_fails(tmp_path):
    """Both directions: a file added without declaring it means the number stops
    describing the repository, and then it stops catching a removal too."""
    declared = tmp_path / "expected.txt"
    declared.write_text(str(len(ci_record.tracked_suites()) - 2), encoding="utf-8")

    lines, failed = ci_record.check_suite_count(declared)

    assert failed and "2 were added" in " ".join(lines)


def test_a_declaration_with_no_number_is_an_error(tmp_path):
    declared = tmp_path / "expected.txt"
    declared.write_text("# only comments\n", encoding="utf-8")

    lines, failed = ci_record.check_suite_count(declared)

    assert failed and "could not check" in " ".join(lines)


def test_the_repository_declaration_is_current():
    """The committed file is only useful while it is true."""
    declared = Path(__file__).resolve().parents[1] / ".github" / "expected-suites.txt"

    _lines, failed = ci_record.check_suite_count(declared)

    assert not failed, "`.github/expected-suites.txt` no longer matches the tracked suites"


def test_a_tracked_suite_that_collected_nothing_is_named(tmp_path):
    """Still on disk, still tracked, collecting nothing - a bad import or a
    renamed class. Invisible to every count, because it was never counted."""
    report = tmp_path / "junit.xml"
    report.write_text(
        '<testsuites><testsuite name="pytest" tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tests.test_alpha" name="one"/></testsuite></testsuites>',
        encoding="utf-8",
    )

    quiet = ci_record.silent_suites(report, ["tests.test_alpha", "tests.test_beta"])

    assert quiet == ["tests.test_beta"]


def test_every_suite_speaking_leaves_nothing_quiet(tmp_path):
    report = tmp_path / "junit.xml"
    report.write_text(
        '<testsuites><testsuite name="pytest" tests="2" failures="0" errors="0" skipped="0">'
        '<testcase classname="tests.test_alpha" name="one"/>'
        '<testcase classname="tests.test_beta" name="two"/></testsuite></testsuites>',
        encoding="utf-8",
    )

    assert ci_record.silent_suites(report, ["tests.test_alpha", "tests.test_beta"]) == []


def test_the_tracked_list_comes_from_git_and_is_never_empty():
    tracked = ci_record.tracked_suites()

    assert "tests.test_ci_record" in tracked
    assert all(name.startswith("tests.test_") for name in tracked)


def test_a_partial_report_is_not_accused_of_silent_suites(tmp_path, capsys):
    """`pytest tests/test_one.py` legitimately mentions one file. Calling that
    144 silent suites is noise, and noise is how a real one gets scrolled past."""
    report = _junit(tmp_path / "junit.xml", tests=17)

    assert ci_record.main(["--junit", str(report)]) == 0
    assert "contributed no case" not in capsys.readouterr().out


def test_a_whole_suite_report_missing_a_file_is_accused(tmp_path, capsys):
    """With --all-suites the same report IS a claim about every tracked file."""
    report = _junit(tmp_path / "junit.xml", tests=17)

    assert ci_record.main(["--junit", str(report), "--all-suites"]) == 1
    assert "contributed no case" in capsys.readouterr().out


def test_a_suite_whose_tests_live_in_classes_counts_as_speaking(tmp_path):
    """pytest reports `module.Class` for a method. Matching the whole string
    called `tests/test_sanitize.py` silent on the first CI run, with 34 passing
    tests in it - a false positive is how a real finding gets scrolled past."""
    report = tmp_path / "junit.xml"
    report.write_text(
        '<testsuites><testsuite name="pytest" tests="2" failures="0" errors="0" skipped="0">'
        '<testcase classname="tests.test_sanitize.TestSanitizeName" name="one"/>'
        '<testcase classname="tests.test_plain" name="two"/></testsuite></testsuites>',
        encoding="utf-8",
    )

    assert ci_record.silent_suites(report, ["tests.test_sanitize", "tests.test_plain"]) == []


def test_a_prefix_is_not_a_match(tmp_path):
    """`tests.test_alpha_extra` speaking says nothing about `tests.test_alpha`."""
    report = tmp_path / "junit.xml"
    report.write_text(
        '<testsuites><testsuite name="pytest" tests="1" failures="0" errors="0" skipped="0">'
        '<testcase classname="tests.test_alpha_extra" name="one"/></testsuite></testsuites>',
        encoding="utf-8",
    )

    assert ci_record.silent_suites(report, ["tests.test_alpha"]) == ["tests.test_alpha"]
