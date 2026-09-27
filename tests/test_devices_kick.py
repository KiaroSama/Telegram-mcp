"""Kick, and the two device switches added in spec 015, on fake clients.

A kick is a ban lifted at once: the member leaves, is not on the removed list,
and can come back. What these pin is the ORDER (ban, then unban) and the honest
answer when only the first half worked. The device switches are pinned on the
value actually SENT, because the wire fields are negated ("calls disabled").
"""

from datetime import datetime, timezone

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import authorizations as auth_mod
from telegram_mcp.tools import moderation as mod

CHANGE = functions.account.ChangeAuthorizationSettingsRequest
SUPERGROUP = types.Channel(
    id=777, title="Group", photo=types.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=7
)
BASIC = types.Chat(
    id=555, title="Small", photo=types.ChatPhotoEmpty(), participants_count=3, date=None, version=1
)
USER = types.User(id=42, first_name="Sara", access_hash=4)


class _Client:
    def __init__(self, fail_on=None):
        self.sent = []
        self.fail_on = fail_on

    async def __call__(self, request):
        self.sent.append(request)
        if self.fail_on and self.fail_on(request):
            raise RuntimeError("CHAT_ADMIN_REQUIRED")
        return types.Updates(updates=[], users=[], chats=[], date=None, seq=0)


@pytest.fixture
def kick(monkeypatch):
    def use(client, chat):
        async def _resolve(value, cl=None, account=None):
            return chat if value == "chat" else USER

        monkeypatch.setattr(mod, "get_client", lambda account=None: client)
        monkeypatch.setattr(mod, "resolve_entity", _resolve)
        return client

    return use


@pytest.mark.asyncio
async def test_a_kick_bans_then_lifts_the_ban(kick):
    client = kick(_Client(), SUPERGROUP)
    text = await mod.kick_user("chat", "user", account="a")
    ban, unban = client.sent
    assert isinstance(ban, functions.channels.EditBannedRequest)
    assert ban.banned_rights.view_messages is True
    assert isinstance(unban, functions.channels.EditBannedRequest)
    assert not unban.banned_rights.view_messages and not unban.banned_rights.send_messages
    assert "can rejoin" in text.lower()


@pytest.mark.asyncio
async def test_a_basic_group_removes_the_member_directly(kick):
    client = kick(_Client(), BASIC)
    await mod.kick_user("chat", "user", account="a")
    (request,) = client.sent
    assert isinstance(request, functions.messages.DeleteChatUserRequest)
    assert request.chat_id == 555


@pytest.mark.asyncio
async def test_a_failed_unban_says_the_user_is_still_banned(kick):
    calls = {"n": 0}

    def second_call(request):
        calls["n"] += 1
        return calls["n"] == 2

    kick(_Client(fail_on=second_call), SUPERGROUP)
    text = await mod.kick_user("chat", "user", account="a")
    assert "still banned" in text.lower()


@pytest.mark.asyncio
async def test_a_failed_ban_sends_no_unban(kick):
    client = kick(_Client(fail_on=lambda request: True), SUPERGROUP)
    await mod.kick_user("chat", "user", account="a")
    assert len(client.sent) == 1


# --- devices -----------------------------------------------------------------------


def _auth(hash, *, secret_off=False, calls_off=False, model="Pixel"):
    return types.Authorization(
        hash=hash,
        device_model=model,
        platform="Android",
        system_version="14",
        api_id=6,
        app_name="Telegram Android",
        app_version="12",
        date_created=datetime(2026, 1, 1, tzinfo=timezone.utc),
        date_active=datetime(2026, 9, 1, tzinfo=timezone.utc),
        ip="203.0.113.7",
        country="X",
        region="Y",
        encrypted_requests_disabled=secret_off,
        call_requests_disabled=calls_off,
    )


class _Devices:
    def __init__(self, authorizations):
        self.authorizations = authorizations
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(request, functions.account.GetAuthorizationsRequest):
            return types.account.Authorizations(
                authorization_ttl_days=180, authorizations=self.authorizations
            )
        return True

    def changes(self):
        return [r for r in self.sent if isinstance(r, CHANGE)]


@pytest.fixture
def devices(monkeypatch):
    def use(client):
        async def _connected(_client):
            return None

        monkeypatch.setattr(auth_mod, "get_client", lambda account=None: client)
        monkeypatch.setattr(auth_mod, "ensure_connected", _connected)
        return client

    return use


@pytest.mark.asyncio
@pytest.mark.parametrize("accept, disabled", [(False, True), (True, False)])
async def test_calls_are_switched_per_device_with_the_negated_field(devices, accept, disabled):
    client = devices(_Devices([_auth(111)]))
    await auth_mod.set_authorization_calls(hash=111, accept_calls=accept, account="a")
    (change,) = client.changes()
    assert change.hash == 111 and change.call_requests_disabled is disabled
    assert change.encrypted_requests_disabled is None, "the secret-chat switch was touched"


@pytest.mark.asyncio
async def test_an_unknown_device_changes_nothing(devices):
    client = devices(_Devices([_auth(111)]))
    await auth_mod.set_authorization_calls(hash=999, accept_calls=False, account="a")
    await auth_mod.set_secret_chats_only_device(hash=999, account="a")
    assert client.changes() == []


@pytest.mark.asyncio
async def test_secret_chats_stay_on_only_for_the_chosen_device(devices):
    client = devices(_Devices([_auth(111), _auth(222, secret_off=True, model="Desk"), _auth(333)]))
    text = await auth_mod.set_secret_chats_only_device(hash=222, account="a")
    sent = {c.hash: c.encrypted_requests_disabled for c in client.changes()}
    assert sent == {222: False, 111: True, 333: True}
    assert all(c.call_requests_disabled is None for c in client.changes())
    assert "Desk" in text


@pytest.mark.asyncio
async def test_devices_already_right_are_not_touched(devices):
    client = devices(_Devices([_auth(111, secret_off=True), _auth(222)]))
    await auth_mod.set_secret_chats_only_device(hash=222, account="a")
    assert client.changes() == []
