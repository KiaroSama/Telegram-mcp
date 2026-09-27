"""Bypass mode (spec 016): on only from the approval bot, for a time or until turned off.

What these pin: off by default; an expired or unreadable state is off (fail closed);
the three durations; and no tool may name the file.
"""

import time

import pytest

from telegram_mcp.safeguard import bypass, middleware


@pytest.fixture(autouse=True)
def path(tmp_path, monkeypatch):
    target = tmp_path / "bypass.json"
    monkeypatch.setattr(bypass, "bypass_path", lambda: target)
    return target


def test_off_by_default():
    assert not bypass.active() and bypass.describe() == "off"


def test_on_until_turned_off():
    bypass.turn_on(None, by=1)
    assert bypass.active() and "until you turn it off" in bypass.describe()
    bypass.turn_off()
    assert not bypass.active()


def test_a_timed_bypass_ends_by_itself(monkeypatch):
    bypass.turn_on(3600, by=1)
    assert bypass.active()
    later = time.time() + 3601
    monkeypatch.setattr(bypass.time, "time", lambda: later)
    assert not bypass.active()


def test_an_unreadable_state_is_off(path):
    path.write_text("{not json", encoding="utf-8")
    assert not bypass.active()


def test_no_tool_may_name_the_file(path):
    assert str(path) in middleware._protected_paths()
