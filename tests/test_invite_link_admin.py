"""Invite links as Telegram's Invite links screen shows them (spec 017), on fake clients.

What these pin: every link names its creator; any admin's links can be listed; the
admins who made links are listed with their counts; who joined through a link; a
paid link carries a 30-day price and is refused where Telegram refuses it; a QR code
is a real PNG of the link; and revoked links are deleted for one admin.
"""

import json
from datetime import datetime, timezone

import pytest
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tools import invite_link_admin as mod
from telegram_mcp.tools import invite_links

WHEN = datetime(2026, 9, 27, tzinfo=timezone.utc)
CHANNEL = tl.Channel(
    id=777, title="Chan", photo=tl.ChatPhotoEmpty(), date=None, broadcast=True, access_hash=7
)
GROUP = tl.Channel(
    id=778, title="Group", photo=tl.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=8
)
SARA = tl.User(id=42, first_name="Sara", access_hash=4)
ALI = tl.User(id=43, first_name="Ali", access_hash=5, deleted=False)


def _invite(link="https://t.me/+abc", admin=42, **kw):
    return tl.ChatInviteExported(link=link, admin_id=admin, date=WHEN, **kw)


class _Client:
    def __init__(self, answers):
        self.answers = answers
        self.sent = []

    async def __call__(self, request):
        self.sent.append(request)
        answer = self.answers.get(type(request).__name__)
        if isinstance(answer, Exception):
            raise answer
        return answer

    def of(self, kind):
        return [r for r in self.sent if isinstance(r, kind)]


@pytest.fixture
def wire(monkeypatch):
    holder = {}

    def use(answers=None, entity=GROUP):
        holder["c"] = _Client(answers or {})

        async def _resolve(value, cl=None, account=None):
            return {"sara": SARA, "ali": ALI}.get(value, entity)

        for module in (mod, invite_links):
            monkeypatch.setattr(module, "get_client", lambda account=None: holder["c"])
            monkeypatch.setattr(module, "resolve_entity", _resolve)
            monkeypatch.setattr(module, "ensure_connected", _connected)
        return holder["c"]

    async def _connected(cl=None):
        return None

    return use


@pytest.mark.asyncio
async def test_every_link_names_its_creator_and_any_admins_links_can_be_listed(wire):
    answer = tl.messages.ExportedChatInvites(count=1, invites=[_invite()], users=[SARA])
    c = wire({"GetExportedChatInvitesRequest": answer})
    payload = json.loads(await invite_links.list_invite_links("chat", admin="sara"))
    (row,) = payload["results"]
    assert row["created_by_id"] == 42 and row["created_by"] == "Sara"
    (request,) = c.of(functions.messages.GetExportedChatInvitesRequest)
    assert request.admin_id.user_id == 42


@pytest.mark.asyncio
async def test_admins_with_links_are_listed_with_their_counts(wire):
    answer = tl.messages.ChatAdminsWithInvites(
        admins=[tl.ChatAdminWithInvites(admin_id=42, invites_count=2, revoked_invites_count=1)],
        users=[SARA],
    )
    wire({"GetAdminsWithInvitesRequest": answer})
    payload = json.loads(await mod.list_invite_link_admins("chat"))
    assert payload["results"] == [
        {"admin_id": 42, "name": "Sara", "live_links": 2, "revoked_links": 1}
    ]


@pytest.mark.asyncio
async def test_who_joined_through_a_link(wire):
    importer = tl.ChatInviteImporter(user_id=43, date=WHEN, approved_by=42)
    answer = tl.messages.ChatInviteImporters(count=1, importers=[importer], users=[ALI, SARA])
    c = wire({"GetChatInviteImportersRequest": answer})
    payload = json.loads(await mod.list_link_joins("chat", "https://t.me/+abc"))
    (row,) = payload["results"]
    assert row["user_id"] == 43 and row["name"] == "Ali" and row["approved_by_id"] == 42
    assert row["joined_at"].startswith("2026-09-27")
    (request,) = c.of(functions.messages.GetChatInviteImportersRequest)
    assert request.link == "https://t.me/+abc" and not request.requested


@pytest.mark.asyncio
async def test_a_paid_link_costs_stars_every_30_days(wire):
    c = wire({"ExportChatInviteRequest": _invite()}, entity=CHANNEL)
    await mod.create_paid_invite_link("chat", monthly_fee_stars=50, title="VIP")
    (request,) = c.of(functions.messages.ExportChatInviteRequest)
    assert request.subscription_pricing.period == 2_592_000
    assert request.subscription_pricing.amount == 50 and request.title == "VIP"


@pytest.mark.asyncio
async def test_a_paid_link_is_refused_outside_a_channel_or_without_a_price(wire):
    c = wire(entity=GROUP)
    assert "channel" in (await mod.create_paid_invite_link("chat", monthly_fee_stars=50)).lower()
    wire(entity=CHANNEL)
    assert "at least 1" in await mod.create_paid_invite_link("chat", monthly_fee_stars=0)
    assert c.sent == []


@pytest.mark.asyncio
async def test_a_qr_code_is_a_png_of_the_link(wire, tmp_path, monkeypatch):
    wire()
    monkeypatch.setattr(mod, "_qr_directory", lambda: tmp_path)
    monkeypatch.setattr(mod, "_ensure_allowed_roots", _roots(tmp_path))
    text = await mod.get_invite_link_qr("https://t.me/+abc")
    (saved,) = list(tmp_path.glob("*.png"))
    assert saved.read_bytes().startswith(b"\x89PNG") and saved.name in text


@pytest.mark.asyncio
async def test_only_a_telegram_link_gets_a_qr_code(wire, tmp_path, monkeypatch):
    wire()
    monkeypatch.setattr(mod, "_qr_directory", lambda: tmp_path)
    assert "t.me" in await mod.get_invite_link_qr("https://evil.example/x")
    assert list(tmp_path.iterdir()) == []


@pytest.mark.asyncio
async def test_revoked_links_are_deleted_for_one_admin(wire):
    c = wire({"DeleteRevokedExportedChatInvitesRequest": True})
    await mod.delete_revoked_invite_links("chat", admin="sara")
    (request,) = c.of(functions.messages.DeleteRevokedExportedChatInvitesRequest)
    assert request.admin_id.user_id == 42


def _roots(path):
    async def _allowed(ctx, tool_name):
        return [path], None

    return _allowed


def test_the_bearer_note_travels_with_every_link_answer():
    assert "bearer" in invite_links._BEARER.lower()
