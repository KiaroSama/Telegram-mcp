"""One reading of the configuration, and one answer about where a value came from.

Two defects with the same root: the configuration was read more than once, and
the readings were allowed to disagree.

* **Provenance was decided after the merge.** `load_dotenv()` does not override,
  so a key set in BOTH the process and the file is served from the process. The
  old test for "external" was "absent from the file", which gets exactly that
  case backwards: the reload view reported the FILE's value while every call was
  served from the process's, and deleting the line from the file dropped the
  genuine external account as well.
* **Three reads at startup.** Clients were built from one reading, the
  fingerprint taken from a second and the active digests from a third. An edit
  landing between them was recorded as active with no client ever built for it -
  and the next reload, comparing the new file against itself, found nothing to do.

Nothing here opens a socket. The subject is a file, an environment and what is
derived from them.
"""

import os

import pytest

from telegram_mcp import account_lifecycle as lifecycle
from telegram_mcp import account_snapshot as snap
from telegram_mcp.settings import StartupMessage

KEY = "TELEGRAM_SESSION_STRING_X"


@pytest.fixture
def env_file(tmp_path, monkeypatch):
    """A `.env` this test owns, with the process-supplied set under its control."""
    path = tmp_path / ".env"
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    for name in list(os.environ):
        if name.startswith(snap.ACCOUNT_PREFIXES):
            monkeypatch.delenv(name, raising=False)

    def write(text):
        path.write_text(text, encoding="utf-8")
        return str(path)

    write("")
    return write


# --- where a value came from --------------------------------------------------


def test_the_process_wins_over_the_file_for_the_same_key(env_file, monkeypatch):
    """The defect. `load_dotenv()` does not override, so the RUNNING client was
    built from `external-B` while the reload view reported `file-A` - one
    process, two answers about the same account."""
    path = env_file(f"{KEY}=file-A\n")
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {KEY: "external-B"})

    assert snap.read_snapshot(path).env[KEY] == "external-B"


def test_a_file_only_key_is_used(env_file):
    path = env_file(f"{KEY}=file-A\n")

    assert snap.read_snapshot(path).env[KEY] == "file-A"


def test_a_process_only_key_is_used(env_file, monkeypatch):
    path = env_file("")
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {KEY: "external-B"})

    assert snap.read_snapshot(path).env[KEY] == "external-B"


def test_deleting_the_line_keeps_a_genuinely_external_account(env_file, monkeypatch):
    """The second half of the same defect: because the overlapping key was
    called file-managed, removing it from the file removed the account the
    PROCESS had supplied, which no edit to that file should be able to do."""
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {KEY: "external-B"})
    env_file(f"{KEY}=file-A\n")
    path = env_file("")

    assert snap.read_snapshot(path).env.get(KEY) == "external-B"


def test_deleting_a_file_managed_key_removes_the_account(env_file):
    """The behaviour that must survive the fix: an account the file owns has to
    be deletable, or `.env` stops being how accounts are managed."""
    env_file(f"{KEY}=file-A\n")
    path = env_file("")

    assert KEY not in snap.read_snapshot(path).env


def test_unrelated_process_variables_are_untouched(env_file, monkeypatch):
    monkeypatch.setenv("TELEGRAM_API_ID", "12345")
    path = env_file(f"{KEY}=file-A\n")

    assert snap.read_snapshot(path).env["TELEGRAM_API_ID"] == "12345"


# --- one reading --------------------------------------------------------------


def test_the_stamp_and_the_values_come_from_the_same_bytes(env_file):
    """The property the whole module exists for: a snapshot cannot report a
    fingerprint for one revision and values for another."""
    path = env_file(f"{KEY}=one\n")
    first = snap.read_snapshot(path)

    env_file(f"{KEY}=two\n")
    second = snap.read_snapshot(path)

    assert first.stamp != second.stamp
    assert first.env[KEY] == "one" and second.env[KEY] == "two"
    assert first.digests != second.digests


def test_a_same_length_rewrite_still_moves_the_stamp(env_file):
    """A re-login rewrites `.env` with a session string of the same length, so
    size and a coarse mtime both stay put. CI caught that on Windows."""
    path = env_file(f"{KEY}=aaaa\n")
    before = snap.read_snapshot(path).stamp
    env_file(f"{KEY}=bbbb\n")

    assert snap.read_snapshot(path).stamp != before


def test_an_unchanged_file_reads_identically(env_file):
    path = env_file(f"{KEY}=one\n")

    assert snap.read_snapshot(path) == snap.read_snapshot(path)


def test_a_missing_file_is_not_an_error(tmp_path, monkeypatch):
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {KEY: "external-B"})

    taken = snap.read_snapshot(str(tmp_path / "absent.env"))

    assert taken.stamp == ()
    assert taken.env[KEY] == "external-B", "real environment variables must still work"


def test_an_empty_file_configures_nothing_and_says_so(env_file):
    path = env_file("")

    taken = snap.read_snapshot(path)

    assert taken.digests == {}
    assert taken.stamp != (), "an empty file is a revision, not an absent one"


