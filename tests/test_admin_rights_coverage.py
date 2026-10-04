"""Every admin right Telegram has, reachable through the tools that grant them.

`promote_admin` hand-wrote twelve fields into `ChatAdminRights`. Telethon 1.44
carries **seventeen**. The five it never constructed - `post_stories`,
`edit_stories`, `delete_stories`, `manage_direct_messages` and `manage_ranks` -
could not be granted by any caller, however complete a `rights` dict it passed:
the keys were simply read past. The failure was invisible from the server side.
It showed up only in Telegram's own admin panel, as "Manage stories 0/3" and two
switches sitting off after a promotion that had reported success.

`demote_admin` had the mirror image: it set twelve fields to False and left the
other five untouched, so a "demotion" could leave story and direct-message
rights standing.

The fix is not a longer list - a longer list falls behind the next time Telegram
adds a right. It is to build the rights object FROM the installed type, which is
what `_build_admin_rights` does and what these tests pin.

For three weeks that was not enough on its own. Telethon 1.44 announced layer
227, which has no `manage_linked_peers` (flags.19) or `manage_welcome_messages`
(flags.20), and Telegram masks flags newer than the layer the client announced
- silently, with a success reply - so those two were hand-added to the object
and then delivered over TDLib. Telethon 1.45 announces layer 229 and carries
both as ordinary fields, so the detour is gone and the installed type is the
whole truth again. What stays is where these are pinned: the bytes on the wire,
because an attribute set on a Python object proves nothing.

Spec 033 split `promote_admin` and `edit_admin_rights` into one tool per chat type; the
tests that pinned their old defaults now pin the builder, the wire bits and the per-type
tools (`test_admin_rights_tools.py` holds the per-type behaviour).
"""

import inspect
import json
from types import SimpleNamespace

import pytest
from telethon.client.chats import ChatMethods
from telethon.errors import BadRequestError
from telethon.tl import functions, types
from telethon.tl.functions import communities as community_requests
from telethon.tl.types import ChatAdminRights

from telegram_mcp import admin_rights_sets as sets

# The admin-rights model moved out of `moderation` into its own module; the
# alias is kept so the assertions below still read as they did.
from telegram_mcp.tools import admin_rights as moderation_mod
from telegram_mcp.tools import admin_rights_by_type as by_type

# Telegram's two most recent admin rights, flags.19 and flags.20 - the pair a
# client one layer behind drops silently.
_THE_TWO_NEWEST = {"manage_linked_peers", "manage_welcome_messages"}

_PHOTO = types.ChatPhotoEmpty()
_CHANNEL = types.Channel(id=5876481644, title="News", photo=_PHOTO, date=None, broadcast=True)
_COMMUNITY = types.Community(id=900, title="Hub", photo=_PHOTO, date=None, access_hash=77)


def _telethon_fields():
    return {
        name for name in inspect.signature(ChatAdminRights.__init__).parameters if name != "self"
    }


def _all_fields():
    """Every right this server can grant, whether or not Telethon knows it."""
    return set(moderation_mod._admin_rights_fields())


def _flags_on_the_wire(rights):
    """The flags int Telegram will actually receive.

    `ChatAdminRights` is a payload-free flags object, so its whole serialised
    form is the constructor id followed by this one integer. Reading it back is
    the only check that distinguishes a right that was set from a right that was
    merely stored on a Python object and then dropped.
    """
    raw = bytes(rights)
    assert len(raw) == 8, f"expected id+flags, got {len(raw)} bytes"
    assert int.from_bytes(raw[:4], "little") == ChatAdminRights.CONSTRUCTOR_ID, (
        "constructor id changed - the hand-added flag bits below are only valid "
        "for the layout they were read from"
    )
    return int.from_bytes(raw[4:], "little")


def test_the_builder_knows_every_field_the_installed_telethon_has():
    """The guard that makes the rest of this file self-maintaining: if a future
    Telethon adds a right, this is what notices.

    An equality again as of 1.45. It was a superset for as long as two rights
    had to be named by hand, so the day it becomes one again is the day a
    hand-written list has crept back in.
    """
    assert _all_fields() == _telethon_fields()


