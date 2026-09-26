"""The community tools (spec 007), on a fake client.

What these pin, because none of it shows in a return value:

  - A community is resolved from the account's own joined-communities list into an
    `InputChannel(id, access_hash)`. An id not on that list is refused before any
    change reaches Telegram.
  - A linked chat's visibility is permanent, so it is never defaulted: anything but
    exactly "visible" or "hidden" is refused without a request.
  - "Who can add chats" flips only `manage_linked_peers` and keeps every other
    default restriction the community already had.
  - A `Bool` false from Telegram is reported as not done, never as success.
"""

import pytest
from telethon.errors import ChatNotModifiedError
from telethon.tl import functions, types
from telethon.tl.functions import communities as cf

from telegram_mcp.tools import communities as shape_mod
from telegram_mcp.tools import community_moderation as mod_mod

COMMUNITY = types.Community(
    id=900,
    title="Numera Hub",
    photo=types.ChatPhotoEmpty(),
    date=None,
    access_hash=77,
    default_banned_rights=types.ChatBannedRights(until_date=None, send_polls=True),
)
CHANNEL = types.Channel(id=555, title="Announcements", photo=types.ChatPhotoEmpty(), date=None)
USER = types.User(id=42, first_name="Ann")


class FakeClient:
    def __init__(self, answers=None):
        self.sent = []
        self.answers = answers or {}

    async def __call__(self, request):
        if isinstance(request, cf.GetJoinedCommunitiesRequest):
            return types.messages.Chats(chats=[COMMUNITY])
        self.sent.append(request)
        answer = self.answers.get(type(request), True)
        if isinstance(answer, Exception):
            raise answer
        return answer

    async def upload_file(self, handle):
        return types.InputFile(id=1, parts=1, name="p.jpg", md5_checksum="")


@pytest.fixture
def client(monkeypatch):
    fake = FakeClient()
    entities = {"@announcements": CHANNEL, 555: CHANNEL, "@ann_user": USER, 42: USER}

    async def _resolve(value, cl=None, account=None):
        return entities[value]

    for module in (shape_mod, mod_mod):
        monkeypatch.setattr(module, "get_client", lambda account=None: fake)
        monkeypatch.setattr(module, "resolve_entity", _resolve)
    return fake


def _is_the_community(channel):
    return (
        isinstance(channel, types.InputChannel)
        and channel.channel_id == 900
        and channel.access_hash == 77
    )


# --- US1: create and shape -----------------------------------------------------------


@pytest.mark.asyncio
async def test_list_names_each_community_with_who_can_add(client):
    text = await shape_mod.list_my_communities()
    assert "900" in text and "Numera Hub" in text and "all members" in text


@pytest.mark.asyncio
async def test_an_unknown_community_is_refused_without_a_request(client):
    text = await shape_mod.rename_community(community=1234, title="X")
    assert "1234" in text and "not" in text.lower()
    assert client.sent == []


@pytest.mark.asyncio
async def test_create_sends_the_first_chat_and_hidden_only_when_asked(client):
    client.answers[cf.CreateRequest] = types.Updates(
        updates=[], users=[], chats=[COMMUNITY], date=None, seq=0
    )
    text = await shape_mod.create_community(first_chat="@announcements", title="Numera Hub")
    request = client.sent[0]
    assert isinstance(request, cf.CreateRequest)
    assert request.peer is CHANNEL and request.title == "Numera Hub"
    assert not request.hidden and request.about is None
    assert "900" in text

    await shape_mod.create_community(
        first_chat="@announcements", title="T", about="a", hidden=True
    )
    assert client.sent[1].hidden is True and client.sent[1].about == "a"


@pytest.mark.asyncio
async def test_rename_edits_the_resolved_community(client):
    await shape_mod.rename_community(community=900, title="New")
    request = client.sent[0]
    assert isinstance(request, functions.channels.EditTitleRequest)
    assert _is_the_community(request.channel) and request.title == "New"


@pytest.mark.asyncio
async def test_renaming_to_the_same_name_is_no_change(client):
    client.answers[functions.channels.EditTitleRequest] = ChatNotModifiedError(request=None)
    text = await shape_mod.rename_community(community=900, title="Numera Hub")
    assert "no change" in text.lower()


