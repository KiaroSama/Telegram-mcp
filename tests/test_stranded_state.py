"""A state directory left behind is reported, and a log file does not hide it.

Pointing `XDG_STATE_HOME` somewhere new - the container image now puts it under
the mounted volume - leaves whatever was in the old location exactly where it
was. Nothing here moves it: a TDLib database's secret-chat keys cannot be
re-derived, so losing one loses the messages it decrypts. The only job is to say
the old directory is there before a fresh sign-in happens beside it.

The defect: the check returned "nothing stranded" whenever the NEW directory
contained anything at all, and ordinary import-time logging creates
`mcp_errors.log` in that directory before startup ever runs the check. So the
warning could not fire on the one deployment shape it exists for.
"""

from pathlib import Path

import pytest

from telegram_mcp import settings


@pytest.fixture
def locations(tmp_path, monkeypatch):
    """A legacy home and a fresh state directory, both real."""
    home = tmp_path / "home"
    legacy = home / ".local" / "state" / "telegram-mcp"
    legacy.mkdir(parents=True)
    current = tmp_path / "data" / "state" / "telegram-mcp"
    current.mkdir(parents=True)
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "data" / "state"))
    return legacy, current


def _tdlib_database(root: Path, account: str = "work") -> None:
    directory = root / "tdlib" / account
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "td.binlog").write_bytes(b"keys that cannot be re-derived")


def test_a_log_file_in_the_new_location_does_not_hide_an_old_database(locations):
    """The counterexample. `mcp_errors.log` is created at import time, before
    `_main` asks - so on every real deployment the check answered "nothing
    here" while a TDLib database sat in the old location."""
    legacy, current = locations
    _tdlib_database(legacy)
    (current / "mcp_errors.log").write_text("starting up\n", encoding="utf-8")

    assert settings.stranded_state_dir() == legacy


def test_a_session_file_left_behind_is_reported(locations):
    legacy, current = locations
    (legacy / "telegram_mcp_session.session").write_bytes(b"an authorisation")
    (current / "mcp_errors.log").write_text("x", encoding="utf-8")

    assert settings.stranded_state_dir() == legacy


def test_an_alias_store_left_behind_is_reported(locations):
    legacy, current = locations
    (legacy / "aliases.json").write_text("{}", encoding="utf-8")

    assert settings.stranded_state_dir() == legacy


def test_a_completed_migration_reports_nothing(locations):
    """The state moved across, so there is nothing to warn about - even though
    the old directory still exists and still has a stale log in it."""
    legacy, current = locations
    (legacy / "mcp_errors.log").write_text("old log\n", encoding="utf-8")
    _tdlib_database(current)

    assert settings.stranded_state_dir() is None


def test_a_partial_migration_is_still_reported(locations):
    """The session was copied and the TDLib database was not. That is exactly
    the case where saying nothing costs the secret chats."""
    legacy, current = locations
    _tdlib_database(legacy)
    (current / "telegram_mcp_session.session").write_bytes(b"an authorisation")

    assert settings.stranded_state_dir() == legacy


def test_an_old_directory_holding_only_a_log_is_not_reported(locations):
    """Nothing of value is over there; a warning about it is noise that trains
    the operator to ignore the real one."""
    legacy, _current = locations
    (legacy / "mcp_errors.log").write_text("old log\n", encoding="utf-8")

    assert settings.stranded_state_dir() is None


def test_nothing_is_reported_when_there_is_no_old_directory(tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "data" / "state"))

    assert settings.stranded_state_dir() is None


def test_nothing_is_reported_when_the_location_never_moved(tmp_path, monkeypatch):
    home = tmp_path / "home"
    legacy = home / ".local" / "state" / "telegram-mcp"
    legacy.mkdir(parents=True)
    _tdlib_database(legacy)
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.delenv("XDG_STATE_HOME", raising=False)

    assert settings.stranded_state_dir() is None


def test_nothing_is_moved_or_removed_by_the_check(locations):
    legacy, _current = locations
    _tdlib_database(legacy)

    settings.stranded_state_dir()

    assert (legacy / "tdlib" / "work" / "td.binlog").exists()