def test_the_two_newest_rights_are_reachable_too():
    """Named individually rather than counted: these are the two a layer bump
    added last, and the two a client one layer behind drops again."""
    assert _THE_TWO_NEWEST <= _all_fields()


def test_the_two_newest_rights_reach_telegram_as_the_right_bits():
    """Setting an attribute on a Python object proves nothing: the earlier bug
    was exactly a right that looked set and never left the process. These bit
    positions come from layer 229's `chatAdminRights`, and this asserts the
    request puts them where Telegram reads them.
    """
    for name, bit in (("manage_linked_peers", 19), ("manage_welcome_messages", 20)):
        granted = _flags_on_the_wire(moderation_mod._build_admin_rights({name: True}))
        assert granted >> bit & 1, f"{name} never reached flags.{bit}"

        withheld = _flags_on_the_wire(moderation_mod._build_admin_rights({name: False}))
        assert not (withheld >> bit & 1), f"{name} set flags.{bit} when it was declined"


def test_the_two_newest_bits_are_the_only_ones_a_bare_grant_sets():
    """A flags field is one integer, so a wrong bit corrupts a neighbouring
    right rather than failing loudly. Pinned as an exact integer for that
    reason, rather than as two independent bit checks."""
    without = _flags_on_the_wire(moderation_mod._build_admin_rights({}))
    with_later = _flags_on_the_wire(
        moderation_mod._build_admin_rights(
            {"manage_linked_peers": True, "manage_welcome_messages": True}
        )
    )

    assert without == 0
    assert with_later == (1 << 19) | (1 << 20)


def test_the_five_that_were_unreachable_are_named_explicitly():
    """Pinned by name, not by count, so the specific regression cannot come back
    while the total happens to match."""
    fields = set(moderation_mod._admin_rights_fields())

    for right in (
        "post_stories",
        "edit_stories",
        "delete_stories",
        "manage_direct_messages",
        "manage_ranks",
    ):
        assert right in fields, right


def test_the_five_formerly_unreachable_rights_are_part_of_a_full_admin_where_they_exist():
    """The user-visible bug: a promotion reported success while Telegram's own
    panel showed "Manage stories 0/3" and two switches sitting off."""
    channel = sets.full_admin("channel")
    group = sets.full_admin("group", is_forum=True)

    for right in ("post_stories", "edit_stories", "delete_stories", "manage_direct_messages"):
        assert channel[right] is True, right
    assert group["manage_ranks"] is True
    assert all(group[right] for right in ("post_stories", "edit_stories", "delete_stories"))


def test_a_full_admin_holds_add_admins_and_never_anonymous():
    """The owner's definition (spec 033): every right of the type except "Remain
    anonymous", "Add new admins" included."""
    for kind in sets.KINDS:
        rights = sets.full_admin(kind, is_forum=True)
        assert rights["add_admins"] is True
        assert rights.get("anonymous", False) is False


def test_a_demotion_clears_every_field_including_the_new_ones():
    """`ChatAdminRights` fields left unset serialise as absent, which is how the
    old demotion could leave story and direct-message rights standing."""
    rights = moderation_mod._build_admin_rights({})

    for name in _all_fields():
        assert getattr(rights, name) is False, f"{name} was not explicitly cleared"


def test_an_unknown_key_is_ignored_rather_than_raising():
    """Telegram adds rights over time. A caller copying a newer example should
    lose that one right, not have the whole promotion refused."""
    rights = moderation_mod._build_admin_rights({"some_right_from_the_future": True})

    assert not any(getattr(rights, name) for name in _all_fields())


@pytest.mark.parametrize(
    "tool",
    [
        moderation_mod.demote_admin,
        *(
            getattr(by_type, f"{family}_{kind}")
            for family in ("promote_admin", "edit_admin_rights")
            for kind in sets.KINDS
        ),
    ],
)
def test_no_tool_hand_rolls_the_rights_object_any_more(tool):
    """All of them built their own `ChatAdminRights(...)` and fell behind
    together. One builder is the reason they cannot drift apart again."""
    source = inspect.getsource(tool)

    assert "ChatAdminRights(" not in source, (
        f"{tool.__name__} constructs ChatAdminRights directly again; use "
        "_build_admin_rights so a new Telethon field cannot go missing"
    )