@pytest.mark.asyncio
async def test_set_photo_uploads_through_the_folder_rule(client, monkeypatch, tmp_path):
    picture = tmp_path / "p.jpg"
    picture.write_bytes(b"\xff\xd8\xff")

    class _Source:
        path = str(picture)
        handle = None

    class _Opened:
        async def __aenter__(self):
            return _Source(), None

        async def __aexit__(self, *exc):
            return False

    monkeypatch.setattr(shape_mod, "_open_verified_source", lambda **kw: _Opened())
    await shape_mod.set_community_photo(community=900, file_path=str(picture))
    request = client.sent[0]
    assert isinstance(request, functions.channels.EditPhotoRequest)
    assert _is_the_community(request.channel)
    assert isinstance(request.photo, types.InputChatUploadedPhoto)


@pytest.mark.asyncio
async def test_a_path_the_folder_rule_refuses_sends_nothing(client, monkeypatch):
    class _Refused:
        async def __aenter__(self):
            return None, "refused by the folder rule"

        async def __aexit__(self, *exc):
            return False

    monkeypatch.setattr(shape_mod, "_open_verified_source", lambda **kw: _Refused())
    text = await shape_mod.set_community_photo(community=900, file_path="C:/x.jpg")
    assert text == "refused by the folder rule" and client.sent == []


@pytest.mark.asyncio
async def test_delete_photo_sends_the_empty_photo(client):
    await shape_mod.delete_community_photo(community=900)
    request = client.sent[0]
    assert isinstance(request.photo, types.InputChatPhotoEmpty)
    assert _is_the_community(request.channel)


# --- US2: link and unlink ------------------------------------------------------------


@pytest.mark.parametrize("visibility", [None, "", "Visible ", "public", "true"])
@pytest.mark.asyncio
async def test_adding_without_an_exact_visibility_is_refused(client, visibility):
    text = await mod_mod.add_chat_to_community(
        community=900, chat="@announcements", visibility=visibility
    )
    assert "permanent" in text.lower() and client.sent == []


@pytest.mark.parametrize("visibility", ["visible", "hidden"])
@pytest.mark.asyncio
async def test_adding_sends_exactly_the_chosen_flag(client, visibility):
    await mod_mod.add_chat_to_community(
        community=900, chat="@announcements", visibility=visibility
    )
    request = client.sent[0]
    assert isinstance(request, cf.TogglePeerLinkRequest)
    assert _is_the_community(request.community) and request.peer is CHANNEL
    assert bool(request.visible) is (visibility == "visible")
    assert bool(request.hidden) is (visibility == "hidden")
    assert not request.deleted


@pytest.mark.asyncio
async def test_removing_sends_deleted(client):
    await mod_mod.remove_chat_from_community(community=900, chat="@announcements")
    request = client.sent[0]
    assert request.deleted is True and not request.visible and not request.hidden


@pytest.mark.asyncio
async def test_a_false_answer_is_not_done(client):
    client.answers[cf.TogglePeerLinkRequest] = False
    text = await mod_mod.remove_chat_from_community(community=900, chat="@announcements")
    assert "not" in text.lower()


@pytest.mark.asyncio
async def test_linked_chats_are_listed_with_their_visibility(client):
    full = types.CommunityFull(
        id=900,
        about="",
        chat_photo=types.PhotoEmpty(id=0),
        linked_peers=[
            types.CommunityPeer(peer=types.PeerChannel(555), visible=True),
            types.CommunityPeer(peer=types.PeerUser(42), visible=False),
        ],
    )
    client.answers[functions.channels.GetFullChannelRequest] = types.messages.ChatFull(
        full_chat=full, chats=[CHANNEL], users=[USER]
    )
    text = await shape_mod.get_community_chats(community=900)
    lines = text.splitlines()
    assert any("Announcements" in line and "visible" in line for line in lines)
    assert any("Ann" in line and "hidden" in line for line in lines)


# --- US3: moderate -------------------------------------------------------------------


@pytest.mark.parametrize("who,flag", [("only_admins", True), ("all_members", False)])
@pytest.mark.asyncio
async def test_who_can_add_flips_only_manage_linked_peers(client, who, flag):
    await shape_mod.set_community_who_can_add(community=900, who=who)
    request = client.sent[0]
    assert isinstance(request, functions.messages.EditChatDefaultBannedRightsRequest)
    assert request.peer.channel_id == 900 and request.peer.access_hash == 77
    assert bool(request.banned_rights.manage_linked_peers) is flag
    assert request.banned_rights.send_polls is True, "an existing restriction was dropped"
    assert COMMUNITY.default_banned_rights.manage_linked_peers is None, "mutated in place"


