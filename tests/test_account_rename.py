"""Renaming an account label carries everything the server stored under it (spec 025).

The label is the key for more than the `.env` line: the secret-chat key store and
its sidecars, ghost-mode settings, proxy routes, approval records, always-approvals
and contact aliases are all filed under it. A rename that moved only `.env` left the
account's secret chats unreadable under the new name.
"""

import json

import pytest

from telegram_mcp import account_rename


def _write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


def _read(path):
    return json.loads(path.read_text(encoding="utf-8"))


@pytest.fixture
def state(tmp_path):
    chats = tmp_path / "secret-chats"
    (chats / "old").mkdir(parents=True)
    (chats / "old" / "keys.db").write_bytes(b"k")
    for name in ("old.owner.json", "old-history.json", "old-media.json"):
        (chats / name).write_text("{}", encoding="utf-8")
    (chats / "other").mkdir()
    _write(
        tmp_path / "ghost.json",
        {"default": True, "accounts": {"old": False}, "chats": {"old": {"5": True}}},
    )
    _write(
        tmp_path / "proxy-pool.json",
        {"shared": [], "accounts": {"old": [1]}, "routes": {"old": "x"}},
    )
    _write(
        tmp_path / "approval-messages.json", {"codes": ["A"], "accounts": {"old": {"selves": []}}}
    )
    _write(
        tmp_path / "always-approvals.json",
        {
            "grants": [["old", "send_message", "1"], ["other", "send_message", "1"]],
            "folders": ["f"],
        },
    )
    _write(tmp_path / "aliases.json", {"old\nboss": 1, "other\nboss": 2, "global": 3})
    return tmp_path


def test_every_piece_moves_to_the_new_label(state):
    moved = account_rename.rename(state, "old", "new")

    chats = state / "secret-chats"
    assert (chats / "new" / "keys.db").read_bytes() == b"k" and not (chats / "old").exists()
    for name in ("new.owner.json", "new-history.json", "new-media.json"):
        assert (chats / name).exists()
    assert (chats / "other").exists()
    assert _read(state / "ghost.json")["accounts"] == {"new": False}
    assert _read(state / "ghost.json")["chats"] == {"new": {"5": True}}
    pool = _read(state / "proxy-pool.json")
    assert pool["accounts"] == {"new": [1]} and pool["routes"] == {"new": "x"}
    assert _read(state / "approval-messages.json")["accounts"] == {"new": {"selves": []}}
    assert _read(state / "always-approvals.json")["grants"] == [
        ["new", "send_message", "1"],
        ["other", "send_message", "1"],
    ]
    assert _read(state / "aliases.json") == {"new\nboss": 1, "other\nboss": 2, "global": 3}
    assert "secret-chats/old" in moved and "ghost.json" in moved


def test_a_label_with_nothing_stored_is_a_no_op(tmp_path):
    assert account_rename.rename(tmp_path, "old", "new") == []


def test_an_existing_target_refuses_before_anything_moves(state):
    (state / "secret-chats" / "new-history.json").write_text("{}", encoding="utf-8")

    with pytest.raises(account_rename.RenameRefused, match="new-history.json"):
        account_rename.rename(state, "old", "new")

    assert (state / "secret-chats" / "old").exists()
    assert _read(state / "ghost.json")["accounts"] == {"old": False}


def test_a_target_key_already_in_a_store_refuses(state):
    _write(state / "proxy-pool.json", {"accounts": {"old": [1], "new": [2]}, "routes": {}})

    with pytest.raises(account_rename.RenameRefused, match="proxy-pool.json"):
        account_rename.rename(state, "old", "new")

    assert (state / "secret-chats" / "old").exists()


def test_a_failure_part_way_puts_everything_back(state, monkeypatch):
    real = account_rename._write_json

    def fail_on_aliases(path, data):
        if path.name == "aliases.json":
            raise OSError("disk full")
        real(path, data)

    monkeypatch.setattr(account_rename, "_write_json", fail_on_aliases)

    with pytest.raises(OSError):
        account_rename.rename(state, "old", "new")

    assert (state / "secret-chats" / "old" / "keys.db").exists()
    assert not (state / "secret-chats" / "new").exists()
    assert _read(state / "ghost.json")["accounts"] == {"old": False}
    assert _read(state / "always-approvals.json")["grants"][0][0] == "old"


def test_the_command_line_reports_and_refuses(state, capsys):
    assert account_rename.main([str(state), "old", "new"]) == 0
    assert "secret-chats/old" in capsys.readouterr().out

    assert account_rename.main([str(state), "missing", "new"]) == 0
    assert account_rename.main([str(state), "other", "new"]) == 2
