"""A configuration revision is accepted whole, or rejected whole.

The snapshot exists so every decision comes from ONE reading of the file. That
was the first half. The second half is that the reading has to mean the same
thing the startup path means, and that a revision which is only partly readable
is refused rather than served:

* **Interpolation had the precedence backwards.** `load_dotenv(override=False)`
  resolves `${X}` against the process environment first, because a variable the
  process supplied is the one in force. The snapshot resolved it against the
  file. Same bytes, two different accounts.
* **A malformed line was skipped.** python-dotenv drops what it cannot parse and
  returns the rest, so half a revision arrived looking like a whole one - and a
  reload that "succeeded" then removed the accounts whose lines were dropped.
* **An unreadable file looked exactly like no file.** Both produced an empty
  contribution, so a `.env` that could not be opened silently became "this
  process runs on environment variables alone", and file-owned accounts were
  removed.

These drive real files, because the whole mechanism is about what is on disk.
"""

import os

import pytest

from telegram_mcp import account_snapshot as snap
from telegram_mcp.settings import StartupMessage


@pytest.fixture
def env_file(tmp_path, monkeypatch):
    path = tmp_path / ".env"

    def _write(lines):
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return str(path)

    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    return _write


# --- interpolation ------------------------------------------------------------


def test_interpolation_resolves_against_the_process_first(env_file, monkeypatch):
    """The audit's exact counterexample. Startup answers `process-value`; the
    snapshot answered `file-value`, so the two paths built different accounts
    from identical bytes."""
    monkeypatch.setattr(
        snap, "PROCESS_ACCOUNT_VARS", {"TELEGRAM_SESSION_STRING_ONE": "process-value"}
    )
    path = env_file(
        [
            "TELEGRAM_SESSION_STRING_ONE=file-value",
            "TELEGRAM_SESSION_STRING_TWO=${TELEGRAM_SESSION_STRING_ONE}",
        ]
    )

    shot = snap.read_snapshot(path)

    assert shot.env["TELEGRAM_SESSION_STRING_ONE"] == "process-value"
    assert shot.env["TELEGRAM_SESSION_STRING_TWO"] == "process-value"


def test_interpolation_still_reads_the_file_when_the_process_is_silent(env_file):
    path = env_file(
        [
            "TELEGRAM_SESSION_STRING_ONE=file-value",
            "TELEGRAM_SESSION_STRING_TWO=${TELEGRAM_SESSION_STRING_ONE}",
        ]
    )

    assert snap.read_snapshot(path).env["TELEGRAM_SESSION_STRING_TWO"] == "file-value"


def test_a_default_is_used_when_nothing_supplies_the_name(env_file):
    path = env_file(["TELEGRAM_SESSION_STRING_ONE=${NOT_SET_ANYWHERE:-fallback}"])

    assert snap.read_snapshot(path).env["TELEGRAM_SESSION_STRING_ONE"] == "fallback"


def test_an_ordinary_value_is_untouched(env_file):
    path = env_file(["TELEGRAM_SESSION_STRING_ONE=plain-value"])

    assert snap.read_snapshot(path).env["TELEGRAM_SESSION_STRING_ONE"] == "plain-value"


# --- partial revisions --------------------------------------------------------


def test_a_malformed_line_rejects_the_whole_revision(env_file):
    """Skipping it produced a configuration that looked complete and was not:
    the accounts on the dropped lines were then reported as removed."""
    path = env_file(
        [
            "TELEGRAM_SESSION_STRING_ONE=aaa",
            "this is not a pair",
            "TELEGRAM_SESSION_STRING_TWO=bbb",
        ]
    )

    with pytest.raises(StartupMessage, match="could not be parsed"):
        snap.read_snapshot(path)


def test_a_clean_file_is_accepted(env_file):
    path = env_file(["TELEGRAM_SESSION_STRING_ONE=aaa", "# a comment", "", "OTHER=1"])

    assert snap.read_snapshot(path).env["TELEGRAM_SESSION_STRING_ONE"] == "aaa"


def test_an_unreadable_file_is_not_an_empty_one(tmp_path, monkeypatch):
    """Both used to produce "no file contribution", so a `.env` that could not be
    opened removed every account it owned."""
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    path = tmp_path / ".env"
    path.write_text("TELEGRAM_SESSION_STRING_ONE=aaa\n", encoding="utf-8")

    def _refuse(_path):
        raise OSError("permission denied")

    monkeypatch.setattr(snap, "_open_file_bytes", _refuse)

    with pytest.raises(StartupMessage, match="could not be read"):
        snap.read_snapshot(str(path))


def test_a_file_that_is_simply_absent_is_fine(tmp_path, monkeypatch):
    """A process legitimately running on environment variables alone."""
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})

    shot = snap.read_snapshot(str(tmp_path / "nothing-here"))

    assert shot.stamp == ()


def test_invalid_utf8_rejects_the_revision(tmp_path, monkeypatch):
    """`errors="replace"` turned undecodable bytes into a session string made of
    replacement characters, which is a silently wrong account rather than a
    refused one."""
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    path = tmp_path / ".env"
    path.write_bytes(b"TELEGRAM_SESSION_STRING_ONE=\xff\xfe\n")

    with pytest.raises(StartupMessage, match="not valid UTF-8"):
        snap.read_snapshot(str(path))


# --- what the caller does with a rejection ------------------------------------


def test_a_rejected_revision_leaves_the_running_accounts_alone(tmp_path, monkeypatch):
    """The refresh caller, not just the parser: a revision it cannot read must
    not be applied, and the accounts already serving must not be removed."""
    from telegram_mcp import account_config as cfg
    from telegram_mcp import connection as conn

    path = tmp_path / ".env"
    path.write_text("TELEGRAM_SESSION_STRING_ONE=aaa\n", encoding="utf-8")
    monkeypatch.setattr(cfg, "_env_file", lambda: str(path))
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.setitem(os.environ, "TELEGRAM_API_ID", "1")
    monkeypatch.setitem(os.environ, "TELEGRAM_API_HASH", "h")

    working = object()
    monkeypatch.setattr(conn, "clients", {"one": working})
    conn._env_stamp = ()
    conn._env_digests = {}

    path.write_text(
        "TELEGRAM_SESSION_STRING_ONE=aaa\nbroken line here\n",
        encoding="utf-8",
    )

    assert conn.refresh_accounts() == []
    assert conn.clients == {"one": working}
    assert conn.last_rejection() is not None, "a rejected revision was not recorded"
