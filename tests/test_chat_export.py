"""`export_chat_history`: one chat's history as a JSON-lines archive under an allowed root.

Layout (owner's decision, plan 017): `<folder>/<chat id>/messages.jsonl` plus
`<folder>/<chat id>/media/<message id>_<name>`, default folder
`files/downloads/exports`. The folder is judged BEFORE any network call, and media
goes through `download_media`, so its byte cap, suffix rule and handle-bound
install apply unchanged.

The installation folder is the test's own (conftest `_folders_are_the_tests_own`),
so the default `downloads/exports` lands in a temporary `files/downloads`.
"""

import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from telethon.tl import types
from telethon.tl.custom.message import Message

from telegram_mcp.safeguard import folders
from telegram_mcp.tools import chat_export as mod
from telegram_mcp.tools import media as media_mod

CHANNEL = types.Channel(id=555, title="News", photo=None, date=None, broadcast=True)
WHEN = datetime(2026, 9, 1, tzinfo=timezone.utc)


def _message(msg_id, text="", media=None):
    msg = Message(
        id=msg_id,
        peer_id=types.PeerChannel(channel_id=555),
        date=WHEN,
        message=text,
        media=media,
    )
    return msg


def _photo():
    return types.MessageMediaPhoto(
        photo=types.Photo(
            id=9,
            access_hash=1,
            file_reference=b"",
            date=WHEN,
            sizes=[types.PhotoSize(type="x", w=10, h=10, size=64)],
            dc_id=2,
        )
    )


class FakeClient:
    """Newest first, as Telegram answers; `download_media` writes what Telethon would."""

    def __init__(self, messages):
        self.messages = messages
        self.limits = []
        self.downloads = 0

    async def __call__(self, request):  # pragma: no cover - no raw request is expected
        raise AssertionError(f"unexpected request {request!r}")

    async def iter_messages(self, entity, limit=None):
        self.limits.append(limit)
        for msg in self.messages[:limit]:
            yield msg

    async def get_messages(self, entity, ids=None):
        return next(m for m in self.messages if m.id == ids)

    async def download_media(self, msg, file=None, progress_callback=None):
        self.downloads += 1
        target = Path(f"{file}.jpg")
        target.write_bytes(b"\xff\xd8" + b"x" * 62)
        return str(target)


@pytest.fixture
def wired(wire_client, monkeypatch):
    def _wire(messages):
        client = FakeClient(messages)
        wire_client(mod, client, entity=CHANNEL)
        wire_client(media_mod, client, entity=CHANNEL)
        return client

    return _wire


def _archive():
    chat_dir = folders.files_dir() / "downloads" / "exports" / "-1000000000555"
    lines = (chat_dir / "messages.jsonl").read_text(encoding="utf-8").splitlines()
    return chat_dir, [json.loads(line) for line in lines]


@pytest.mark.asyncio
async def test_writes_one_json_object_per_message_oldest_first(wired):
    client = wired([_message(3, "third"), _message(2, ""), _message(1, "first")])

    result = await mod.export_chat_history("@news")

    chat_dir, records = _archive()
    assert [r["id"] for r in records] == [1, 2, 3], "an archive reads oldest first"
    assert records[0]["text"] == "first"
    assert (
        "text" not in records[1] or not records[1]["text"]
    ), "a message with no text still exports"
    assert client.limits == [1000]
    summary = json.loads(result)
    assert summary["results"][0]["messages"] == 3
    assert summary["results"][0]["truncated"] is False
    assert not (chat_dir / "media").exists(), "media is opt-in"
    assert client.downloads == 0


@pytest.mark.asyncio
async def test_the_limit_is_reported_as_truncation(wired):
    wired([_message(i, f"m{i}") for i in range(5, 0, -1)])

    result = await mod.export_chat_history("@news", limit=2)

    _, records = _archive()
    assert [r["id"] for r in records] == [4, 5], "the most recent `limit` messages"
    assert json.loads(result)["results"][0]["truncated"] is True


@pytest.mark.parametrize("limit", [0, -5, "lots"])
@pytest.mark.asyncio
async def test_a_bad_limit_is_refused_before_any_request(wired, limit):
    client = wired([_message(1, "x")])

    result = await mod.export_chat_history("@news", limit=limit)

    assert client.limits == []
    assert "limit" in result


@pytest.mark.asyncio
async def test_a_limit_past_the_ceiling_is_served_at_the_ceiling(wired):
    client = wired([_message(1, "x")])

    await mod.export_chat_history("@news", limit=10**9)

    assert client.limits == [mod.EXPORT_CEILING]


@pytest.mark.asyncio
async def test_media_opt_in_downloads_through_the_download_path(wired):
    client = wired([_message(2, "caption", media=_photo()), _message(1, "plain")])

    result = await mod.export_chat_history("@news", media=True)

    chat_dir, records = _archive()
    assert client.downloads == 1, "only the message with media is downloaded"
    saved = [p.name for p in (chat_dir / "media").iterdir()]
    assert saved == ["2.jpg"]
    assert records[1]["media_file"] == "media/2.jpg"
    assert "media_file" not in records[0]
    assert json.loads(result)["results"][0]["media_saved"] == 1


@pytest.mark.asyncio
async def test_a_folder_outside_every_root_is_refused_before_any_network_call(
    wired, tmp_path, monkeypatch
):
    client = wired([_message(1, "x")])

    def _no_client(account=None):
        raise AssertionError("the client was reached before the folder was judged")

    monkeypatch.setattr(mod, "get_client", _no_client)

    result = await mod.export_chat_history("@news", destination=str(tmp_path / "elsewhere"))

    assert "outside" in result.lower()
    assert client.limits == []
    assert not (tmp_path / "elsewhere").exists()


@pytest.mark.asyncio
async def test_a_path_trick_is_refused(wired):
    client = wired([_message(1, "x")])

    result = await mod.export_chat_history("@news", destination="downloads/*")

    assert client.limits == []
    assert result
