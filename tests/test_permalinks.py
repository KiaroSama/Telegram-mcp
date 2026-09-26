"""The link domain has one home (spec 009).

`TELEGRAM_LINK_DOMAIN` existed for years, but only message links obeyed it: nine other
places pasted a fixed `t.me` address, so an owner who changed the setting got half their
links on the old domain. These tests pin the one builder every link now goes through, and
the username prefixes it implies.
"""

import pytest

from telegram_mcp import permalinks
from telegram_mcp.tools import channel_admin


@pytest.fixture
def domain(monkeypatch):
    def _set(value):
        monkeypatch.setattr(permalinks, "LINK_DOMAIN", permalinks.normalize_domain(value))

    return _set


def test_the_default_link_is_the_usual_one():
    assert permalinks.normalize_domain(None) == "t.me"
    assert permalinks.public_link("name") == "https://t.me/name"
    assert (
        permalinks.public_link("addstickers", "HotCherry") == "https://t.me/addstickers/HotCherry"
    )


@pytest.mark.parametrize(
    "raw", ["tg.example", "https://tg.example", "https://tg.example/", " tg.example/ "]
)
def test_a_configured_domain_is_used_as_a_bare_host(domain, raw):
    domain(raw)
    assert permalinks.public_link("name") == "https://tg.example/name"


def test_message_links_follow_the_same_setting(domain):
    domain("tg.example")
    assert (
        permalinks.post_link(permalinks.LINK_DOMAIN, 7, username="news")
        == "https://tg.example/news/7"
    )


@pytest.mark.parametrize(
    "raw",
    [
        "@news_room",
        "t.me/news_room",
        "https://t.me/news_room",
        "http://T.ME/news_room",
        "news_room",
    ],
)
def test_usernames_are_read_in_every_usual_form(raw):
    assert channel_admin._normalize_username(raw) == "news_room"


def test_the_configured_domain_is_accepted_as_input_too(domain):
    domain("tg.example")
    assert channel_admin._normalize_username("https://tg.example/news_room") == "news_room"
    assert channel_admin._normalize_username("t.me/news_room") == "news_room"


def test_a_username_that_only_starts_like_a_domain_is_kept():
    assert channel_admin._normalize_username("tmember") == "tmember"


@pytest.mark.parametrize(
    "module", ["channel_admin", "groups", "message_search", "profile", "stickers"]
)
def test_no_tool_pastes_a_fixed_link_domain(module):
    """The bug this spec fixes: a hard-coded address ignores TELEGRAM_LINK_DOMAIN."""
    import importlib
    import inspect

    source = inspect.getsource(importlib.import_module(f"telegram_mcp.tools.{module}"))
    assert "https://t.me" not in source and "http://t.me" not in source
