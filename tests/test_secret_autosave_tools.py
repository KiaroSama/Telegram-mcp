"""Secret-chat auto-save and delete tools over package 98c366e (spec 031).

The package does the saving and deleting; these tools only route to it, keep the folder
inside the allowed roots, and clear this server's own copies of a deleted chat. The
safeguard asks the owner before every one of them (owner, 2026-09-30).
"""

import json

import pytest

from telegram_mcp import secret_history, secret_media_refs
from telegram_mcp.safeguard import policy
from telegram_mcp.tools import secret_autosave as mod

from secret_fakes import CHAT_ID, SECRET_ID

TOOLS = (
    "start_secret_auto_save",
    "stop_secret_auto_save",
    "delete_secret_chat",
    "delete_secret_chat_both_sides",
    "delete_saved_secret_messages",
)


def _result(raw):
    return json.loads(raw)["results"]


@pytest.fixture
def allowed(monkeypatch, tmp_path):
    async def resolve(folder, ctx, tool_name):
        if folder.startswith("outside"):
            return None, f"{folder} is outside the allowed folders."
        return tmp_path / folder, None

    monkeypatch.setattr(mod.file_roots, "resolve_allowed_folder", resolve)
    return tmp_path


@pytest.mark.parametrize("tool", TOOLS)
def test_the_safeguard_asks_before_every_one(tool):
    assert tool in policy.GATED


@pytest.mark.asyncio
async def test_auto_save_starts_in_an_allowed_folder(backend, allowed):
    answer = _result(await mod.start_secret_auto_save("saved/secret", account="acct"))

    assert answer["auto_save"] is True
    assert backend.auto_save_secret_chats == str(allowed / "saved" / "secret")
    assert (allowed / "saved" / "secret").is_dir()


@pytest.mark.asyncio
async def test_a_folder_outside_the_roots_starts_nothing(backend, allowed):
    said = await mod.start_secret_auto_save("outside/x", account="acct")

    assert "outside the allowed folders" in said
    assert backend.auto_save_secret_chats is None


@pytest.mark.asyncio
async def test_auto_save_stops_and_says_what_stays(backend, allowed):
    backend.auto_save_secret_chats = str(allowed)

    answer = _result(await mod.stop_secret_auto_save(account="acct"))

    assert answer["auto_save"] is False and backend.auto_save_secret_chats is None
    assert "delete_saved_secret_messages" in answer["note"]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "tool, how", [("delete_secret_chat", "this side"), ("delete_secret_chat_both_sides", "both")]
)
async def test_deleting_a_chat_clears_this_servers_copies_too(backend, tool, how):
    secret_history.record("acct", SECRET_ID, {"message_id": "1", "text": "x"})
    secret_media_refs.remember("acct", SECRET_ID, 1, {"version": 1}, 0)

    answer = _result(await getattr(mod, tool)(CHAT_ID, account="acct"))

    assert answer["deleted"] is True and backend.deleted_chats[-1][0] == SECRET_ID
    assert how in backend.deleted_chats[-1][1]
    assert secret_history.read("acct", SECRET_ID, 10) == []
    assert secret_media_refs.load("acct", SECRET_ID, 1) is None


@pytest.mark.asyncio
async def test_both_sides_reports_when_telegram_could_not_reach_the_peer(backend):
    backend.both_sides_reached = False

    answer = _result(await mod.delete_secret_chat_both_sides(CHAT_ID, account="acct"))

    assert answer["deleted"] is True and answer["peer_history_deleted"] is False


@pytest.mark.asyncio
async def test_saved_messages_are_deleted_on_request_only(backend):
    backend.saved_messages[SECRET_ID] = [{"type": "message", "id": 1}]

    answer = _result(await mod.delete_saved_secret_messages(CHAT_ID, account="acct"))

    assert answer["deleted"] is True and backend.deleted_saved == [SECRET_ID]


@pytest.mark.asyncio
async def test_an_unknown_chat_is_named(backend):
    said = await mod.delete_secret_chat(424242, account="acct")

    assert "No secret chat" in said and backend.deleted_chats == []


@pytest.mark.asyncio
async def test_the_status_says_whether_auto_save_is_on_and_where(backend):
    from telegram_mcp.tools import secret_chats as sc

    off = _result(await sc.secret_chat_status(account="acct"))
    backend.auto_save_secret_chats = "D:/saved"
    on = _result(await sc.secret_chat_status(account="acct"))

    assert off["auto_save"] is None and on["auto_save"] == "D:/saved"
