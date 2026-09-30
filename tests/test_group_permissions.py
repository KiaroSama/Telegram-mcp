"""A group's member permissions and member exceptions, as Telegram Desktop 7.2.10 sets them.

Desktop's Permissions screen (edit_peer_permissions_box.cpp) lists Send messages, the ten
"Send media" kinds, Add members, Create topics (forums), Pin messages, Edit own tags and
Change group info, then Charge Stars for Messages, Do not restrict boosters and Slow mode.
What these pin: each item reaches exactly its ChatBannedRights fields, an item not passed
keeps its current value, Desktop's couplings hold (stickers/GIFs/games/inline move together,
no embed links without send messages), invalid values are refused before any request, and a
member exception starts from what the group already restricts.
"""

import pytest
import telethon.errors.rpcerrorlist as rpc
from telethon.tl import functions, types

from telegram_mcp.tools import group_permissions as mod

MEDIA_FIELDS = {
    "send_photos",
    "send_videos",
    "send_roundvideos",
    "send_audios",
    "send_voices",
    "send_docs",
    "send_stickers",
    "send_gifs",
    "send_games",
    "send_inline",
    "embed_links",
    "send_polls",
    "send_reactions",
}


def _group(rights=None, **extra):
    return types.Channel(
        id=777,
        title="Test Group",
        photo=types.ChatPhotoEmpty(),
        date=None,
        megagroup=True,
        access_hash=7,
        default_banned_rights=rights,
        **extra,
    )


NEWS = types.Channel(id=555, title="News", photo=types.ChatPhotoEmpty(), date=None, broadcast=True)
BASIC = types.Chat(
    id=9,
    title="Small",
    photo=types.ChatPhotoEmpty(),
    participants_count=3,
    date=None,
    version=1,
    default_banned_rights=None,
)
USER = types.User(id=42, first_name="Sara", access_hash=4)


class FakeClient:
    def __init__(self, participant=None, fail_on=None):
        self.sent = []
        self.participant = participant or types.ChannelParticipant(user_id=42, date=None)
        self.fail_on = fail_on

    async def __call__(self, request):
        if self.fail_on and isinstance(request, self.fail_on):
            raise rpc.ChatAdminRequiredError(request=request)
        if isinstance(request, functions.channels.GetParticipantRequest):
            return types.channels.ChannelParticipant(
                participant=self.participant, chats=[], users=[USER]
            )
        self.sent.append(request)
        return types.Updates(updates=[], users=[], chats=[], date=None, seq=0)


@pytest.fixture
def wire(wire_client):
    def _wire(entity=None, **client_kwargs):
        client = FakeClient(**client_kwargs)
        chat = entity if entity is not None else _group()

        async def _resolve(value, cl=None, account=None):
            return USER if value == 42 else chat

        return wire_client(mod, client, resolve=_resolve)

    return _wire


def _banned(request):
    rights = request.banned_rights
    return {k for k, v in rights.to_dict().items() if v is True}


# --- set_group_permissions -------------------------------------------------------------


@pytest.mark.asyncio
async def test_an_item_not_passed_keeps_its_current_value(wire):
    client = wire(_group(types.ChatBannedRights(until_date=None, send_polls=True)))
    await mod.set_group_permissions("@group", photos=False)
    (request,) = client.sent
    assert isinstance(request, functions.messages.EditChatDefaultBannedRightsRequest)
    assert _banned(request) == {"send_photos", "send_polls"}


@pytest.mark.asyncio
async def test_desktop_never_sends_the_legacy_flags(wire):
    client = wire(_group(types.ChatBannedRights(until_date=None, send_media=True)))
    await mod.set_group_permissions("@group", send_messages=False)
    assert _banned(client.sent[0]) == {"send_plain", "embed_links"}


@pytest.mark.asyncio
async def test_send_media_off_restricts_all_ten_kinds(wire):
    client = wire()
    await mod.set_group_permissions("@group", send_media=False)
    assert _banned(client.sent[0]) == MEDIA_FIELDS


@pytest.mark.asyncio
async def test_a_named_kind_wins_over_its_send_media_parent(wire):
    client = wire()
    await mod.set_group_permissions("@group", send_media=False, photos=True)
    assert _banned(client.sent[0]) == MEDIA_FIELDS - {"send_photos"}


