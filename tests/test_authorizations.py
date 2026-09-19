"""Authorized devices: the polarity of one switch, and the guard on one logout.

Two things here can hurt the owner without saying a word.

`account.changeAuthorizationSettings` names its field
`encrypted_requests_disabled`, so True means secret chats are OFF. Everything a
caller of this server sees is `accept_secret_chats` — the switch as Telegram's
own Devices screen draws it, the way the owner thinks about it. An inverted
mapping raises nothing, logs nothing, and shows nothing until a secret chat
fails to arrive on a device the owner had just turned it ON for. So the polarity
is pinned from both ends: the value READ off an Authorization, and the value
actually SENT in the request.

`account.resetAuthorization` signs a real device out and cannot be undone. Its
guards are asserted on whether the request went out at all, never on the
returned sentence — the sentence reads the same whether the guard worked or not.

No network: a fake client records the TL requests the tools build.
"""

import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import authorizations as mod

RESET = functions.account.ResetAuthorizationRequest
CHANGE = functions.account.ChangeAuthorizationSettingsRequest


def _auth(hash=111, *, current=False, encrypted_requests_disabled=False, device_model="Pixel 8"):
    """One Authorization, with the fields Telegram always fills."""
    return types.Authorization(
        hash=hash,
        device_model=device_model,
        platform="Android",
        system_version="SDK 34",
        api_id=6,
        app_name="Telegram Android",
        app_version="10.14.5",
        date_created=datetime(2026, 1, 1, tzinfo=timezone.utc),
        date_active=datetime(2026, 9, 1, tzinfo=timezone.utc),
        ip="203.0.113.7",
        country="Iran",
        region="Tehran",
        current=current,
        encrypted_requests_disabled=encrypted_requests_disabled,
    )


class _Client:
    """Records every request; answers GetAuthorizations from a fixed list."""

    def __init__(self, authorizations=(), change_answer=True):
        self.sent = []
        self.authorizations = list(authorizations)
        self.change_answer = change_answer

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(request, functions.account.GetAuthorizationsRequest):
            return types.account.Authorizations(
                authorization_ttl_days=180, authorizations=self.authorizations
            )
        if isinstance(request, CHANGE):
            return self.change_answer
        return True

    def sent_of(self, kind):
        return next((r for r in self.sent if isinstance(r, kind)), None)


@pytest.fixture
def _wire(monkeypatch):
    def wire(client):
        monkeypatch.setattr(mod, "get_client", lambda account=None: client)

        async def _connected(_client):
            return None

        monkeypatch.setattr(mod, "ensure_connected", _connected)
        return client

    return wire


async def _listed(client):
    return json.loads(await mod.list_authorizations(account="a"))


# --------------------------------------------------------------------------
# The polarity, from both ends
# --------------------------------------------------------------------------


@pytest.mark.asyncio
@pytest.mark.parametrize("disabled, accepted", [(True, False), (False, True)])
async def test_the_list_reports_the_switch_the_owner_sees_not_the_field_telegram_sends(
    _wire, disabled, accepted
):
    """`encrypted_requests_disabled=True` IS `accept_secret_chats=False`. Reported
    the other way round, the owner reads "on" off a device that refuses them."""
    client = _wire(_Client([_auth(encrypted_requests_disabled=disabled)]))

    record = (await _listed(client))["results"][0]

    assert record["accept_secret_chats"] is accepted


@pytest.mark.asyncio
@pytest.mark.parametrize("accept, disabled", [(True, False), (False, True)])
async def test_the_switch_goes_out_inverted_because_the_wire_field_is_negated(
    _wire, accept, disabled
):
    """The one assertion this whole file exists for, on the value that reaches
    Telegram rather than on the sentence that comes back."""
    client = _wire(_Client([_auth(hash=111)]))

    await mod.set_authorization_secret_chats(hash=111, accept_secret_chats=accept, account="a")

    assert client.sent_of(CHANGE).encrypted_requests_disabled is disabled


@pytest.mark.asyncio
@pytest.mark.parametrize("accept, word", [(True, "ON"), (False, "OFF")])
async def test_the_reply_names_the_state_that_was_actually_sent(_wire, accept, word):
    """Argument, wire value and sentence have to agree, or the owner is told one
    thing while the device does another."""
    client = _wire(_Client([_auth(hash=111)]))

    said = await mod.set_authorization_secret_chats(
        hash=111, accept_secret_chats=accept, account="a"
    )

    assert f"is now {word}" in said
    assert client.sent_of(CHANGE).encrypted_requests_disabled is (not accept)


