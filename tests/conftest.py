"""Shared pytest setup for import-time Telegram configuration."""

import os

import pytest


# The suite describes the CODE, never the machine it runs on.
#
# Startup discovery now resolves the `.env` the way the reload path always did -
# `find_dotenv(usecwd=True)` - so on a developer's own checkout it finds THEIR
# configuration. Before, the two disagreed about which file they meant, and the
# tests were quietly relying on startup finding nothing.
#
# What that dependence costs is not hypothetical: with a real `.env` present the
# registry holds that person's accounts, every non-readonly tool becomes
# multi-account and refuses a call with no `account`, and a hundred tests fail on
# one machine and pass on another. It also points the suite at live logins.
#
# Stubbed at module scope because `telegram_mcp.connection` builds its registry
# at IMPORT time, which happens while the first test module is collected - a
# fixture would be far too late.
def _no_dotenv_for_tests(*_args, **_kwargs):
    return ""


import dotenv  # noqa: E402

dotenv.find_dotenv = _no_dotenv_for_tests
dotenv.main.find_dotenv = _no_dotenv_for_tests

os.environ.setdefault("TELEGRAM_API_ID", "12345")
os.environ.setdefault("TELEGRAM_API_HASH", "dummy_hash")
os.environ.setdefault("TELEGRAM_SESSION_NAME", "test_session")
for _name in list(os.environ):
    # Any account the developer's environment supplies, out of the way too: a
    # test that means to configure two accounts says so itself.
    if _name.startswith(("TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME_")):
        del os.environ[_name]


@pytest.fixture(autouse=True)
def _tdlib_not_shutting_down():
    """Clear the shutdown latch between tests.

    `tdlib_registry.close_all()` sets `_closing` and deliberately never clears
    it: once a shutdown has begun, starting a new native client against a
    database being flushed is never right. That is correct for a process and
    poisonous for a test session, where one suite calling `close_all` left every
    later `secret_client` in any file answering "the server is shutting down" -
    which CI found and a local run, in a different order, did not.

    Autouse and here rather than in each suite, because the leak is a property
    of the module rather than of any one test file.
    """
    from telegram_mcp import admission, tdlib_registry

    tdlib_registry._closing = False
    # The same shape one module along: `admission.release_all()` closes the door
    # so a slow acquire cannot publish after shutdown, and every suite's cleanup
    # calls it. Without this, the first cleanup left every later admission in the
    # session silently refusing to publish.
    admission.begin_serving()
    admission.unreleased_leases.clear()
    yield
    tdlib_registry._closing = False
    admission.begin_serving()
    admission.unreleased_leases.clear()


@pytest.fixture
def wire_client(monkeypatch):
    """Patch a tool module's client seams in one call.

    There were 33 hand-rolled copies of this across 28 test files, and that is
    the direct reason ~38 registered tools have no behavioural test: writing one
    started with 25 lines of boilerplate before it could assert anything.

    Patch the module that OWNS each name. The tool modules star-import from
    `runtime`, so patching through `runtime` binds a second name and changes
    nothing the tool actually calls - the trap `tests/test_tool_registry.py`
    exists to catch.
    """

    def _wire(module, client, *, resolve=None, entity=None, marked_id=None):
        # `with_account` refreshes before it decides single- or multi-mode, which
        # it must: the registry only moves when something refreshes it, so a
        # second account added while the server ran was invisible to the routing
        # decision. In a test that means the REAL `.env` would be read and the
        # machine's own accounts published - so the wired client is the whole
        # registry here, and the reload is a no-op.
        from telegram_mcp import connection as conn

        monkeypatch.setattr(conn, "refresh_accounts", lambda: [])
        monkeypatch.setattr(conn, "clients", {"default": client})

        async def _resolve_entity(chat_id, cl=None, account=None):
            if resolve is not None:
                return await resolve(chat_id, cl, account)
            return entity if entity is not None else object()

        async def _ensure_connected(_client=None):
            return None

        monkeypatch.setattr(module, "get_client", lambda account=None: client)
        for name, value in (
            ("ensure_connected", _ensure_connected),
            ("resolve_entity", _resolve_entity),
            ("resolve_input_entity", _resolve_entity),
        ):
            if hasattr(module, name):
                monkeypatch.setattr(module, name, value)
        if marked_id is not None and hasattr(module, "get_marked_id"):
            monkeypatch.setattr(module, "get_marked_id", marked_id)
        return client

    return _wire
