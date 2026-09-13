"""A secret-chat tool must prove whose database it is talking to.

The login path compared TDLib's `getMe.id` against the Telethon session's, and
that was the only place it happened. Everything else reached a client through
`secret_client()`, which compared the database against the NOTE BESIDE IT - so a
label reused for another account had an old database and an old note, the two
agreed perfectly, and every secret-chat call ran as the previous owner.

A cached client skipped even that. And `_is_usable` asked "not in a small set of
dead states", which let `authorizationStateWaitPassword`, `WaitCode` and any
state the set had never heard of be handed to a tool.

These go through the REGISTERED tool, not through `_attempt_login`, because
`_attempt_login` already passed the narrow check while every tool did not.
"""

import asyncio

import pytest

from telegram_mcp import tdlib, tdlib_identity as identity, tdlib_registry as reg
from telegram_mcp.tools import secret_chats


class _Telethon:
    def __init__(self, user_id):
        self.user_id = user_id

    async def get_me(self):
        return type("Me", (), {"id": self.user_id})()


class _TDLibClient:
    """A started TDLib client that answers `getMe` and reports a state."""

    def __init__(self, user_id, state="authorizationStateReady"):
        self.user_id = user_id
        self.authorization_state = state
        self._client_id = 1
        self.closed = 0

    async def start(self):
        return self.authorization_state

    async def request(self, obj, timeout=30.0):
        if obj["@type"] == "getMe":
            return {"@type": "user", "id": self.user_id}
        return {"@type": "ok"}

    async def close(self):
        self.closed += 1
        self._client_id = None


@pytest.fixture(autouse=True)
def _clean(monkeypatch, tmp_path):
    reg._by_account.clear()
    reg._verified_against.clear()
    monkeypatch.setattr(reg, "_by_account_lock", asyncio.Lock())
    monkeypatch.setattr(tdlib, "database_dir_for", lambda label: tmp_path / "tdlib" / label)
    (tmp_path / "tdlib" / "work").mkdir(parents=True, exist_ok=True)
    yield
    reg._by_account.clear()
    reg._verified_against.clear()


def _wire(monkeypatch, telethon, tdlib_client):
    """The production registry, with the two clients this test supplies."""
    from telegram_mcp import connection as conn

    monkeypatch.setattr(conn, "clients", {"work": telethon})
    monkeypatch.setattr(conn, "refresh_accounts", lambda: [])
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: tdlib_client)


# --- through a registered tool -------------------------------------------------


@pytest.mark.asyncio
async def test_a_tool_refuses_a_database_belonging_to_another_user(monkeypatch):
    """The defect, reached the way a caller reaches it: not through the login
    path, which already checked, but through an ordinary tool."""
    _wire(monkeypatch, _Telethon(222), _TDLibClient(user_id=111))

    answer = await secret_chats.list_secret_chats(account="work")

    assert "111" in answer and "222" in answer
    assert "work" not in reg._by_account, "the wrong owner's client was cached"


@pytest.mark.asyncio
async def test_a_tool_serves_when_the_two_halves_agree(monkeypatch):
    client = _TDLibClient(user_id=777)
    _wire(monkeypatch, _Telethon(777), client)

    await secret_chats.list_secret_chats(account="work")

    assert reg._by_account.get("work") is client


@pytest.mark.asyncio
async def test_a_legacy_database_with_no_note_is_verified_and_recorded(monkeypatch):
    """Missing metadata is a verification case, not permission to guess: with a
    live session in hand the comparison IS the proof, and the note is written."""
    _wire(monkeypatch, _Telethon(500), _TDLibClient(user_id=500))
    assert identity.read_identity("work") is None

    await secret_chats.list_secret_chats(account="work")

    assert identity.read_identity("work") == 500


@pytest.mark.asyncio
async def test_a_stale_note_does_not_override_the_live_session(monkeypatch):
    """The exact shape of the reused label: the database and its note agree with
    each other and both disagree with the configured account."""
    identity.record_identity("work", 111)
    _wire(monkeypatch, _Telethon(222), _TDLibClient(user_id=111))

    answer = await secret_chats.list_secret_chats(account="work")

    assert "222" in answer, "the live session was not what it was compared against"
    assert identity.read_identity("work") == 111, "a mismatch rewrote the metadata"


# --- the cached path -----------------------------------------------------------


@pytest.mark.asyncio
async def test_a_cached_client_is_revalidated_when_the_generation_changes(monkeypatch):
    """A re-login or a reload replaces the Telethon client object, and that is
    exactly when the proof has to be redone. The cache skipped it entirely."""
    from telegram_mcp import connection as conn

    first = _TDLibClient(user_id=111)
    _wire(monkeypatch, _Telethon(111), first)
    await secret_chats.list_secret_chats(account="work")
    assert reg._by_account["work"] is first

    # The label now names a different account - a new Telethon client object -
    # and a fresh start builds its own client rather than handing back the one
    # that was cached, which is what makes "closed once" a meaningful count.
    monkeypatch.setattr(conn, "clients", {"work": _Telethon(222)})
    rebuilt = _TDLibClient(user_id=111)
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: rebuilt)

    answer = await secret_chats.list_secret_chats(account="work")

    assert "111" in answer and "222" in answer, "the cached client was served unchecked"
    assert first.closed == 1, "the superseded client was left open"
    assert rebuilt.closed == 1, "the mismatching replacement was left open"
    assert "work" not in reg._by_account


@pytest.mark.asyncio
async def test_a_cached_client_is_reused_while_the_generation_stands(monkeypatch):
    """Re-proving on every call would cost a round trip per secret-chat
    operation for a question whose answer cannot have changed."""
    client = _TDLibClient(user_id=111)
    built = []

    def _one_only(account):
        built.append(account)
        return client

    _wire(monkeypatch, _Telethon(111), client)
    monkeypatch.setattr(tdlib, "TDLibClient", _one_only)

    await secret_chats.list_secret_chats(account="work")
    await secret_chats.list_secret_chats(account="work")

    assert built == ["work"], "the second call built a client instead of reusing one"
    assert reg._by_account["work"] is client


# --- which states may serve ----------------------------------------------------


@pytest.mark.parametrize(
    "state",
    [
        "authorizationStateWaitPassword",
        "authorizationStateWaitCode",
        "authorizationStateWaitPhoneNumber",
        "authorizationStateSomethingThisListHasNeverHeardOf",
    ],
)
def test_only_ready_counts_as_usable(state):
    """`not in a small set of dead states` is a much weaker claim than `Ready`,
    and every one of these passed it."""
    client = _TDLibClient(user_id=1, state=state)

    assert reg._is_usable(client) is False


def test_ready_counts_as_usable():
    assert reg._is_usable(_TDLibClient(user_id=1)) is True


def test_a_closed_client_is_not_usable():
    client = _TDLibClient(user_id=1)
    client._client_id = None

    assert reg._is_usable(client) is False


@pytest.mark.asyncio
async def test_a_cached_client_that_left_ready_is_replaced(monkeypatch):
    client = _TDLibClient(user_id=111)
    _wire(monkeypatch, _Telethon(111), client)
    await secret_chats.list_secret_chats(account="work")

    client.authorization_state = "authorizationStateWaitPassword"
    replacement = _TDLibClient(user_id=111)
    monkeypatch.setattr(tdlib, "TDLibClient", lambda account: replacement)

    await secret_chats.list_secret_chats(account="work")

    assert reg._by_account["work"] is replacement
    assert client.closed == 1
