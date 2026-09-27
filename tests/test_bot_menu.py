"""The approval bot's `/` menu (spec 016), without Telegram.

What these pin: the menu is published on every login; only the owner is answered,
by command or by button; a remove button names its grant by content, so a list
that changed since it was shown cannot remove the wrong one; reset asks first and
leaves the folders alone; and nothing here can grant anything.
"""

import pytest
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.safeguard import bot_menu, channels, grants, state_files

OWNER, STRANGER = 111, 999


@pytest.fixture(autouse=True)
def state(tmp_path, monkeypatch):
    monkeypatch.setattr(state_files, "grants_path", lambda: tmp_path / "grants.json")
    grants.reset_cache()

    async def _accounts():
        return [{"label": "main", "id": 5, "username": "me", "connected": True}]

    monkeypatch.setattr(bot_menu, "_accounts", _accounts)
    monkeypatch.setattr(bot_menu, "_ghost", lambda label: True)
    yield
    grants.reset_cache()


def _datas(reply):
    return [button.type.data.decode() for row in reply.buttons or [] for button in row]


class _Bot:
    def __init__(self):
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        return True


@pytest.mark.asyncio
async def test_the_menu_is_published_for_everyone():
    bot = _Bot()
    await bot_menu.install(bot)
    (request,) = bot.sent
    assert isinstance(request, functions.bots.SetBotCommandsRequest)
    assert isinstance(request.scope, tl.BotCommandScopeDefault)
    names = [c.command for c in request.commands]
    assert names == [name for name, _ in bot_menu.COMMANDS]
    assert {"always", "reset_always", "accounts", "folders", "pending", "status", "help"} <= set(
        names
    )


@pytest.mark.asyncio
async def test_a_stranger_gets_nothing():
    grants.add("main", "send_message", "-100")
    for text in ("/always", "/reset_always", "/accounts", "/status"):
        assert await bot_menu.answer_command(text, STRANGER, {OWNER}) is None
    data = "sgm:reset:yes"
    assert await bot_menu.answer_button(data.encode(), STRANGER, {OWNER}) is None
    assert grants.list_all()


@pytest.mark.asyncio
async def test_always_lists_each_grant_with_its_own_remove_button():
    grants.add("main", "send_message", "-100")
    grants.add("main", "ban_user", "-200")
    reply = await bot_menu.answer_command("/always", OWNER, {OWNER})
    assert "send_message" in reply.text and "ban_user" in reply.text
    assert len([d for d in _datas(reply) if d.startswith("sgm:rv:")]) == 2


@pytest.mark.asyncio
async def test_a_remove_button_removes_that_grant_only():
    grants.add("main", "send_message", "-100")
    grants.add("main", "ban_user", "-200")
    reply = await bot_menu.answer_command("/always", OWNER, {OWNER})
    first = next(d for d in _datas(reply) if d.startswith("sgm:rv:"))
    await bot_menu.answer_button(first.encode(), OWNER, {OWNER})
    assert len(grants.list_all()) == 1
    stale = await bot_menu.answer_button(first.encode(), OWNER, {OWNER})
    assert "already" in stale.toast.lower() and len(grants.list_all()) == 1


@pytest.mark.asyncio
async def test_reset_asks_first_and_keeps_folders(tmp_path):
    grants.add("main", "send_message", "-100")
    grants.add_folder(tmp_path)
    reply = await bot_menu.answer_command("/reset_always", OWNER, {OWNER})
    assert "sgm:reset:yes" in _datas(reply) and grants.list_all()
    await bot_menu.answer_button(b"sgm:reset:no", OWNER, {OWNER})
    assert grants.list_all()
    await bot_menu.answer_button(b"sgm:reset:yes", OWNER, {OWNER})
    assert grants.list_all() == [] and grants.list_folders()


@pytest.mark.asyncio
async def test_folders_are_listed_and_removed_one_by_one(tmp_path):
    grants.add_folder(tmp_path)
    reply = await bot_menu.answer_command("/folders", OWNER, {OWNER})
    (remove,) = [d for d in _datas(reply) if d.startswith("sgm:rf:")]
    await bot_menu.answer_button(remove.encode(), OWNER, {OWNER})
    assert grants.list_folders() == []


@pytest.mark.asyncio
async def test_accounts_status_pending_and_help_answer():
    accounts = await bot_menu.answer_command("/accounts", OWNER, {OWNER})
    assert "main" in accounts.text and "@me" in accounts.text
    status = await bot_menu.answer_command("/status", OWNER, {OWNER})
    assert "Always approvals" in status.text and "Ghost" in status.text
    assert "No approval" in (await bot_menu.answer_command("/pending", OWNER, {OWNER})).text
    for text in ("/help", "/start"):
        help_text = (await bot_menu.answer_command(text, OWNER, {OWNER})).text
        assert all(f"/{name}" in help_text for name, _ in bot_menu.COMMANDS)


@pytest.mark.asyncio
async def test_an_open_request_is_listed_under_pending():
    request = channels.new_request("send_message", "main", "-100", "send message", ["tainted"])
    channels._open[request.code] = request
    try:
        reply = await bot_menu.answer_command("/pending", OWNER, {OWNER})
        assert "send_message" in reply.text and "-100" in reply.text
    finally:
        channels._open.pop(request.code, None)


@pytest.mark.asyncio
async def test_a_command_with_the_bot_name_and_unknown_commands():
    assert (await bot_menu.answer_command("/status@SomeBot", OWNER, {OWNER})) is not None
    assert "/help" in (await bot_menu.answer_command("/nope", OWNER, {OWNER})).text
    assert await bot_menu.answer_command("hello", OWNER, {OWNER}) is None


@pytest.mark.asyncio
async def test_bypass_is_switched_only_by_the_owner_with_three_durations(tmp_path, monkeypatch):
    from telegram_mcp.safeguard import bypass

    monkeypatch.setattr(bypass, "bypass_path", lambda: tmp_path / "bypass.json")
    reply = await bot_menu.answer_command("/bypass", OWNER, {OWNER})
    assert {"sgm:bp:3600", "sgm:bp:86400", "sgm:bp:inf"} <= set(_datas(reply))
    assert await bot_menu.answer_button(b"sgm:bp:inf", STRANGER, {OWNER}) is None
    assert not bypass.active()
    await bot_menu.answer_button(b"sgm:bp:3600", OWNER, {OWNER})
    assert bypass.active() and bypass.until() is not None
    await bot_menu.answer_button(b"sgm:bp:inf", OWNER, {OWNER})
    assert bypass.active() and bypass.until() is None
    status = await bot_menu.answer_command("/status", OWNER, {OWNER})
    assert "Bypass: ON" in status.text
    assert "sgm:bp:off" in _datas(await bot_menu.answer_command("/bypass", OWNER, {OWNER}))
    await bot_menu.answer_button(b"sgm:bp:off", OWNER, {OWNER})
    assert not bypass.active()