def test_the_later_flags_survive_a_round_trip_through_telethons_reader():
    """Granting a right this server cannot then report would be the same
    asymmetry in a new place.

    `ChatAdminRights.from_reader` used to set the seventeen fields 1.44 knew and
    discard the rest, so bits 19 and 20 arrived from Telegram and vanished
    before any caller saw them - which is why the reader was wrapped. 1.45
    decodes both itself, which is what this proves and why the wrap could go.
    """
    from telethon.extensions.binaryreader import BinaryReader

    granted = moderation_mod._build_admin_rights(
        {"manage_welcome_messages": True, "manage_linked_peers": True}
    )

    read_back = BinaryReader(bytes(granted)).tgread_object()
    rights = moderation_mod.admin_rights_to_dict(read_back)

    assert rights["manage_welcome_messages"] is True
    assert rights["manage_linked_peers"] is True
    assert rights["ban_users"] is False, "a right never granted came back set"


def test_the_reported_rights_cover_exactly_what_can_be_set():
    """`get_admins` reads back through `admin_rights_to_dict`. A right the
    reader cannot name is a right nobody can see the absence of - which is how
    "one or two admins are missing this permission" became unanswerable
    without opening Telegram itself."""
    granted = moderation_mod._build_admin_rights({name: True for name in _all_fields()})

    assert set(moderation_mod.admin_rights_to_dict(granted)) == set(
        moderation_mod._admin_rights_fields()
    )


class _Fake:
    """A client answering what the per-type tools send; ``edit`` decides the edit's fate."""

    def __init__(self, edit=None, participant=None):
        self._edit = edit
        self._participant = participant
        self.edits = []

    def is_connected(self):
        return True

    async def __call__(self, request):
        name = type(request).__name__
        if isinstance(request, community_requests.GetJoinedCommunitiesRequest):
            return types.messages.Chats(chats=[_COMMUNITY])
        if name == "EditAdminRequest":
            self.edits.append(request)
            if self._edit is not None:
                raise self._edit
            return SimpleNamespace(updates=[])
        if name == "GetParticipantRequest":
            applied = self._participant(self.edits[-1]) if self._participant else None
            return SimpleNamespace(participant=SimpleNamespace(admin_rights=applied))
        raise AssertionError(f"unexpected request {name}")


@pytest.fixture
def _fake(wire_client):
    def _make(**kwargs):
        async def _resolve(reference, cl=None, account=None):
            return _CHANNEL

        return wire_client(by_type, _Fake(**kwargs), resolve=_resolve)

    return _make


@pytest.mark.asyncio
async def test_a_session_too_new_to_promote_says_so_instead_of_looking_like_a_permission_gap(
    _fake,
):
    """Telegram refuses admin changes from a login younger than about 24 hours,
    however complete its rights are. Measured live: the channel's own CREATOR
    was refused, minutes after that account was added.

    Worth its own message because the account that hits this is nearly always
    one just configured - the rights read correctly, the call fails, and the
    generic "you need admin rights" answer sends the reader to check a
    permission that was never the problem.
    """
    import telethon

    _fake(edit=telethon.errors.rpcerrorlist.FreshChangeAdminsForbiddenError(request=None))

    answer = await by_type.edit_admin_rights_channel(
        5876481644, 5876481644, change_info=True, account="acct"
    )

    assert "24 hours" in answer, "the age rule was not named"
    assert "anti-hijack" in answer
    assert "need admin rights" not in answer, "it still reads as a missing permission"


