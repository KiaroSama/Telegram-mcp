"""Promoting and editing admins, one tool per chat type (spec 033).

The bug these exist for: an agent sent every right, group-only ones included, to a CHANNEL;
Telegram answered RIGHT_FORBIDDEN, the agent guessed which right to drop, and dropped
welcome messages. Each chat type now has its own tool offering only that type's rights,
the chat's REAL type is checked before any request, and "full admin" is every right of the
type except "Remain anonymous".

The client is a fake that records the requests it was handed, so the assertions are about
what reaches Telegram, not about a string that could be right for the wrong reason.
"""

import asyncio
import inspect
from types import SimpleNamespace

import pytest
from telethon.tl import types
from telethon.tl.functions import communities as cf

from telegram_mcp import admin_rights_sets as sets
from telegram_mcp import approval_details
from telegram_mcp.safeguard import channels as ac
from telegram_mcp.safeguard import grants, middleware, policy
from telegram_mcp.tools import admin_rights_by_type as mod

PHOTO = types.ChatPhotoEmpty()
CHANNEL = types.Channel(id=10, title="News", photo=PHOTO, date=None, broadcast=True)
GROUP = types.Channel(id=11, title="Chatters", photo=PHOTO, date=None, megagroup=True)
FORUM = types.Channel(id=12, title="Topics", photo=PHOTO, date=None, megagroup=True, forum=True)
BASIC = types.Chat(id=13, title="Old", photo=PHOTO, participants_count=3, date=None, version=1)
COMMUNITY = types.Community(id=900, title="Hub", photo=PHOTO, date=None, access_hash=77)
PERSON = types.User(id=42, first_name="Ann")
BOT = types.User(id=43, first_name="Helper", bot=True)

ENTITIES = {
    "news": CHANNEL,
    "chatters": GROUP,
    "topics": FORUM,
    "old": BASIC,
    "hub": COMMUNITY,
    "ann": PERSON,
    "helper": BOT,
}


class _Client:
    """Records every request; answers the participant read-back from what was sent."""

    def __init__(self, drops=(), fails=None):
        self.sent = []
        self.drops = set(drops)
        self.fails = fails or {}
        self._rights = None

    async def __call__(self, request):
        name = type(request).__name__
        if isinstance(request, cf.GetJoinedCommunitiesRequest):
            return types.messages.Chats(chats=[COMMUNITY])
        self.sent.append(request)
        if name in self.fails:
            raise self.fails[name]
        if name == "EditAdminRequest":
            self._rights = request.admin_rights
            return SimpleNamespace(updates=[])
        if name == "GetParticipantRequest":
            applied = types.ChatAdminRights(
                **{
                    field: False if field in self.drops else bool(value)
                    for field, value in self._rights.to_dict().items()
                    if field != "_"
                }
            )
            return SimpleNamespace(participant=SimpleNamespace(admin_rights=applied))
        raise AssertionError(f"unexpected request {name}")

    def requests(self, name):
        return [r for r in self.sent if type(r).__name__ == name]


@pytest.fixture
def client(wire_client):
    async def _resolve(value, cl=None, account=None):
        return ENTITIES[value]

    return wire_client(mod, _Client(), resolve=_resolve)


def _granted(request):
    rights = request.admin_rights.to_dict()
    return {name for name, on in rights.items() if on is True}


def _on(kind, **overrides):
    wanted = {**sets.full_admin(kind, is_forum=True), **overrides}
    return {name for name, on in wanted.items() if on}


# --- the tool surface --------------------------------------------------------------


def test_the_old_tools_are_gone_and_the_six_new_ones_exist():
    from telegram_mcp.tools import admin_rights

    assert not hasattr(admin_rights, "promote_admin")
    assert not hasattr(admin_rights, "edit_admin_rights")
    for family in ("promote_admin", "edit_admin_rights"):
        for kind in sets.KINDS:
            assert inspect.iscoroutinefunction(getattr(mod, f"{family}_{kind}")), (family, kind)


@pytest.mark.parametrize("family", ["promote_admin", "edit_admin_rights"])
@pytest.mark.parametrize("kind", sets.KINDS)
def test_each_tool_offers_only_the_rights_of_its_chat_type(family, kind):
    params = set(inspect.signature(getattr(mod, f"{family}_{kind}")).parameters)
    offered = params & set(sets.telethon_fields())
    assert offered == set(sets.fields(kind, is_forum=True))


