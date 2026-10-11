"""Read-only one-member status on real TL participant families."""

import json

import pytest
from telethon.errors import UserNotParticipantError
from telethon.tl import functions, types

from telegram_mcp.tools import admin_rights

RIGHTS = types.ChatAdminRights(change_info=True, invite_users=True)
GROUP = types.Channel(id=7, title="Group", photo=None, date=None, megagroup=True)
USER = types.User(id=42, first_name="Member")


class StatusClient:
    def __init__(self, participant):
        self.participant = participant
        self.requests = []

    async def __call__(self, request):
        self.requests.append(request)
        assert isinstance(request, functions.channels.GetParticipantRequest)
        assert request.channel is GROUP and request.participant is USER
        if isinstance(self.participant, Exception):
            raise self.participant
        return types.channels.ChannelParticipant(
            participant=self.participant, chats=[], users=[USER]
        )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "participant,role",
    [
        (types.ChannelParticipantCreator(42, RIGHTS, rank="Boss‮"), "creator"),
        (types.ChannelParticipantAdmin(42, 1, None, RIGHTS, rank="Boss‮"), "admin"),
        (types.ChannelParticipant(42, None), "member"),
        (types.ChannelParticipantSelf(42, 1, None), "member"),
        (
            types.ChannelParticipantBanned(
                types.PeerUser(42), 1, None, types.ChatBannedRights(None, send_messages=True)
            ),
            "restricted",
        ),
        (
            types.ChannelParticipantBanned(
                types.PeerUser(42),
                1,
                None,
                types.ChatBannedRights(None, view_messages=True),
                left=True,
            ),
            "banned",
        ),
        (
            types.ChannelParticipantBanned(
                types.PeerUser(42),
                1,
                None,
                types.ChatBannedRights(None, send_messages=True),
                left=True,
            ),
            "left",
        ),
        (types.ChannelParticipantLeft(types.PeerUser(42)), "left"),
        (UserNotParticipantError(None), "left"),
    ],
)
async def test_member_status_classifies_rights_without_mutation(wire_client, participant, role):
    async def resolve(value, client, account):
        return GROUP if value == 7 else USER

    client = wire_client(admin_rights, StatusClient(participant), resolve=resolve)
    tool = getattr(admin_rights, "get_member_admin_status", None)
    assert callable(tool), "get_member_admin_status must be a public reader"
    payload = json.loads(await tool(7, 42, account="default"))
    record = payload["results"][0]
    assert record["role"] == role
    assert record["is_admin"] is (role in ("creator", "admin"))
    assert record["is_creator"] is (role == "creator")
    if role in ("creator", "admin"):
        assert record["rank"] == "Boss"
        assert record["rights"]["change_info"] is True
        assert record["rights"]["invite_users"] is True
        assert record["rights"]["add_admins"] is False
    else:
        assert record["rights"] == {}
    assert len(client.requests) == 1


@pytest.mark.asyncio
async def test_basic_group_status_refuses_channel_rpc(wire_client):
    client = wire_client(
        admin_rights,
        StatusClient(None),
        entity=types.Chat(
            id=7, title="Basic", photo=None, participants_count=1, date=None, version=1
        ),
    )
    tool = getattr(admin_rights, "get_member_admin_status", None)
    assert callable(tool)
    result = await tool(7, 42, account="default")
    assert "basic groups" in result.lower()
    assert client.requests == []
