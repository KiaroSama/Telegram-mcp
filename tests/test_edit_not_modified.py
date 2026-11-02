"""An unchanged edit is an observed no-op, not an opaque failure."""

import json
import pytest
from telethon import errors
from telegram_mcp.tools import messages as mod


@pytest.mark.asyncio
async def test_unchanged_plain_edit_is_explicit(wire_client):
    class Client:
        async def edit_message(self, *args, **kwargs):
            raise errors.MessageNotModifiedError(request=None)

    wire_client(mod, Client())
    result = await mod.edit_message("me", 42, "same", account="default")
    assert "not modified" in result.lower(), result
    assert "GEN-ERR" not in result


@pytest.mark.asyncio
async def test_unchanged_rich_edit_is_explicit(wire_client, monkeypatch):
    class Client:
        async def __call__(self, request):
            raise errors.MessageNotModifiedError(request=request)

    async def premium(*_):
        return True

    wire_client(mod, Client())
    monkeypatch.setattr(mod, "account_is_premium", premium)
    result = json.loads(
        await mod.edit_message("me", 42, "same", parse_mode="rich", account="default")
    )
    assert result == {"sent": False, "rich": True, "edited_message_id": 42, "not_modified": True}
