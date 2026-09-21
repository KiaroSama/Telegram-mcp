"""Everything that must outlive a container replacement lands under the mount.

Only `/data` was declared persistent, and the default Telethon session path
pointed there - so a session file survived. `state_dir()` did not: it resolved
under `XDG_STATE_HOME`, which defaulted to the container user's home, and every
TDLib database, the identity note beside each one, any quarantined database, the
alias store and the event feed followed it into the container's own filesystem.

Replacing the container therefore kept the login and lost the secret chats.
That asymmetry is the defect, and losing a TDLib database is not a re-login: its
secret-chat keys cannot be re-derived, so the messages they decrypt go with them.

These are path and image-definition tests. **No container is built or run here**
- the owner's standing instruction for this repository is that Docker is not
executed - so the end-to-end proof (create state, replace the container with the
same volume, show the bytes survive) is NOT covered by this file and is recorded
as untested.
"""

import re
from pathlib import Path

import pytest

from telegram_mcp import settings

REPO = Path(__file__).resolve().parents[1]
DOCKERFILE = (REPO / "Dockerfile").read_text(encoding="utf-8")


def _env_value(name: str):
    found = re.search(rf'^ENV {name}="([^"]*)"', DOCKERFILE, re.M)
    return found.group(1) if found else None


# --- what the image declares ---------------------------------------------------


def test_the_image_points_the_state_directory_at_the_mounted_volume():
    """The whole repair, in one line of the image: `state_dir()` already honours
    XDG_STATE_HOME, so pointing it under /data moves everything that follows it."""
    assert _env_value("XDG_STATE_HOME") == "/data/state"


def test_the_session_default_is_still_under_the_volume():
    assert (
        _env_value(
            "TELEGRAM_SESSION_NAME",
        )
        == "/data/telegram_mcp_session"
    )


def test_the_volume_is_declared():
    assert 'VOLUME ["/data"]' in DOCKERFILE


def test_the_state_directory_exists_before_the_user_switch():
    """A bind mount onto a path that does not exist yet is created by the daemon
    as root, and the non-root account then cannot write it."""
    made = DOCKERFILE.index("mkdir -p /data /data/state")
    switched = DOCKERFILE.index("USER appuser")

    assert made < switched


def test_the_state_directory_is_owned_by_the_non_root_account():
    assert "chown -R appuser:appuser /app /data" in DOCKERFILE


def test_the_state_directory_is_private():
    """It holds session files and TDLib databases, each of which IS the account
    to whoever can read it."""
    assert "chmod 700 /data/state" in DOCKERFILE


def test_the_image_still_pins_its_interpreter_and_installs_from_the_lock():
    """Guarding the repair against collateral damage: these are the properties
    the audit asked to be preserved, not improved."""
    assert "ENV UV_PYTHON=/usr/local/bin/python3" in DOCKERFILE
    assert "UV_PYTHON_DOWNLOADS=never" in DOCKERFILE
    assert "--frozen" in DOCKERFILE or "--locked" in DOCKERFILE


def test_the_compose_file_documents_what_the_volume_carries():
    compose = (REPO / "docker-compose.yml").read_text(encoding="utf-8")

    assert "/data/state" in compose


# --- where the state actually resolves to --------------------------------------


@pytest.fixture
def container_like(monkeypatch, tmp_path):
    """The image's own environment, with a writable stand-in for /data."""
    root = tmp_path / "data" / "state"
    root.mkdir(parents=True)
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "data" / "state"))
    return tmp_path / "data"


def test_state_dir_follows_the_override(container_like):
    assert settings.state_dir() == container_like / "state" / "telegram-mcp"


def test_a_secret_chat_key_store_lands_under_the_mount(container_like):
    from telegram_mcp import secret_backend

    assert container_like in secret_backend._storage_for("work").path.parents


def test_the_event_feed_lands_under_the_mount(container_like, monkeypatch):
    from telegram_mcp.tools import events_store

    monkeypatch.delenv("TELEGRAM_EVENT_FEED_FILE", raising=False)

    assert container_like in events_store.feed_file_path().parents


def test_a_file_session_lands_under_the_mount(container_like):
    from telegram_mcp import session_files

    assert container_like in Path(session_files.session_file_path("work")).parents


def test_an_explicit_override_still_wins(monkeypatch, tmp_path):
    """A deployment that already places its state somewhere must keep doing so."""
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "elsewhere"))

    assert settings.state_dir() == tmp_path / "elsewhere" / "telegram-mcp"


# --- the deployment that already exists ----------------------------------------


def test_state_left_in_the_previous_location_is_reported_not_moved(monkeypatch, tmp_path):
    """Nothing here relocates a TDLib database. Its secret-chat keys cannot be
    re-derived, so the answer to finding one is to say so - never to tidy it."""
    home = tmp_path / "home"
    legacy = home / ".local" / "state" / "telegram-mcp" / "secret-chats" / "work"
    legacy.mkdir(parents=True)
    (legacy / "td.binlog").write_bytes(b"keys that cannot be re-derived")
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "data" / "state"))

    stranded = settings.stranded_state_dir()

    assert stranded == home / ".local" / "state" / "telegram-mcp"
    assert (legacy / "td.binlog").exists(), "the check moved or removed something"


def test_nothing_is_reported_when_the_new_location_is_already_in_use(monkeypatch, tmp_path):
    home = tmp_path / "home"
    (home / ".local" / "state" / "telegram-mcp").mkdir(parents=True)
    (home / ".local" / "state" / "telegram-mcp" / "old").write_text("x", encoding="utf-8")
    current = tmp_path / "data" / "state" / "telegram-mcp"
    current.mkdir(parents=True)
    (current / "in-use").write_text("x", encoding="utf-8")
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "data" / "state"))

    assert settings.stranded_state_dir() is None


def test_nothing_is_reported_when_there_is_nothing_there(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "data" / "state"))

    assert settings.stranded_state_dir() is None


def test_nothing_is_reported_when_the_location_did_not_move(monkeypatch, tmp_path):
    home = tmp_path / "home"
    legacy = home / ".local" / "state" / "telegram-mcp"
    legacy.mkdir(parents=True)
    (legacy / "something").write_text("x", encoding="utf-8")
    monkeypatch.setattr(Path, "home", staticmethod(lambda: home))
    monkeypatch.delenv("XDG_STATE_HOME", raising=False)

    assert settings.stranded_state_dir() is None
