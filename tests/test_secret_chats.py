"""The five chat-lifecycle tools, on the replacement backend.

What these pin is the part of the migration a reader cannot check by eye: the
published answer shape was fixed while the previous backend was installed, and the
package underneath speaks a different vocabulary. Three translations carry the
difference and each is asserted here rather than assumed -

* both published ids survive, and either one reaches the chat;
* `self_destruct_timer` appears ONLY when the timer is off, which is how it was
  published and therefore what a caller branching on it means;
* an unknown chat is a named refusal rather than a traceback.
"""

import json

import pytest

from telegram_mcp.tools import secret_chats as sc

from secret_fakes import CHAT_ID, SECRET_ID


def _results(raw):
    return json.loads(raw)["results"]


# --- the two ids ---------------------------------------------------------------


def test_the_two_published_ids_are_one_chat():
    """Measured on this account's own chats, both pairs agreeing. If this drifts,
    every id an agent wrote down before the migration stops resolving."""
    assert sc.to_secret_id(-1999372142509) == 627857491
    assert sc.to_secret_id(-1999794063129) == 205936871


def test_either_id_reaches_the_chat():
    assert sc.to_secret_id(CHAT_ID) == SECRET_ID
    assert sc.to_secret_id(SECRET_ID) == SECRET_ID


# --- listing -------------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_chat_is_published_with_both_ids_and_its_state(backend, monkeypatch):
    monkeypatch.setattr(sc, "get_client", lambda account=None: _client())
    monkeypatch.setattr(sc, "ensure_connected", _noop)

    answer = _results(await sc.list_secret_chats(account="acct"))

    assert answer[0]["chat_id"] == CHAT_ID
    assert answer[0]["secret_chat_id"] == SECRET_ID
    assert answer[0]["state"] == "ready"
    assert answer[0]["is_outbound"] is True


@pytest.mark.asyncio
async def test_a_chat_with_no_timer_says_so_in_words(backend, monkeypatch):
    """The field is absent when a timer IS set, so its presence is the signal -
    and a caller switching on that would break if it were always there."""
    monkeypatch.setattr(sc, "get_client", lambda account=None: _client())
    monkeypatch.setattr(sc, "ensure_connected", _noop)

    off = _results(await sc.list_secret_chats(account="acct"))[0]
    assert off["self_destruct_timer_seconds"] == 0
    assert "off" in off["self_destruct_timer"]

    backend.status(SECRET_ID).ttl = 60
    armed = _results(await sc.list_secret_chats(account="acct"))[0]
    assert armed["self_destruct_timer_seconds"] == 60
    assert "self_destruct_timer" not in armed


@pytest.mark.asyncio
async def test_no_chats_says_they_are_per_device(backend, monkeypatch):
    """The empty answer is where a caller decides whether to go looking on their
    phone, so it has to say that looking will not help."""
    backend._chats.clear()
    monkeypatch.setattr(sc, "get_client", lambda account=None: _client())
    monkeypatch.setattr(sc, "ensure_connected", _noop)

    answer = await sc.list_secret_chats(account="acct")

    assert "per-device" in answer


@pytest.mark.asyncio
async def test_an_unresolvable_peer_costs_a_blank_name_not_an_error(backend, monkeypatch):
    """A name is a convenience on every one of these answers. A peer this account
    cannot resolve must not make the chat unlistable."""

    class _Failing:
        async def get_entity(self, _):
            raise ValueError("no such user")

    monkeypatch.setattr(sc, "get_client", lambda account=None: _Failing())
    monkeypatch.setattr(sc, "ensure_connected", _noop)

    answer = _results(await sc.list_secret_chats(account="acct"))

    assert answer[0]["title"] == ""
    assert answer[0]["chat_id"] == CHAT_ID


# --- creating ------------------------------------------------------------------


@pytest.mark.asyncio
async def test_creating_says_the_chat_is_not_usable_yet(backend, monkeypatch):
    """The invitation returning is not the chat working, and a caller who sends
    immediately gets a refusal from the protocol instead of an explanation."""
    monkeypatch.setattr(sc, "get_client", lambda account=None: _client())
    monkeypatch.setattr(sc, "ensure_connected", _noop)
    monkeypatch.setattr(sc, "resolve_entity", _resolve)

    answer = _results(await sc.create_secret_chat(5001, account="acct"))

    assert answer["state"] == "requested"
    assert "key exchange" in answer["note"]
    assert backend.created == [_PEER]