@pytest.mark.asyncio
async def test_a_right_telegram_declined_is_read_back_not_assumed_applied(_fake):
    """Measured live on a broadcast channel: `channels.editAdmin` was accepted in
    full and `pin_messages`, `manage_topics` and `manage_ranks` came back False,
    because those are not channel rights. Setting each one alone through TDLib
    produced the same False, so it is Telegram scoping them out, not a transport
    fault.

    The tool used to end its note with "Every other right in this call was
    applied" - a claim about an outcome nobody had read back. The owner acted on
    it and believed a user held rights Telegram had never granted. So the write
    is no longer the report: the rights are re-read and a declined one is named.
    """
    applied = ChatAdminRights(
        change_info=True,
        delete_messages=True,
        post_messages=False,
        manage_direct_messages=False,
    )
    _fake(participant=lambda request: applied)

    answer = await by_type.edit_admin_rights_channel(
        5876481644,
        5876481644,
        account="acct",
        change_info=True,
        delete_messages=True,
        post_messages=True,
        manage_direct_messages=True,
    )

    assert "Requested rights read back as off: manage_direct_messages, post_messages" in answer
    # A right that WAS applied must not be named as off.
    declined = answer.split("Requested rights read back as off:")[1]
    assert "change_info" not in declined
    assert "delete_messages" not in declined


@pytest.mark.asyncio
async def test_a_failed_read_back_does_not_turn_an_applied_change_into_an_error(_fake):
    """The check is an improvement to the report, not a second thing that can
    fail the call. If Telegram will not answer the read-back, the rights were
    still written - saying otherwise would be a worse lie than the one it
    replaced."""

    def _refused(request):
        raise ConnectionError("read-back refused")

    _fake(participant=_refused)

    answer = await by_type.edit_admin_rights_channel(
        5876481644, 5876481644, account="acct", change_info=True
    )

    assert "request was accepted" in answer.lower()
    assert "not verified" in answer
    assert "declined" not in answer, "an unread right was reported as declined"


@pytest.mark.asyncio
async def test_both_newest_rights_are_set_on_the_plain_telethon_path(_fake):
    """Telethon 1.45 announces layer 229 - the same layer TDLib does - so
    `channels.editAdmin` carries flags.19 and flags.20 by itself.

    Proved live on 2026-09-19, one pure `channels.editAdmin` with no fallback,
    on a real channel, restored afterwards:

        manage_linked_peers        asked True -> got True
        manage_welcome_messages    asked True -> got True

    So the answer has to be the plain one. Anything appended about TDLib means
    the detour is back, and with it a second Telegram authorisation for
    something Telethon already does. Linked peers is a community right and
    welcome messages a channel one, so each goes through its own tool.
    """
    client = _fake(participant=lambda request: request.admin_rights)

    await by_type.edit_admin_rights_channel(
        5876481644, 5876481644, account="acct", manage_welcome_messages=True
    )
    await by_type.edit_admin_rights_community(
        900, 5876481644, account="acct", manage_linked_peers=True
    )

    welcome, linked = client.edits
    assert _flags_on_the_wire(welcome.admin_rights) == 1 << 20, "flags.20 missing or mixed"
    assert _flags_on_the_wire(linked.admin_rights) == 1 << 19, "flags.19 missing or mixed"


# --- the caller can see which keys exist ------------------------------------------


@pytest.mark.parametrize("kind", sets.KINDS)
def test_the_description_lists_every_right_of_the_type(kind):
    """Reported 2026-09-29: the `rights` schema was `{}`, so callers guessed the names."""
    doc = getattr(by_type, f"promote_admin_{kind}").__doc__
    for name in sets.fields(kind, is_forum=True):
        assert name in doc, name


@pytest.mark.asyncio
async def test_a_privacy_refusal_is_named(_fake):
    import telethon.errors.rpcerrorlist as rpc

    _fake(edit=rpc.UserPrivacyRestrictedError(request=None))

    refused = await by_type.promote_admin_channel(5876481644, 5876481644, account="acct")

    assert refused.startswith("Error") and "chat_invite" in refused


