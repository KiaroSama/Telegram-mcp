"""A leg's record has to be evidence, not a set of numbers that agree.

The comparator already refuses missing legs, duplicates, empty records, mixed
commits and interpreter mismatches. Four synthetic counterexamples still passed
it, and each one is a run that proves nothing while reporting green:

* **A Windows-labelled leg claiming Linux.** The manifest named the label and
  the interpreter and said nothing about the platform, so the two OS legs could
  both be the same OS and nothing noticed.
* **More skipped than collected.** Internally impossible, so the record is not
  describing a real run - and it was compared as though it were.
* **Every record saying `commit=local, lock=absent`.** Those are the values the
  emitter writes when it is NOT running in CI. Six of them agree perfectly and
  describe six local runs.
* **Every record naming a commit that is not the one being verified.** They
  agree with each other and not with the tree the gate is about.

These are validator gaps, not evidence that the observed CI run used such
records.
"""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

import ci_record  # noqa: E402

LEGS = Path(__file__).resolve().parents[1] / ".github" / "expected-legs.txt"
REPO_LOCK = ci_record.lock_digest(Path(__file__).resolve().parents[1] / "uv.lock")


def _leg(directory, label, **over):
    """One believable record, with the fields a test wants overridden."""
    record = {
        "label": label,
        "python": ci_record.expected_legs(LEGS)[label] + ".9",
        "platform": "Windows" if "windows" in label else "Linux",
        "commit": "abc123def456",
        "lock": REPO_LOCK,
        "collected": 3000,
        "failures": 0,
        "errors": 0,
        "skipped": 40,
    }
    record.update(over)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{label}.json").write_text(json.dumps(record), encoding="utf-8")
    return record


def _full_set(directory, **over):
    for label in ci_record.expected_legs(LEGS):
        _leg(directory, label, **over)


def test_a_complete_believable_set_passes(tmp_path, capsys):
    """The control: everything below must fail for its own reason, not because
    the comparator rejects everything."""
    _full_set(tmp_path)

    assert ci_record.main(["--compare", str(tmp_path), "--legs", str(LEGS)]) == 0


def test_a_leg_that_ran_the_wrong_platform_fails(tmp_path, capsys):
    """The windows leg claiming Linux. Both OS legs could be the same OS and the
    comparator had nothing to check it against."""
    _full_set(tmp_path)
    _leg(tmp_path, "lottie-windows-latest", platform="Linux")

    assert ci_record.main(["--compare", str(tmp_path), "--legs", str(LEGS)]) == 1
    assert "platform" in capsys.readouterr().out.lower()


def test_more_skipped_than_collected_fails(tmp_path, capsys):
    """Internally impossible, so the record is not describing a real run."""
    _full_set(tmp_path)
    _leg(tmp_path, "py3.13", collected=10, skipped=99)

    assert ci_record.main(["--compare", str(tmp_path), "--legs", str(LEGS)]) == 1
    assert "skipped" in capsys.readouterr().out.lower()


def test_records_that_all_say_local_fail(tmp_path, capsys):
    """`commit=local, lock=absent` is what the emitter writes when it is NOT in
    CI. Six of them agree with each other and describe six local runs."""
    _full_set(tmp_path, commit="local", lock="absent")

    assert ci_record.main(["--compare", str(tmp_path), "--legs", str(LEGS)]) == 1
    said = capsys.readouterr().out.lower()
    assert "local" in said


def test_records_naming_another_commit_fail(tmp_path, capsys):
    """They agree with each other and not with the tree the gate is about."""
    _full_set(tmp_path, commit="0" * 40)

    code = ci_record.main(
        ["--compare", str(tmp_path), "--legs", str(LEGS), "--expect-commit", "abc123def456"]
    )

    assert code == 1
    assert "commit" in capsys.readouterr().out.lower()


def test_the_expected_commit_is_accepted_when_it_matches(tmp_path):
    _full_set(tmp_path, commit="abc123def456")

    assert (
        ci_record.main(
            ["--compare", str(tmp_path), "--legs", str(LEGS), "--expect-commit", "abc123def456"]
        )
        == 0
    )


def test_a_lock_that_is_not_the_repositorys_fails(tmp_path, capsys):
    """Agreement among records says nothing about whether they resolved from the
    lockfile this tree actually has."""
    _full_set(tmp_path, lock="0123456789ab")

    assert ci_record.main(["--compare", str(tmp_path), "--legs", str(LEGS), "--expect-lock"]) == 1
    assert "lock" in capsys.readouterr().out.lower()


def test_the_repositorys_own_lock_is_accepted(tmp_path):
    _full_set(tmp_path)

    assert ci_record.main(["--compare", str(tmp_path), "--legs", str(LEGS), "--expect-lock"]) == 0


def test_the_manifest_names_a_platform_for_every_leg():
    """The manifest is what makes the platform checkable at all."""
    legs = ci_record.expected_legs(LEGS)

    assert set(legs) == {
        "py3.11",
        "py3.12",
        "py3.13",
        "py3.14",
        "lottie-ubuntu-latest",
        "lottie-windows-latest",
    }
    for label, expected in legs.items():
        assert expected.platform, f"{label} declares no platform"
    assert legs["lottie-windows-latest"].platform == "Windows"
    assert legs["py3.11"].platform == "Linux"