def test_a_channel_tool_cannot_even_be_asked_for_a_group_right():
    assert "pin_messages" not in inspect.signature(mod.promote_admin_channel).parameters
    assert "manage_welcome_messages" in inspect.signature(mod.promote_admin_channel).parameters


# --- wrong chat type is refused before any request -----------------------------------


@pytest.mark.asyncio
async def test_the_group_tool_on_a_channel_names_the_channel_tool(client):
    answer = await mod.promote_admin_group("news", "ann", account="a")

    assert "promote_admin_channel" in answer
    assert client.sent == []


@pytest.mark.asyncio
async def test_the_channel_tool_on_a_group_names_the_group_tool(client):
    answer = await mod.edit_admin_rights_channel(
        "chatters", "ann", post_messages=True, account="a"
    )

    assert "edit_admin_rights_group" in answer
    assert client.sent == []


@pytest.mark.asyncio
async def test_a_group_tool_pointed_at_a_community_names_the_community_tool(client):
    answer = await mod.promote_admin_group("hub", "ann", account="a")

    assert "promote_admin_community" in answer
    assert client.sent == []


@pytest.mark.asyncio
async def test_a_basic_group_is_told_to_upgrade_first(client):
    answer = await mod.promote_admin_group("old", "ann", account="a")

    assert "upgrade_to_supergroup" in answer
    assert client.sent == []


@pytest.mark.asyncio
async def test_a_community_tool_refuses_an_id_that_is_not_a_community(client):
    answer = await mod.promote_admin_community("555", "ann", account="a")

    assert "list_my_communities" in answer
    assert client.requests("EditAdminRequest") == []


# --- full admin --------------------------------------------------------------------


@pytest.mark.asyncio
async def test_promoting_a_channel_admin_with_nothing_named_grants_the_full_set(client):
    answer = await mod.promote_admin_channel("news", "ann", account="a")

    (request,) = client.requests("EditAdminRequest")
    assert _granted(request) == _on("channel")
    assert "manage_welcome_messages" in _granted(request)
    assert "anonymous" not in _granted(request)
    assert answer.startswith("Successfully promoted")
    assert "permissions: Change channel info | Manage Welcome Messages" in answer


@pytest.mark.asyncio
async def test_promoting_a_group_admin_grants_welcome_messages_and_add_admins(client):
    await mod.promote_admin_group("chatters", "ann", account="a")

    (request,) = client.requests("EditAdminRequest")
    granted = _granted(request)
    assert {"manage_welcome_messages", "add_admins", "pin_messages", "manage_ranks"} <= granted
    assert "anonymous" not in granted
    assert "manage_topics" not in granted, "a plain group has no topics"


@pytest.mark.asyncio
async def test_a_forum_group_admin_also_gets_topics(client):
    await mod.promote_admin_group("topics", "ann", account="a")

    (request,) = client.requests("EditAdminRequest")
    assert "manage_topics" in _granted(request)


@pytest.mark.asyncio
async def test_a_community_admin_has_exactly_its_four_rights(client):
    await mod.promote_admin_community("900", "ann", account="a")

    (request,) = client.requests("EditAdminRequest")
    assert _granted(request) == {"change_info", "manage_linked_peers", "ban_users", "add_admins"}
    assert isinstance(request.channel, types.InputChannel)
    assert request.channel.channel_id == 900


@pytest.mark.asyncio
async def test_declining_one_right_keeps_the_rest_of_the_full_set(client):
    await mod.promote_admin_group("chatters", "ann", ban_users=False, anonymous=True, account="a")

    (request,) = client.requests("EditAdminRequest")
    assert _granted(request) == _on("group", ban_users=False, anonymous=True) - {"manage_topics"}


@pytest.mark.asyncio
async def test_topics_outside_a_forum_are_refused_before_any_request(client):
    answer = await mod.promote_admin_group("chatters", "ann", manage_topics=True, account="a")

    assert "forum" in answer
    assert client.sent == []


# --- edit: exactly what was named --------------------------------------------------


@pytest.mark.asyncio
async def test_editing_grants_exactly_the_rights_named(client):
    await mod.edit_admin_rights_channel(
        "news", "ann", post_messages=True, delete_stories=True, rank="Editor", account="a"
    )

    (request,) = client.requests("EditAdminRequest")
    assert _granted(request) == {"post_messages", "delete_stories"}
    assert request.rank == "Editor"