@pytest.mark.asyncio
async def test_stickers_gifs_games_and_inline_are_one_item(wire):
    client = wire()
    await mod.set_group_permissions("@group", stickers_and_gifs=False)
    assert _banned(client.sent[0]) == {"send_stickers", "send_gifs", "send_games", "send_inline"}
    rights = types.ChatBannedRights(until_date=None, send_gifs=True)
    client = wire(_group(rights))
    await mod.set_group_permissions("@group", stickers_and_gifs=True)
    assert _banned(client.sent[0]) == set()


@pytest.mark.parametrize(
    "item,fields",
    [
        ("send_messages", {"send_plain", "embed_links"}),
        ("photos", {"send_photos"}),
        ("video_files", {"send_videos"}),
        ("video_messages", {"send_roundvideos"}),
        ("music", {"send_audios"}),
        ("voice_messages", {"send_voices"}),
        ("files", {"send_docs"}),
        ("embed_links", {"embed_links"}),
        ("polls", {"send_polls"}),
        ("reactions", {"send_reactions"}),
        ("add_members", {"invite_users"}),
        ("pin_messages", {"pin_messages"}),
        ("edit_own_tags", {"edit_rank"}),
        ("change_group_info", {"change_info"}),
    ],
)
@pytest.mark.asyncio
async def test_each_item_reaches_its_own_fields(wire, item, fields):
    client = wire()
    await mod.set_group_permissions("@group", **{item: False})
    assert _banned(client.sent[0]) == fields


@pytest.mark.asyncio
async def test_embed_links_without_send_messages_is_refused(wire):
    client = wire()
    text = await mod.set_group_permissions("@group", send_messages=False, embed_links=True)
    assert client.sent == [] and "Embed links" in text and "Send messages" in text


@pytest.mark.asyncio
async def test_create_topics_exists_only_in_forums(wire):
    client = wire()
    text = await mod.set_group_permissions("@group", create_topics=False)
    assert client.sent == [] and "forum" in text.lower()
    client = wire(_group(forum=True))
    await mod.set_group_permissions("@group", create_topics=False)
    assert _banned(client.sent[0]) == {"manage_topics"}


@pytest.mark.parametrize("item", ["pin_messages", "change_group_info"])
@pytest.mark.asyncio
async def test_a_public_group_cannot_allow_pin_or_info(wire, item):
    client = wire(_group(username="publicgroup"))
    text = await mod.set_group_permissions("@group", **{item: True})
    assert client.sent == [] and "public groups" in text


@pytest.mark.parametrize(
    "value,seconds",
    [
        ("off", 0),
        ("5s", 5),
        ("10s", 10),
        ("30s", 30),
        ("1m", 60),
        ("5m", 300),
        ("15m", 900),
        ("1h", 3600),
        (30, 30),
    ],
)
@pytest.mark.asyncio
async def test_slow_mode_takes_desktops_eight_values(wire, value, seconds):
    client = wire()
    await mod.set_group_permissions("@group", slow_mode=value)
    (request,) = client.sent
    assert isinstance(request, functions.channels.ToggleSlowModeRequest)
    assert request.seconds == seconds


@pytest.mark.parametrize("value", ["2m", 20, -5, True, "fast"])
@pytest.mark.asyncio
async def test_slow_mode_outside_desktops_list_is_refused(wire, value):
    client = wire()
    text = await mod.set_group_permissions("@group", photos=False, slow_mode=value)
    assert client.sent == [] and "slow_mode" in text


@pytest.mark.asyncio
async def test_charging_stars_sets_the_group_price(wire):
    client = wire()
    await mod.set_group_permissions("@group", charge_stars_per_message=10)
    (request,) = client.sent
    assert isinstance(request, functions.channels.UpdatePaidMessagesPriceRequest)
    assert request.send_paid_messages_stars == 10 and not request.broadcast_messages_allowed


@pytest.mark.parametrize("value", [-1, True, 2.5])
@pytest.mark.asyncio
async def test_a_bad_star_price_is_refused(wire, value):
    client = wire()
    text = await mod.set_group_permissions("@group", charge_stars_per_message=value)
    assert client.sent == [] and "charge_stars_per_message" in text


@pytest.mark.asyncio
async def test_boosters_are_exempted_when_something_is_restricted(wire):
    client = wire()
    await mod.set_group_permissions("@group", photos=False, do_not_restrict_boosters=3)
    request = client.sent[-1]
    assert isinstance(request, functions.channels.SetBoostsToUnblockRestrictionsRequest)
    assert request.boosts == 3


