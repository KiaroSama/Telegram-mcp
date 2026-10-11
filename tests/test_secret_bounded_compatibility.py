"""Bounded close and refusal preserve application copies."""

import json
from types import SimpleNamespace
import pytest


@pytest.mark.asyncio
async def test_bounded_close_reports_local_preservation_and_delete_refuses(backend, monkeypatch):
    from telegram_mcp import secret_backend, secret_history, secret_media_refs
    from telegram_mcp.tools import secret_chats, secret_autosave
    from secret_fakes import SECRET_ID

    backend.pause_status = lambda chat: SimpleNamespace(paused=True, resumable=False)
    raw = await secret_chats.close_secret_chat(SECRET_ID, account="acct")
    answer = json.loads(raw)["results"]
    assert "retained" in answer["note"] and "No peer erasure" in answer["note"]
    secret_history.record("acct", SECRET_ID, {"message_id": 1, "text": "kept"})
    secret_media_refs.remember("acct", SECRET_ID, 1, {"version": 1}, 0)

    async def refuse(chat):
        raise secret_backend.ChatPaused(chat_id=chat)

    backend.delete_secret_chat = refuse
    said = await secret_autosave.delete_secret_chat(SECRET_ID, account="acct")
    assert "paused" in said
    assert secret_history.read("acct", SECRET_ID, 1)[0]["text"] == "kept"
    assert secret_media_refs.load("acct", SECRET_ID, 1) is not None
