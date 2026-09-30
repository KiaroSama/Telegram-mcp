"""`export_secret_chat`: a secret chat's auto-saved messages exported as Telegram Desktop
would export that 1:1 chat (spec 031 FR-003b).

The rendering is `tdexport.secret_saved` (its own suite); the dialog is the same one
`export_chat_history` shows (`export_dialog`). What is checked here is the routing: saved
records and both users reach the exporter, a chat with nothing saved asks nothing, a
deleted chat needs its peer named, and the safeguard asks first.
"""

from types import SimpleNamespace

import pytest
from telethon.tl import types

from telegram_mcp import export_jobs
from telegram_mcp.safeguard import policy
from telegram_mcp.tdexport.fetch import ExportResult
from telegram_mcp.tdexport.settings import Format
from telegram_mcp.tools import secret_autosave as mod

from secret_fakes import CHAT_ID, SECRET_ID

ME = types.User(id=1, first_name="Me")
PEER = types.User(id=5001, first_name="Peer", access_hash=9)
RECORDS = [{"v": 1, "type": "message", "id": 77, "date": 1, "out": True, "text": "hi"}]
OPTIONS = dict(
    format="json",
    photos=True,
    videos=False,
    voice_messages=False,
    video_messages=False,
    stickers=False,
    gifs=False,
    files=False,
    size_limit_mb=8,
)


class Client:
    async def get_me(self):
        return ME

    async def get_entity(self, user_id):
        assert user_id == PEER.id
        return PEER

    async def __call__(self, request):
        return SimpleNamespace(me_url_prefix="https://t.me/")


@pytest.fixture
def exported(backend, wire_client, monkeypatch):
    wire_client(mod, Client())
    runs = []

    def _export(records, settings, writer, self_user, peer_user, environment=None):
        runs.append(SimpleNamespace(**locals()))
        return ExportResult(settings.path + "ChatExport_2026-09-30/", False, "", 1, 0)

    monkeypatch.setattr(mod, "export_saved_chat", _export)
    monkeypatch.setattr(mod.export_dialog, "writer_for", lambda fmt: f"writer:{fmt.name}")
    return SimpleNamespace(manager=backend, runs=runs)


def test_the_safeguard_asks_first():
    assert "export_secret_chat" in policy.GATED


@pytest.mark.asyncio
async def test_saved_records_and_both_users_reach_the_exporter(exported):
    exported.manager.saved_messages[SECRET_ID] = RECORDS

    answer = await mod.export_secret_chat(CHAT_ID, **OPTIONS)

    await export_jobs.settle()
    run = exported.runs[0]
    assert run.records == RECORDS and run.self_user is ME and run.peer_user is PEER
    assert run.settings.format is Format.Json and run.writer == "writer:Json"
    assert run.environment.internal_links_domain == "https://t.me/"
    assert '"status": "running"' in answer
    assert '"messages": 1' in await export_jobs_status()


@pytest.mark.asyncio
async def test_nothing_saved_asks_nothing(exported):
    said = await mod.export_secret_chat(CHAT_ID, **OPTIONS)

    assert "Nothing was auto-saved" in said and exported.runs == []


@pytest.mark.asyncio
async def test_a_deleted_chat_needs_its_peer_named(exported):
    exported.manager.saved_messages[424242] = RECORDS

    said = await mod.export_secret_chat(424242, **OPTIONS)
    assert "peer_user" in said and exported.runs == []

    await mod.export_secret_chat(424242, peer_user=PEER.id, **OPTIONS)
    await export_jobs.settle()
    assert exported.runs[0].peer_user is PEER


@pytest.mark.asyncio
async def test_the_choices_are_the_owners(exported):
    exported.manager.saved_messages[SECRET_ID] = RECORDS

    said = await mod.export_secret_chat(CHAT_ID, format="html")

    assert "Ask the owner" in said and exported.runs == []


async def export_jobs_status():
    from telegram_mcp.tools import chat_export

    return await chat_export.export_status()