@pytest.mark.asyncio
async def test_editing_with_no_right_named_is_refused_and_points_at_demote(client):
    answer = await mod.edit_admin_rights_group("chatters", "ann", account="a")

    assert "demote_admin" in answer
    assert client.sent == []


# --- what Telegram dropped is reported ---------------------------------------------


@pytest.mark.asyncio
async def test_a_right_telegram_read_back_as_off_is_reported_declined(wire_client):
    async def _resolve(value, cl=None, account=None):
        return ENTITIES[value]

    wire_client(mod, _Client(drops={"manage_direct_messages"}), resolve=_resolve)

    answer = await mod.promote_admin_channel("news", "ann", account="a")

    assert "Telegram declined: manage_direct_messages" in answer
    assert "change_info" not in answer.split("Telegram declined:")[1]


@pytest.mark.asyncio
async def test_a_failed_read_back_does_not_turn_the_grant_into_an_error(wire_client):
    async def _resolve(value, cl=None, account=None):
        return ENTITIES[value]

    wire_client(
        mod, _Client(fails={"GetParticipantRequest": ConnectionError("no")}), resolve=_resolve
    )

    answer = await mod.promote_admin_channel("news", "ann", account="a")

    assert answer.startswith("Successfully promoted")
    assert "declined" not in answer


@pytest.mark.asyncio
async def test_a_telegram_refusal_is_a_sentence_not_a_traceback(wire_client):
    import telethon.errors.rpcerrorlist as rpc

    async def _resolve(value, cl=None, account=None):
        return ENTITIES[value]

    wire_client(
        mod,
        _Client(fails={"EditAdminRequest": rpc.RightForbiddenError(request=None)}),
        resolve=_resolve,
    )

    answer = await mod.promote_admin_channel("news", "ann", account="a")

    assert answer.startswith("Error") and "not allowed" in answer


# --- the approval shows exactly what is granted ------------------------------------


def _detail(tool, **arguments):
    return asyncio.run(approval_details.detail_for(tool, {"account": "a", **arguments}))


def test_the_approval_line_of_a_full_channel_promotion(client):
    assert _detail("promote_admin_channel", channel_id="news", user_id="ann") == (
        "permissions: Change channel info | Manage Welcome Messages"
        " | Manage messages: Post messages, Edit messages of others, Delete messages of others"
        " | Manage stories: Post stories, Edit stories of others, Delete stories of others"
        " | Add members | Manage live streams | Manage direct messages | Add new admins"
        " | Ban users"
    )


def test_the_approval_line_says_send_for_a_bot_and_add_members_when_members_may_not_add(client):
    closed = types.Channel(
        id=14,
        title="Closed",
        photo=PHOTO,
        date=None,
        megagroup=True,
        default_banned_rights=types.ChatBannedRights(until_date=None, invite_users=True),
    )
    ENTITIES["closed"] = closed
    try:
        line = _detail("promote_admin_group", group_id="closed", user_id="helper")
    finally:
        del ENTITIES["closed"]

    assert "Send Welcome Messages" in line
    assert "Add members" in line and "Invite users via link" not in line


def test_the_approval_line_of_an_edit_lists_only_what_is_named(client):
    line = _detail("edit_admin_rights_channel", channel_id="news", user_id="ann", ban_users=True)

    assert line == "permissions: Ban users"


def test_the_approval_line_of_a_forum_promotion_includes_topics(client):
    assert "Manage topics" in _detail("promote_admin_group", group_id="topics", user_id="ann")
    assert "Manage topics" not in _detail(
        "promote_admin_group", group_id="chatters", user_id="ann"
    )


def test_the_approval_line_of_a_wrong_chat_type_says_the_call_will_be_refused(client):
    line = _detail("promote_admin_group", group_id="news", user_id="ann")

    assert line.startswith("permissions: none") and "promote_admin_channel" in line


def test_a_detail_that_cannot_be_computed_still_lets_the_owner_be_asked(monkeypatch):
    async def _broken(arguments):
        raise RuntimeError("boom")

    monkeypatch.setitem(approval_details._DETAILS, "x_tool", _broken)

    assert asyncio.run(approval_details.detail_for("x_tool", {})) == (
        "details: could not be listed"
    )
    assert asyncio.run(approval_details.detail_for("no_such_tool", {})) == ""


