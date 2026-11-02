"""Admin patch changes only named rights through existing public editors."""

from types import SimpleNamespace

import pytest
from telethon import functions, types
from telegram_mcp.tools import admin_rights_by_type as mod


@pytest.mark.asyncio
async def test_patch_preserves_unmentioned_rights_and_title(wire_client):
    chat = types.Channel(
        id=7, title="Group", photo=types.ChatPhotoEmpty(), date=None, megagroup=True
    )
    user = types.User(id=8, first_name="Admin", access_hash=1)
    current = types.ChatAdminRights(delete_messages=True, ban_users=True, other=True)

    class Client:
        def __init__(self):
            self.writes = []

        async def __call__(self, request):
            if isinstance(request, functions.channels.GetParticipantRequest):
                return SimpleNamespace(
                    participant=SimpleNamespace(admin_rights=current, rank="Keeper")
                )
            assert isinstance(request, functions.channels.EditAdminRequest)
            self.writes.append(request)
            for name, value in request.admin_rights.to_dict().items():
                if name != "_":
                    setattr(current, name, value)
            return SimpleNamespace()

    async def resolve(identifier, *_):
        return chat if identifier == 7 else user

    client = wire_client(mod, Client(), resolve=resolve)
    answer = await mod.edit_admin_rights_group(
        7, 8, delete_messages=False, mode="patch", account="default"
    )
    assert "updated" in answer.lower(), answer
    sent = client.writes[0]
    assert sent.admin_rights.delete_messages is False
    assert sent.admin_rights.ban_users is True
    assert sent.admin_rights.other is True
    assert sent.rank == "Keeper"
    answer = await mod.edit_admin_rights_group(
        7, 8, delete_messages=False, mode="patch", account="default"
    )
    assert "unchanged" in answer.lower(), answer
    assert len(client.writes) == 1


@pytest.mark.asyncio
async def test_patch_refuses_demotion_nonadmin_and_noncreator_self(wire_client):
    chat = types.Channel(
        id=7, title="Group", photo=types.ChatPhotoEmpty(), date=None, megagroup=True
    )
    user = types.User(id=8, first_name="Admin", access_hash=1)
    state = SimpleNamespace(
        admin_rights=types.ChatAdminRights(delete_messages=True), rank="Keeper"
    )

    class Client:
        writes = []

        async def __call__(self, request):
            if isinstance(request, functions.channels.GetParticipantRequest):
                return SimpleNamespace(participant=state)
            self.writes.append(request)
            raise AssertionError("A refused patch must not write")

    async def resolve(identifier, *_):
        return chat if identifier == 7 else user

    client = wire_client(mod, Client(), resolve=resolve)
    answer = await mod.edit_admin_rights_group(
        7, 8, delete_messages=False, mode="patch", account="default"
    )
    assert "demote_admin" in answer
    state.admin_rights = None
    answer = await mod.edit_admin_rights_group(
        7, 8, delete_messages=True, mode="patch", account="default"
    )
    assert "no admin rights" in answer
    state.admin_rights = types.ChatAdminRights(delete_messages=True)
    state.is_self = True
    answer = await mod.edit_admin_rights_group(
        7, 8, ban_users=True, mode="patch", account="default"
    )
    assert "creator" in answer
    assert client.writes == []
    state.is_self = False
    state = types.ChannelParticipantCreator(
        user_id=8, admin_rights=types.ChatAdminRights(delete_messages=True), rank="Owner"
    )
    user = types.InputUserSelf()
    answer = await mod.edit_admin_rights_group(
        7, 8, ban_users=True, mode="patch", account="default"
    )
    assert "only their own anonymous" in answer
    assert client.writes == []
