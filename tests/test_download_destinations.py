"""Configured Downloads roots do not change the private default destination."""

from pathlib import Path
from types import SimpleNamespace

import pytest
from telegram_mcp import file_roots
from telegram_mcp.tools import media


@pytest.mark.asyncio
async def test_default_download_is_private_and_never_nests_configured_downloads(
    tmp_path, monkeypatch, wire_client
):
    configured = tmp_path / "Downloads"
    configured.mkdir()
    private = tmp_path / "files" / "downloads"
    monkeypatch.setattr(file_roots, "SERVER_ALLOWED_ROOTS", [configured])
    monkeypatch.setattr(file_roots.folders, "files_dir", lambda: private.parent)

    class Client:
        async def get_messages(self, entity, ids):
            return SimpleNamespace(
                id=ids, media=object(), file=SimpleNamespace(size=7, ext=".png")
            )

        async def download_media(self, message, file, progress_callback):
            written = Path(file + ".png")
            written.write_bytes(b"fixture")
            return str(written)

    wire_client(media, Client())
    result = await media.download_media("me", 7, account="default")
    assert "Media downloaded" in result, result
    assert len(list(private.glob("*.png"))) == 1
    assert not (configured / "downloads").exists()
    assert list(configured.iterdir()) == []