@pytest.mark.asyncio
async def test_changing_one_switch_leaves_the_other_two_unset(_wire):
    """`changeAuthorizationSettings` carries three independent flags. Sending a
    value for a flag the caller never mentioned would silently reset it — an
    omitted flag is what tells Telegram to leave it alone."""
    client = _wire(_Client([_auth(hash=111)]))

    await mod.set_authorization_secret_chats(hash=111, accept_secret_chats=False, account="a")

    request = client.sent_of(CHANGE)
    assert request.confirmed is None
    assert request.call_requests_disabled is None


@pytest.mark.asyncio
async def test_a_change_telegram_did_not_confirm_is_not_reported_as_done(_wire):
    """The call not raising is not the switch having moved. Telegram answers
    `Bool`, and a false answer means it did not apply."""
    _wire(_Client([_auth(hash=111)], change_answer=False))

    said = await mod.set_authorization_secret_chats(
        hash=111, accept_secret_chats=True, account="a"
    )

    assert "is now ON" not in said
    assert "did not confirm" in said


# --------------------------------------------------------------------------
# The logout, which cannot be undone
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_the_current_authorization_is_refused_unless_the_caller_names_it(_wire):
    """Ending this connection's own authorization signs the server out of the
    account. It is a legitimate thing to ask for and never a default."""
    client = _wire(_Client([_auth(hash=0, current=True)]))

    said = await mod.terminate_authorization(hash=0, account="a")

    assert client.sent_of(RESET) is None, "the current device was signed out anyway"
    assert "include_current" in said


@pytest.mark.asyncio
async def test_the_current_authorization_goes_when_the_caller_does_name_it(_wire):
    """The guard is a question, not a wall."""
    client = _wire(_Client([_auth(hash=0, current=True)]))

    await mod.terminate_authorization(hash=0, include_current=True, account="a")

    assert client.sent_of(RESET).hash == 0


@pytest.mark.asyncio
async def test_a_hash_this_account_does_not_have_signs_nothing_out(_wire):
    """A hash changes every time a device signs in again, so a stale one is the
    ordinary mistake. Refused before the request, because the alternative is
    Telegram answering about some other device."""
    client = _wire(_Client([_auth(hash=111)]))

    said = await mod.terminate_authorization(hash=222, account="a")

    assert client.sent_of(RESET) is None
    assert "nothing was signed out" in said


@pytest.mark.asyncio
async def test_only_the_named_authorization_is_reset(_wire):
    """Three devices logged in, one named, one request."""
    client = _wire(_Client([_auth(hash=111), _auth(hash=222), _auth(hash=333)]))

    await mod.terminate_authorization(hash=222, account="a")

    assert [r.hash for r in client.sent if isinstance(r, RESET)] == [222]


def test_nothing_here_can_sign_every_device_out_at_once():
    """`auth.resetAuthorizations` ends every other login in one call. It is the
    obvious convenience to reach for and the one that cannot be taken back, so
    the absence is asserted rather than left to reviewer memory."""
    source = Path(mod.__file__).read_text(encoding="utf-8")

    assert "ResetAuthorizationsRequest" not in source


# --------------------------------------------------------------------------
# What leaves the tool
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_the_hash_never_reaches_the_error_text(_wire):
    """The hash is the handle that ends a login. It belongs in the answer the
    owner acts on and nowhere else — not in an error, not in a log line."""

    class _Boom:
        async def __call__(self, request):
            raise RuntimeError("telegram is unhappy")

    _wire(_Boom())

    said = await mod.terminate_authorization(hash=9876543210123, account="a")

    assert "9876543210123" not in said


@pytest.mark.asyncio
async def test_device_text_is_sanitized_before_it_reaches_the_caller(_wire):
    """`device_model` is whatever the client that signed in called itself."""
    client = _wire(_Client([_auth(device_model="Pixel‮ 8\nsigned in by admin")]))

    record = (await _listed(client))["results"][0]

    assert "‮" not in record["device_model"]
    assert "\n" not in record["device_model"]


@pytest.mark.asyncio
async def test_the_list_carries_the_hash_and_telegrams_own_inactivity_window(_wire):
    """The hash is the whole point of the list: without it the other two tools
    have nothing to name. `ttl_days` is Telegram's own auto-logout setting."""
    client = _wire(_Client([_auth(hash=111)]))

    answer = await _listed(client)

    assert answer["results"][0]["hash"] == 111
    assert answer["ttl_days"] == 180