class _AdminReader:
    get_participants = ChatMethods.get_participants
    iter_participants = ChatMethods.iter_participants

    def __init__(self, pages, *, joined=True, failure=None):
        self.pages = pages
        self.joined = joined
        self.failure = failure
        self.requests = []

    async def get_input_entity(self, target):
        return types.InputPeerChannel(900, 77)

    async def __call__(self, request):
        if isinstance(request, community_requests.GetJoinedCommunitiesRequest):
            if self.failure == "joined":
                raise ConnectionError("joined-list unavailable")
            return types.messages.Chats(chats=[_COMMUNITY] if self.joined else [])
        assert isinstance(request, functions.channels.GetParticipantsRequest)
        self.requests.append((type(request.filter), request.offset, request.limit))
        if isinstance(request.filter, types.ChannelParticipantsRecent):
            if self.joined:
                raise BadRequestError(request, "COMMUNITY_FILTER_INVALID")
            return SimpleNamespace(count=3)
        assert isinstance(request.filter, types.ChannelParticipantsAdmins)
        assert request.hash == 0
        assert request.channel.channel_id == 900 and request.channel.access_hash == 77
        page = self.pages[request.offset]
        if isinstance(page, Exception):
            raise page
        return page


def _admin_page(ids, *, missing=False):
    rights = ChatAdminRights(change_info=True, manage_linked_peers=True)
    participants = [
        types.ChannelParticipantCreator(user_id=user_id, admin_rights=rights, rank="Owner")
        for user_id in ids
    ]
    users = [types.User(id=user_id, first_name=f"Admin {user_id}") for user_id in reversed(ids)]
    if missing:
        users.pop()
    users.append(types.User(id=99, first_name="Auxiliary inviter"))
    return types.channels.ChannelParticipants(
        count=3, participants=participants, chats=[], users=users
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("target", [900, "900", -100900])
async def test_get_admins_reads_community_without_recent_and_pages_by_participants(
    wire_client, target
):
    client = wire_client(
        moderation_mod,
        _AdminReader({0: _admin_page([1, 2]), 2: _admin_page([3]), 3: _admin_page([])}),
    )
    answer = await moderation_mod.get_admins(target)
    records = json.loads(answer)["results"]
    assert [record["id"] for record in records] == [1, 2, 3]
    assert [record["name"] for record in records] == ["Admin 1", "Admin 2", "Admin 3"]
    assert all(record["role"] == "creator" and record["rank"] == "Owner" for record in records)
    assert all(record["rights"]["manage_linked_peers"] for record in records)
    assert all(record["rights"]["ban_users"] is False for record in records)
    assert [(kind, offset) for kind, offset, _ in client.requests] == [
        (types.ChannelParticipantsAdmins, 0),
        (types.ChannelParticipantsAdmins, 2),
        (types.ChannelParticipantsAdmins, 3),
    ]
    assert all(0 < limit <= 200 for _, _, limit in client.requests)


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["missing_user", "later_page", "joined"])
async def test_get_admins_never_reports_incomplete_community_pages_as_success(
    wire_client, failure
):
    pages = {0: _admin_page([1]), 1: _admin_page([2], missing=True)}
    if failure == "later_page":
        pages[1] = ConnectionError("admin page unavailable")
    client = wire_client(moderation_mod, _AdminReader(pages, failure=failure))
    answer = await moderation_mod.get_admins(900)
    assert "error" in answer.lower() and '"results"' not in answer
    expected_offsets = [] if failure == "joined" else [0, 1]
    assert [offset for _, offset, _ in client.requests] == expected_offsets


@pytest.mark.asyncio
async def test_get_admins_empty_community_ignores_auxiliary_users(wire_client):
    client = wire_client(moderation_mod, _AdminReader({0: _admin_page([])}))
    assert await moderation_mod.get_admins(900) == "No admins found."
    assert [offset for _, offset, _ in client.requests] == [0]


@pytest.mark.asyncio
@pytest.mark.parametrize("target", [123, -100123, "@ordinary_channel"])
async def test_get_admins_preserves_ordinary_numeric_and_username_paths(wire_client, target):
    client = wire_client(
        moderation_mod, _AdminReader({0: _admin_page([1]), 1: _admin_page([])}, joined=False)
    )
    answer = await moderation_mod.get_admins(target)
    assert [record["id"] for record in json.loads(answer)["results"]] == [1]
    assert client.requests[0][:2] == (types.ChannelParticipantsRecent, 0)
