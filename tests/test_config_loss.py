"""A configuration file that VANISHES is a revision this process cannot read.

`account_snapshot` already separates "absent" from "unreadable": a `.env` that
exists and cannot be opened raises, and `connection.refresh_accounts` catches
that and keeps the running clients. What it could not separate is a file that was
there a moment ago and is not there now, because `_read_file_bytes` returned
`None` for both "no file was ever configured" and "the file is gone".

Those are opposite facts. The first is an operator running on real environment
variables, which is supported. The second parses as an empty file - and with a
second account supplied through the process environment, an empty file is a valid
SMALLER configuration, so the file's accounts land in the removal set and are
retired. A momentary disappearance is not rare: an editor that writes by unlink
then create, an atomic rewrite between unlink and rename, a network drive
blinking.

The rule this pins: intentional removal comes from a revision that parsed, never
from a read that failed.
"""

import os

import pytest

from telegram_mcp import account_snapshot
from telegram_mcp.settings import StartupMessage

ONE_ACCOUNT = "TELEGRAM_SESSION_STRING_WORK=aaaa\n"


@pytest.fixture(autouse=True)
def _forget_between_tests():
    """The module remembers the file it last read; each test starts fresh."""
    account_snapshot.forget_accepted_source()
    yield
    account_snapshot.forget_accepted_source()


def _env(tmp_path, body=ONE_ACCOUNT):
    path = tmp_path / ".env"
    path.write_text(body, encoding="utf-8")
    return str(path)


# --- the defect -----------------------------------------------------------


def test_a_file_that_vanishes_after_being_read_is_refused(tmp_path):
    """Not treated as "there is no file". The accounts it owned are still real."""
    path = _env(tmp_path)
    first = account_snapshot.read_snapshot(path)
    assert first.env["TELEGRAM_SESSION_STRING_WORK"] == "aaaa"

    os.remove(path)

    with pytest.raises(StartupMessage) as refused:
        account_snapshot.read_snapshot(path)

    said = str(refused.value)
    assert "aaaa" not in said, "the refusal quoted a session value"
    assert "restart" in said.lower(), "the refusal does not say how to proceed"


def test_the_refusal_names_no_accounts_so_nothing_is_retired(tmp_path):
    """The caller's rejection path keeps the running clients only if it never
    sees a smaller account set. A raise is what guarantees that."""
    path = _env(tmp_path)
    account_snapshot.read_snapshot(path)
    os.remove(path)

    with pytest.raises(StartupMessage):
        account_snapshot.read_snapshot(path)


# --- what must keep working -----------------------------------------------


def test_starting_with_no_file_at_all_is_environment_only(tmp_path):
    """Supported, and the reason the two cases cannot simply be merged: nothing
    was accepted yet, so there is nothing to have lost."""
    snapshot = account_snapshot.read_snapshot(str(tmp_path / "never-existed"))

    assert snapshot.stamp == ()
    assert "TELEGRAM_SESSION_STRING_WORK" not in snapshot.env


def test_a_file_that_comes_back_is_read_again(tmp_path):
    """A rewrite that briefly removes the file recovers on the next read, with
    no restart. The refusal is a pause, not a latch."""
    path = _env(tmp_path)
    account_snapshot.read_snapshot(path)
    os.remove(path)
    with pytest.raises(StartupMessage):
        account_snapshot.read_snapshot(path)

    _env(tmp_path, "TELEGRAM_SESSION_STRING_WORK=bbbb\n")
    recovered = account_snapshot.read_snapshot(path)

    assert recovered.env["TELEGRAM_SESSION_STRING_WORK"] == "bbbb"


def test_a_different_path_is_not_the_one_that_was_accepted(tmp_path):
    """Only the file this process actually read is the one whose absence is a
    loss. A path it never accepted is just absent."""
    accepted = _env(tmp_path)
    account_snapshot.read_snapshot(accepted)

    snapshot = account_snapshot.read_snapshot(str(tmp_path / "other.env"))

    assert snapshot.stamp == ()


def test_an_unreadable_file_still_refuses(tmp_path, monkeypatch):
    """The half that already worked, pinned so this change cannot remove it."""
    path = _env(tmp_path)

    def refuse(_p):
        raise OSError("denied")

    monkeypatch.setattr(account_snapshot, "_open_file_bytes", refuse)

    with pytest.raises(StartupMessage):
        account_snapshot.read_snapshot(path)
