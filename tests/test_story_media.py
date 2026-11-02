"""Story tools exercise the actual preview boundary without a view signal."""

from datetime import datetime, timedelta, timezone
from types import SimpleNamespace
import json

import pytest
from telethon import functions, types


@pytest.mark.asyncio
async def test_story_preview_returns_image_without_view_signal(wire_client):
    from telegram_mcp.tools import story_preview as mod

    class Client:
        def __init__(self):
            self.requests = []

        async def __call__(self, request):
            self.requests.append(request)
            now = datetime.now(timezone.utc)
            return SimpleNamespace(
                stories=[
                    types.StoryItem(
                        id=7,
                        date=now,
                        expire_date=now + timedelta(hours=1),
                        media=types.MessageMediaPhoto(
                            photo=types.Photo(
                                id=1,
                                access_hash=2,
                                file_reference=b"fixture",
                                date=now,
                                sizes=[types.PhotoSize(type="x", w=2, h=2, size=68)],
                                dc_id=1,
                            )
                        ),
                    )
                ]
            )

        def iter_download(self, _target):
            from io import BytesIO
            from PIL import Image

            buf = BytesIO()
            Image.new("RGB", (2, 2), "blue").save(buf, "PNG")

            async def chunks():
                yield buf.getvalue()

            return chunks()

    client = Client()
    wire_client(mod, client)
    result = await mod.preview_story("peer", 7, account="default")
    assert isinstance(result, list), result
    assert len(result) == 2
    metadata = json.loads(result[0])
    assert metadata["story_id"] == 7
    assert metadata["seen_sent"] is False
    assert result[1].data
    assert len(client.requests) == 1
    assert isinstance(client.requests[0], functions.stories.GetStoriesByIDRequest)


@pytest.mark.asyncio
@pytest.mark.parametrize("identifier", [0, -1, True])
async def test_invalid_story_id_fetches_nothing(identifier):
    from telegram_mcp.story_media import fetch_story_media

    class Client:
        async def __call__(self, request):
            raise AssertionError("Invalid ID reached Telegram")

    with pytest.raises(ValueError, match="positive integer"):
        await fetch_story_media(Client(), types.InputPeerSelf(), identifier)


@pytest.mark.asyncio
async def test_deleted_story_refuses_without_transfer():
    from telegram_mcp.story_media import fetch_story_media

    class Client:
        async def __call__(self, request):
            return SimpleNamespace(stories=[types.StoryItemDeleted(id=7)])

    with pytest.raises(ValueError, match="unavailable"):
        await fetch_story_media(Client(), types.InputPeerSelf(), 7)


@pytest.mark.asyncio
async def test_story_download_reuses_no_overwrite_transaction(wire_client, monkeypatch, tmp_path):
    from pathlib import Path
    from telegram_mcp import file_roots
    from telegram_mcp.tools import media as mod

    now = datetime.now(timezone.utc)

    class Client:
        async def __call__(self, request):
            assert isinstance(request, functions.stories.GetStoriesByIDRequest)
            return SimpleNamespace(
                stories=[
                    types.StoryItem(
                        id=7,
                        date=now,
                        expire_date=now + timedelta(hours=1),
                        noforwards=True,
                        media=types.MessageMediaPhoto(
                            photo=types.Photo(
                                id=1,
                                access_hash=2,
                                file_reference=b"fixture",
                                date=now,
                                sizes=[types.PhotoSize(type="x", w=2, h=2, size=1)],
                                dc_id=1,
                            )
                        ),
                    )
                ]
            )

        async def download_media(self, msg, file, progress_callback):
            path = Path(file + ".jpg")
            path.write_bytes(b"story")
            progress_callback(5, 5)
            return str(path)

    wire_client(mod, Client())

    async def roots(ctx, tool_name):
        return [tmp_path], None

    async def destination(**kwargs):
        return tmp_path / "keep.jpg", None

    monkeypatch.setattr(file_roots, "_ensure_allowed_roots", roots)
    monkeypatch.setattr(mod, "_resolve_writable_file_path", destination)
    (tmp_path / "keep.jpg").write_bytes(b"original")
    result = await mod.download_media("me", story_id=7, account="default")
    assert "keep-1.jpg" in result, result
    assert (tmp_path / "keep.jpg").read_bytes() == b"original"
    assert (tmp_path / "keep-1.jpg").read_bytes() == b"story"
    assert sorted(p.name for p in tmp_path.iterdir()) == ["keep-1.jpg", "keep.jpg"]