@pytest.mark.parametrize("value", [6, -1, True])
@pytest.mark.asyncio
async def test_boosters_outside_one_to_five_are_refused(wire, value):
    client = wire()
    text = await mod.set_group_permissions("@group", photos=False, do_not_restrict_boosters=value)
    assert client.sent == [] and "do_not_restrict_boosters" in text


@pytest.mark.asyncio
async def test_boosters_need_a_restriction_or_slow_mode(wire):
    """Desktop shows the boosters option only when sending is restricted or slow mode is on."""
    client = wire()
    text = await mod.set_group_permissions("@group", do_not_restrict_boosters=2)
    assert client.sent == [] and "slow mode" in text.lower()


@pytest.mark.asyncio
async def test_a_basic_group_takes_rights_but_not_supergroup_settings(wire):
    client = wire(BASIC)
    text = await mod.set_group_permissions("@group", photos=False, slow_mode="30s")
    assert client.sent == [] and "upgrade_to_supergroup" in text
    await mod.set_group_permissions("@group", photos=False, slow_mode="off")
    (request,) = client.sent
    assert _banned(request) == {"send_photos"}


@pytest.mark.asyncio
async def test_a_broadcast_channel_has_no_member_permissions(wire):
    client = wire(NEWS)
    text = await mod.set_group_permissions("@group", photos=False)
    assert client.sent == [] and "channel" in text.lower()


@pytest.mark.asyncio
async def test_nothing_passed_changes_nothing(wire):
    client = wire()
    text = await mod.set_group_permissions("@group")
    assert client.sent == [] and "Nothing" in text


@pytest.mark.parametrize("value", ["yes", 1])
@pytest.mark.asyncio
async def test_an_item_that_is_not_true_or_false_is_refused(wire, value):
    client = wire()
    text = await mod.set_group_permissions("@group", photos=value)
    assert client.sent == [] and "photos" in text


@pytest.mark.asyncio
async def test_a_failed_step_stops_and_says_what_was_applied(wire):
    client = wire(fail_on=functions.channels.ToggleSlowModeRequest)
    text = await mod.set_group_permissions(
        "@group", photos=False, slow_mode="30s", do_not_restrict_boosters=2
    )
    assert [type(r) for r in client.sent] == [
        functions.messages.EditChatDefaultBannedRightsRequest
    ]
    assert "Slow mode" in text and "not applied" in text.lower()


# --- member exceptions -----------------------------------------------------------------


@pytest.mark.asyncio
async def test_an_exception_starts_from_the_group_defaults(wire):
    client = wire(_group(types.ChatBannedRights(until_date=None, send_polls=True)))
    await mod.set_member_exception("@group", 42, photos=False)
    (request,) = client.sent
    assert isinstance(request, functions.channels.EditBannedRequest)
    assert request.participant is USER
    assert _banned(request) == {"send_photos", "send_polls"}


@pytest.mark.asyncio
async def test_an_exception_starts_from_the_members_own_rights(wire):
    own = types.ChatBannedRights(until_date=None, send_voices=True)
    participant = types.ChannelParticipantBanned(
        peer=types.PeerUser(42), kicked_by=1, date=None, banned_rights=own
    )
    client = wire(participant=participant)
    await mod.set_member_exception("@group", 42, photos=False, until_date=1893456000)
    (request,) = client.sent
    assert _banned(request) == {"send_voices", "send_photos"}
    assert request.banned_rights.until_date == 1893456000


@pytest.mark.asyncio
async def test_an_exception_cannot_allow_what_the_group_forbids(wire):
    client = wire(_group(types.ChatBannedRights(until_date=None, send_polls=True)))
    text = await mod.set_member_exception("@group", 42, polls=True)
    assert client.sent == [] and "disabled for all members" in text and "Polls" in text


@pytest.mark.asyncio
async def test_a_removed_member_is_not_given_an_exception(wire):
    kicked = types.ChannelParticipantBanned(
        peer=types.PeerUser(42),
        kicked_by=1,
        date=None,
        banned_rights=types.ChatBannedRights(until_date=None, view_messages=True),
        left=True,
    )
    client = wire(participant=kicked)
    text = await mod.set_member_exception("@group", 42, photos=False)
    assert client.sent == [] and "unban_user" in text


@pytest.mark.asyncio
async def test_an_exception_needs_a_supergroup(wire):
    client = wire(BASIC)
    text = await mod.set_member_exception("@group", 42, photos=False)
    assert client.sent == [] and "upgrade_to_supergroup" in text