def test_a_malformed_file_is_refused_rather_than_partly_applied(env_file):
    """INVERTED 2026-09-19. This used to assert that the reader returns a
    snapshot for a file it could not fully parse, on the reasoning that a broken
    `.env` must not take the server down.

    The server still does not go down - the caller catches this and keeps the
    running accounts - but the reader must not hand back HALF a revision. dotenv
    drops the lines it cannot parse and returns the rest, so a file with one bad
    line arrived looking complete, and the reload that "succeeded" then reported
    every account on a dropped line as deliberately removed.
    """
    path = env_file("this is not = a = valid line\n\x00\n")

    with pytest.raises(StartupMessage, match="could not be parsed"):
        snap.read_snapshot(path)


def test_a_snapshot_cannot_be_edited_apart(env_file):
    path = env_file(f"{KEY}=one\n")
    taken = snap.read_snapshot(path)

    with pytest.raises(Exception):
        taken.stamp = ("something else",)


# --- what the startup path is built from --------------------------------------


def test_startup_builds_clients_digests_and_stamp_from_one_revision(monkeypatch, tmp_path):
    """The window the audit names: an edit between startup's reads was recorded
    as the ACTIVE revision while the old client kept serving it."""
    from telegram_mcp import connection as conn

    path = tmp_path / ".env"
    path.write_text(f"{KEY}=one\n", encoding="utf-8")
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})

    reads = []
    real = snap.read_snapshot

    def _counting(p=None):
        taken = real(str(path))
        reads.append(taken.stamp)
        return taken

    monkeypatch.setattr(conn, "read_snapshot", _counting)
    monkeypatch.setattr(conn, "_env_stamp", ())
    monkeypatch.setattr(conn, "_env_digests", {})
    monkeypatch.setattr(conn, "clients", {})
    built = {}

    def _discover(env=None, reuse=None):
        built.update(env or {})
        return {"x": object()}

    monkeypatch.setattr(conn, "_discover_accounts", _discover)
    monkeypatch.setattr(conn._admission, "reject_duplicate_sessions", lambda _c: None)
    monkeypatch.setattr(conn, "_notify_clients_changed", lambda *a, **k: None)

    conn.refresh_accounts()

    assert len(reads) == 1, f"the reload read the configuration {len(reads)} times"
    assert built[KEY] == "one"
    assert conn._env_stamp == reads[0]
    assert conn._env_digests == snap.digests_of({KEY: "one"})


def test_a_refused_revision_is_seen_but_not_made_active(monkeypatch, tmp_path):
    """Seen and active are different facts. The stamp moves so a broken file is
    not re-parsed every call; the digests stay on the generation still serving."""
    from telegram_mcp import connection as conn

    path = tmp_path / ".env"
    path.write_text(f"{KEY}=new\n", encoding="utf-8")
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.setattr(conn, "read_snapshot", lambda p=None: snap.read_snapshot(str(path)))
    monkeypatch.setattr(conn, "_env_stamp", ())
    monkeypatch.setattr(conn, "_env_digests", {"TELEGRAM_SESSION_STRING_OLD": "d"})
    monkeypatch.setattr(conn, "clients", {"old": object()})
    lifecycle.clear_rejection()

    def _refuse(env=None, reuse=None):
        raise conn.StartupMessage("two labels share one session")

    monkeypatch.setattr(conn, "_discover_accounts", _refuse)

    assert conn.refresh_accounts() == []
    assert conn._env_digests == {"TELEGRAM_SESSION_STRING_OLD": "d"}, "a refusal was applied"
    assert conn._env_stamp != (), "the broken file will be re-parsed on every call"
    assert conn.last_rejection() is not None, "the refusal was not recorded anywhere"
    assert "share one session" in conn.last_rejection()[1]


def test_a_fixed_file_recovers_after_a_refused_revision(monkeypatch, tmp_path):
    from telegram_mcp import connection as conn

    path = tmp_path / ".env"
    monkeypatch.setattr(snap, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.setattr(conn, "read_snapshot", lambda p=None: snap.read_snapshot(str(path)))
    monkeypatch.setattr(conn, "_env_stamp", ())
    monkeypatch.setattr(conn, "_env_digests", {})
    monkeypatch.setattr(conn, "clients", {})
    lifecycle.clear_rejection()
    monkeypatch.setattr(conn._admission, "reject_duplicate_sessions", lambda _c: None)
    monkeypatch.setattr(conn, "_notify_clients_changed", lambda *a, **k: None)

    path.write_text(f"{KEY}=broken\n", encoding="utf-8")
    monkeypatch.setattr(
        conn,
        "_discover_accounts",
        lambda env=None, reuse=None: (_ for _ in ()).throw(ValueError()),
    )
    conn.refresh_accounts()

    path.write_text(f"{KEY}=fixed\n", encoding="utf-8")
    monkeypatch.setattr(conn, "_discover_accounts", lambda env=None, reuse=None: {"x": object()})

    assert conn.refresh_accounts() == ["x"]
    assert conn.last_rejection() is None, "the cleared rejection was still being reported"