@pytest.mark.asyncio
async def test_who_can_add_refuses_other_values(client):
    text = await shape_mod.set_community_who_can_add(community=900, who="everyone")
    assert "all_members" in text and client.sent == []


@pytest.mark.asyncio
async def test_link_requests_are_listed_with_chat_requester_and_date(client):
    from datetime import datetime, timezone

    client.answers[cf.GetPeerLinkRequestsRequest] = types.communities.PeerLinkRequests(
        total_count=1,
        requests=[
            types.CommunityPeerRequest(
                peer=types.PeerChannel(555),
                requested_by=42,
                date=datetime(2026, 9, 27, tzinfo=timezone.utc),
                visible=True,
            )
        ],
        chats=[CHANNEL],
        users=[USER],
    )
    text = await mod_mod.get_community_link_requests(community=900)
    assert "Announcements" in text and "Ann" in text and "2026-09-27" in text
    assert client.sent[0].limit == 50


@pytest.mark.parametrize("tool,reject", [("approve", None), ("reject", True)])
@pytest.mark.asyncio
async def test_one_link_request_is_approved_or_rejected(client, tool, reject):
    await getattr(mod_mod, f"{tool}_community_link_request")(community=900, chat="@announcements")
    request = client.sent[0]
    assert isinstance(request, cf.TogglePeerLinkRequestApprovalRequest)
    assert request.peer is CHANNEL and request.reject is reject


@pytest.mark.parametrize("tool,reject", [("approve", None), ("reject", True)])
@pytest.mark.asyncio
async def test_all_link_requests_at_once(client, tool, reject):
    await getattr(mod_mod, f"{tool}_community_link_request")(community=900, all=True)
    request = client.sent[0]
    assert isinstance(request, cf.ToggleAllPeerLinkRequestApprovalRequest)
    assert _is_the_community(request.community) and request.reject is reject


@pytest.mark.parametrize("kwargs", [{}, {"chat": "@announcements", "all": True}])
@pytest.mark.asyncio
async def test_a_link_request_needs_exactly_one_target(client, kwargs):
    text = await mod_mod.reject_community_link_request(community=900, **kwargs)
    assert "all" in text and client.sent == []


@pytest.mark.asyncio
async def test_ban_alone_touches_only_the_community(client):
    await mod_mod.ban_community_member(community=900, user="@ann_user")
    assert len(client.sent) == 1
    request = client.sent[0]
    assert isinstance(request, cf.ToggleParticipantBannedRequest)
    assert request.participant is USER and not request.unban


@pytest.mark.asyncio
async def test_ban_also_from_chats_removes_them_from_each_joined_chat(client):
    other = types.Channel(id=556, title="Talk", photo=types.ChatPhotoEmpty(), date=None)
    client.answers[cf.GetParticipantJoinedChatsRequest] = types.communities.ParticipantJoinedChats(
        creator_chat_ids=[], joined_chat_ids=[555, 556], chats=[CHANNEL, other], users=[]
    )
    text = await mod_mod.ban_community_member(
        community=900, user="@ann_user", also_from_chats=True
    )
    kinds = [type(r) for r in client.sent]
    assert kinds[:2] == [cf.GetParticipantJoinedChatsRequest, cf.ToggleParticipantBannedRequest]
    removals = [r for r in client.sent if isinstance(r, functions.channels.EditBannedRequest)]
    assert [r.channel.id for r in removals] == [555, 556]
    assert all(r.participant is USER and r.banned_rights.view_messages for r in removals)
    assert "Announcements" in text and "Talk" in text


@pytest.mark.asyncio
async def test_unban_sends_the_unban_flag(client):
    await mod_mod.unban_community_member(community=900, user=42)
    assert client.sent[0].unban is True


# --- US4: delete ---------------------------------------------------------------------


@pytest.mark.asyncio
async def test_delete_deletes_the_resolved_community_only(client):
    await shape_mod.delete_community(community=900)
    assert len(client.sent) == 1
    request = client.sent[0]
    assert isinstance(request, functions.channels.DeleteChannelRequest)
    assert _is_the_community(request.channel)