@pytest.mark.asyncio
async def test_removing_an_exception_lifts_every_restriction(wire):
    own = types.ChatBannedRights(until_date=None, send_voices=True)
    participant = types.ChannelParticipantBanned(
        peer=types.PeerUser(42), kicked_by=1, date=None, banned_rights=own
    )
    client = wire(participant=participant)
    await mod.remove_member_exception("@group", 42)
    (request,) = client.sent
    assert isinstance(request, functions.channels.EditBannedRequest)
    assert _banned(request) == set()


@pytest.mark.asyncio
async def test_removing_a_missing_exception_sends_nothing(wire):
    client = wire()
    text = await mod.remove_member_exception("@group", 42)
    assert client.sent == [] and "no exception" in text


# --- the approval line -----------------------------------------------------------------


def test_the_line_lists_rights_in_desktop_order_and_words():
    line = mod.permissions_line(
        {
            "chat_id": "@group",
            "change_group_info": True,
            "photos": True,
            "send_messages": True,
            "video_files": True,
            "add_members": True,
            "slow_mode": "30s",
            "charge_stars_per_message": 10,
            "do_not_restrict_boosters": 2,
        }
    )
    assert line == (
        "permissions: Send messages | Send media: Photos, Video files | Add members"
        " | Change group info | Charge Stars for Messages: 10"
        " | Do not restrict boosters: 2 | Slow mode: 30s"
    )


def test_the_line_names_what_is_turned_off():
    line = mod.permissions_line({"send_media": False, "photos": True, "pin_messages": False})
    assert line == (
        "permissions: Send media: Photos; not allowed: Send media: Video files, Video messages,"
        " Music, Voice messages, Files, Stickers & GIFs, Embed links, Polls, Reactions"
        " | Pin messages"
    )


def test_a_member_exception_says_own_tag():
    line = mod.permissions_line({"user_id": 42, "edit_own_tags": False, "slow_mode": "off"})
    assert line == "permissions: not allowed: Edit own tag"


def test_an_empty_call_says_nothing_changes():
    assert mod.permissions_line({"chat_id": 1}) == "permissions: nothing changes"


def test_the_old_tool_is_gone_and_the_new_ones_ask_the_owner():
    from telegram_mcp.safeguard.policy import GATED
    from telegram_mcp.tools import moderation

    assert not hasattr(moderation, "set_default_chat_permissions")
    assert "set_default_chat_permissions" not in GATED
    assert {"set_group_permissions", "set_member_exception", "remove_member_exception"} <= GATED


# --- wiring: the approval shows the permissions, bad values never reach the owner (T010) ---


@pytest.mark.asyncio
@pytest.mark.parametrize("tool", ["set_group_permissions", "set_member_exception"])
async def test_the_approval_lists_the_permissions_being_set(tool):
    from telegram_mcp import approval_details

    arguments = {"chat_id": -100555, "photos": True, "pin_messages": False}
    if tool == "set_member_exception":
        arguments["user_id"] = 42

    line = await approval_details.detail_for(tool, arguments)

    assert line == mod.permissions_line(arguments) and line.startswith("permissions: ")


@pytest.mark.asyncio
async def test_removing_an_exception_says_so_in_the_approval():
    from telegram_mcp import approval_details

    line = await approval_details.detail_for(
        "remove_member_exception", {"chat_id": -100555, "user_id": 42}
    )

    assert line.startswith("permissions: ") and "group's permissions" in line


@pytest.mark.parametrize("tool", ["set_group_permissions", "set_member_exception"])
def test_bad_values_are_refused_before_the_owner_is_asked(tool):
    from telegram_mcp import preflight

    assert preflight.RULES[tool]({"chat_id": 1, "slow_mode": "7s"})
    assert preflight.RULES[tool]({"chat_id": 1, "photos": True}) is None


@pytest.mark.asyncio
async def test_editing_an_exception_keeps_its_end_date(wire):
    """Review M2: until_date not passed keeps the member's own end date, not "forever"."""
    own = types.ChatBannedRights(until_date=1893456000, send_voices=True)
    participant = types.ChannelParticipantBanned(
        peer=types.PeerUser(42), kicked_by=1, date=None, banned_rights=own
    )
    client = wire(participant=participant)

    await mod.set_member_exception("@group", 42, photos=False)

    (request,) = client.sent
    until = request.banned_rights.until_date
    assert until == 1893456000 or getattr(until, "timestamp", lambda: 0)() == 1893456000


def test_the_approval_shows_an_exceptions_end_date():
    line = mod.permissions_line({"chat_id": 1, "user_id": 42, "photos": False, "until_date": 99})

    assert "until 99" in line