def test_the_detail_line_follows_the_tool_line_in_every_channel():
    request = ac.new_request(
        "promote_admin_channel",
        "a",
        "News",
        "promote admin channel in News",
        ["gated"],
        detail="permissions: Ban users",
    )

    lines = request.text().splitlines()
    assert lines[lines.index("Tool: promote_admin_channel") + 1] == "permissions: Ban users"
    assert "permissions: Ban users" in request.html()
    assert "Tool: <code>promote_admin_channel</code>\npermissions: Ban users\n" in request.html()


def test_a_request_without_a_detail_reads_as_before():
    request = ac.new_request("delete_message", "a", "News", "deletes 1 message", ["gated"])

    assert [line.split(":")[0] for line in request.text().splitlines()] == [
        "Account",
        "Action",
        "Tool",
        "Chat",
        "Why asked",
    ]


def test_the_middleware_puts_the_tools_detail_into_the_approval(monkeypatch, tmp_path):
    monkeypatch.setitem(
        approval_details._DETAILS, "delete_message", lambda arguments: "permissions: Ban users"
    )
    monkeypatch.setattr(grants, "grants_path", lambda: tmp_path / "grants.json")
    grants.reset_cache()
    seen = []

    class _Channel:
        kind = "dialog"

        def available(self):
            return True

        async def ask(self, request, timeout):
            seen.append(request)
            return "declined"

    async def _first(account, chat):
        return False

    async def _identity(account):
        return "a | 1 | @a"

    async def _label(account, chat):
        return "News"

    async def _warm():
        return None

    async def _sealed(account, arguments):
        return False

    guard = middleware.Safeguard(
        hints={"delete_message": (False, True)}.get,
        channels=lambda ctx, account: [_Channel()],
        first_message=_first,
        ghost_on=lambda account, chat: False,
        approval_chats=lambda: frozenset(),
        account_of=lambda arguments: "a",
        after=lambda account: None,
        identity=_identity,
        chat_label=_label,
        sealed_target=_sealed,
        warm=_warm,
        timeout=5,
    )
    ctx = SimpleNamespace(
        method="tools/call",
        params={"name": "delete_message", "arguments": {"chat_id": 5, "message_ids": [1]}},
        request_id=1,
        session=None,
    )

    async def _next(ctx):
        return "RESULT"

    asyncio.run(guard(ctx, _next))

    assert seen and seen[0].detail == "permissions: Ban users"


def test_the_six_tools_ask_the_owner_and_the_old_two_no_longer_do():
    for family in ("promote_admin", "edit_admin_rights"):
        for kind in sets.KINDS:
            assert f"{family}_{kind}" in policy.GATED
    assert "promote_admin" not in policy.GATED
    assert "edit_admin_rights" not in policy.GATED


# --- review H1/M1/L1: what the owner approves is what runs ------------------------------

_SIX = [
    f"{family}_{kind}"
    for family in ("promote_admin", "edit_admin_rights")
    for kind in ("group", "channel", "community")
]


@pytest.mark.parametrize("tool", _SIX)
@pytest.mark.parametrize("value", ["true", 1, "yes"])
def test_a_right_that_is_not_a_real_boolean_is_refused_before_the_owner_is_asked(tool, value):
    """The approval line counts only real booleans; the tool would coerce "true"/1 to True,
    so without this an approval could show fewer rights than are granted."""
    from telegram_mcp import preflight

    assert preflight.RULES[tool]({"user_id": 1, "add_admins": value, "change_info": True})
    assert preflight.RULES[tool]({"user_id": 1, "change_info": True, "rank": "x"}) is None


@pytest.mark.asyncio
async def test_a_failed_lookup_still_lists_the_rights(monkeypatch):
    """Review M1: the owner never approves blind; without the chat the line says so."""
    from telegram_mcp import approval_details
    from telegram_mcp.tools import admin_rights_by_type as by_type

    def _offline(account=None):
        raise ConnectionError("no network")

    monkeypatch.setattr(by_type, "get_client", _offline)
    line = await approval_details.detail_for(
        "promote_admin_group", {"group_id": -100, "user_id": 5}
    )

    assert line.startswith("permissions: ") and "Add new admins" in line
    assert "chat not checked" in line


@pytest.mark.asyncio
async def test_a_detail_line_is_one_bounded_line():
    from telegram_mcp import approval_details

    approval_details.register("_probe_tool", lambda arguments: "a\nb" + "x" * 2000)
    line = await approval_details.detail_for("_probe_tool", {})

    assert "\n" not in line and len(line) <= 500
