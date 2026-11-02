"""Caption modes reach the public upload boundary without changing defaults."""

import pytest
from telegram_mcp import file_roots
from telegram_mcp.tools import media


@pytest.mark.asyncio
async def test_caption_modes_preserve_uploads_and_refuse_conflicts(
    tmp_path, monkeypatch, wire_client
):
    root = tmp_path.resolve()
    for name in ("one.png", "two.png"):
        (root / name).write_bytes(b"synthetic-image")
    calls = []

    class Client:
        async def send_file(self, entity, files, **flags):
            handles = files if isinstance(files, list) else [files]
            assert all(handle.read() == b"synthetic-image" for handle in handles)
            calls.append(flags)

    wire_client(media, Client())
    monkeypatch.setattr(file_roots, "SERVER_ALLOWED_ROOTS", [root])
    monkeypatch.setattr(file_roots, "_relative_base", lambda: root)
    await media.send_file("me", "one.png", caption="**bold**", parse_mode="md", account="default")
    assert calls[-1]["parse_mode"] == "md"
    await media.send_album(
        "me", ["one.png", "two.png"], caption="<b>bold</b>", parse_mode="html", account="default"
    )
    assert calls[-1]["parse_mode"] == "html"
    await media.send_file(
        "me", ["one.png", "two.png"], caption="**literal**", parse_mode="plain", account="default"
    )
    assert "parse_mode" in calls[-1] and calls[-1]["parse_mode"] is None
    await media.send_file("me", "one.png", caption="legacy", account="default")
    assert "parse_mode" not in calls[-1]
    before = len(calls)
    for mode in ("invalid", "md"):
        result = await media.send_file(
            "me",
            "one.png",
            caption="x",
            parse_mode=mode,
            caption_entities=[{"type": "bold", "offset": 0, "length": 1}],
            account="default",
        )
        assert "Nothing was sent" in result
    assert len(calls) == before