@pytest.mark.asyncio
async def test_a_peer_that_resolves_to_nothing_is_named(backend, monkeypatch):
    async def _nothing(user_id, client):
        return object()

    monkeypatch.setattr(sc, "get_client", lambda account=None: _client())
    monkeypatch.setattr(sc, "ensure_connected", _noop)
    monkeypatch.setattr(sc, "resolve_entity", _nothing)

    assert "did not resolve" in await sc.create_secret_chat("ghost", account="acct")


# --- the timer -----------------------------------------------------------------


@pytest.mark.asyncio
async def test_the_timer_is_set_on_the_chat_and_says_what_it_reaches(backend):
    answer = _results(await sc.set_secret_chat_timer(CHAT_ID, 60, account="acct"))

    assert backend.ttls == [(SECRET_ID, 60)]
    assert answer["self_destruct_timer_seconds"] == 60
    assert "not existing ones" in answer["applies_to"]


@pytest.mark.asyncio
async def test_the_timer_accepts_the_secret_chat_id_too(backend):
    await sc.set_secret_chat_timer(SECRET_ID, 30, account="acct")

    assert backend.ttls == [(SECRET_ID, 30)]


@pytest.mark.asyncio
async def test_a_chat_this_login_does_not_have_is_named_not_raised(backend):
    answer = await sc.set_secret_chat_timer(424242, 60, account="acct")

    assert "No secret chat" in answer and "list_secret_chats" in answer


# --- closing -------------------------------------------------------------------


@pytest.mark.asyncio
async def test_closing_reports_the_key_is_gone_on_both_sides(backend):
    answer = _results(await sc.close_secret_chat(SECRET_ID, account="acct"))

    assert backend.closed == [SECRET_ID]
    assert answer["closed"] is True
    assert "cannot be reopened" in answer["note"]


@pytest.mark.asyncio
async def test_closing_accepts_the_larger_id_too(backend):
    """The argument is documented as the smaller id, and a caller holding the
    other one should not have to do arithmetic to end a chat."""
    await sc.close_secret_chat(CHAT_ID, account="acct")

    assert backend.closed == [SECRET_ID]


# --- status --------------------------------------------------------------------


@pytest.mark.asyncio
async def test_status_is_ready_and_carries_the_capability_list(backend):
    answer = _results(await sc.secret_chat_status(account="acct"))

    assert answer["secret_chats"] == "ready"
    assert answer["capabilities"], "the capability list is why this tool is read"
    assert "secret-chats" in answer["key_store"]


@pytest.mark.asyncio
async def test_status_reports_the_capabilities_even_when_unavailable(backend, monkeypatch):
    """The verdicts describe the PROTOCOL, not this installation, so they are
    just as true with the backend down - and a caller planning work deserves them
    before fixing setup."""
    from telegram_mcp.secret_common import SecretChatUnavailable

    async def _down(account):
        raise SecretChatUnavailable(account, "the server is shutting down")

    monkeypatch.setattr(sc, "secret_manager", _down)

    answer = _results(await sc.secret_chat_status(account="acct"))

    assert answer["secret_chats"] == "unavailable"
    assert "shutting down" in answer["reason"]
    assert answer["capabilities"]


@pytest.mark.asyncio
async def test_status_no_longer_claims_a_second_login_is_needed(backend):
    """The whole point of the migration. A fix naming a second sign-in would send
    an operator to do something that is no longer possible or necessary."""
    answer = await sc.secret_chat_status(account="acct")

    assert "secret_chat_login" not in answer
    assert "tdlib" not in answer.lower()


# --- helpers -------------------------------------------------------------------


class _Peer:
    id = 5001
    first_name = "Kay"
    last_name = None
    username = "kay"


_PEER = _Peer()


class _Client:
    async def get_entity(self, _):
        return _PEER


def _client():
    return _Client()


async def _noop(client):
    return None


async def _resolve(user_id, client):
    return _PEER
