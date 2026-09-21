"""Getting the BYTES out of a rich message.

Its own file because it is its own job, and its own backend. A rich message's blocks
NAME their photos and documents; fetching one is a second act with a second failure
mode, and `download_rich_media` is still the one tool here that runs on TDLib -
`read_rich_message` moved onto plain Telethon on 2026-09-21 and its tests went with it,
leaving these behind as the only reason the TDLib wire is still built in this suite.

Splitting it out was not bookkeeping: `tests/test_rich_messages.py` crossed the 800-line
ceiling when the Telethon wire was added beside the TDLib one, and two backends in one
file is exactly the responsibility the ceiling is asking about.
"""

import json

import pytest

from telegram_mcp.tools import rich_messages as rm


class FakeTDLib:
    def __init__(self, message=None, error=None, downloaded=None):
        self.requests = []
        self.message = message
        self.error = error
        self.downloaded = downloaded

    async def request(self, obj, timeout=30.0):
        self.requests.append(obj)
        if obj["@type"] == "getChat":
            return {"@type": "chat", "id": obj["chat_id"]}
        if self.error:
            raise self.error
        if obj["@type"] == "downloadFile":
            return self.downloaded
        return self.message

    def types(self):
        return [r["@type"] for r in self.requests]


@pytest.fixture
def wire(monkeypatch):
    def _wire(message=None, error=None, downloaded=None):
        client = FakeTDLib(message, error, downloaded)
        monkeypatch.setattr(rm, "account_label", lambda account=None: "acct")

        async def _client(label):
            return client

        monkeypatch.setattr(rm, "secret_client", _client)
        return client

    return _wire


def _results(raw):
    return json.loads(raw)["results"]


# --------------------------------------------------------------------------
# Getting the bytes out of a rich message
# --------------------------------------------------------------------------


def _photo_message(file_obj):
    return {
        "content": {
            "@type": "messageRichMessage",
            "message": {
                "blocks": [{"@type": "pageBlockPhoto", "photo": {"sizes": [{"photo": file_obj}]}}]
            },
        }
    }


def test_a_photo_blocks_file_is_found_under_its_last_size():
    """A photo has no file of its own - the sizes ARE the picture - so indexing
    the block key the way every other kind does returns nothing."""
    handle = {"id": 7, "size": 99}
    block = {
        "@type": "pageBlockPhoto",
        "photo": {"sizes": [{"photo": {"id": 1}}, {"photo": handle}]},
    }

    assert rm._block_file(block) is handle


def test_a_voice_notes_file_is_not_under_the_block_key():
    """TDLib calls it `voice`, not `voice_note`. Deriving the inner key from the
    block key silently returned None for the one kind that differs."""
    handle = {"id": 3}
    block = {"@type": "pageBlockVoiceNote", "voice_note": {"voice": handle}}

    assert rm._block_file(block) is handle


def test_the_reader_publishes_the_file_id_so_it_can_be_fetched():
    """It used to report a photo's width and height and drop the handle, leaving
    a caller able to see the picture existed and unable to ask for it."""
    block = {"@type": "pageBlockPhoto", "photo": {"sizes": [{"photo": {"id": 42}, "width": 8}]}}

    assert rm._render_block(block)["media"]["file_id"] == 42


def test_a_half_downloaded_file_reports_no_path():
    """`path` is set while a transfer is still running, so trusting it without
    `is_downloading_completed` hands back a partial file."""
    partial = {"id": 1, "local": {"path": "C:/half.jpg", "is_downloading_completed": False}}
    whole = {"id": 2, "local": {"path": "C:/whole.jpg", "is_downloading_completed": True}}

    assert (
        "local_path"
        not in rm._render_block(
            {"@type": "pageBlockPhoto", "photo": {"sizes": [{"photo": partial}]}}
        )["media"]
    )
    assert (
        rm._render_block({"@type": "pageBlockPhoto", "photo": {"sizes": [{"photo": whole}]}})[
            "media"
        ]["local_path"]
        == "C:/whole.jpg"
    )


@pytest.mark.asyncio
async def test_a_file_tdlib_already_holds_is_not_downloaded_again(wire):
    cached = {
        "id": 5,
        "size": 12,
        "local": {"path": "C:/have.jpg", "is_downloading_completed": True},
    }
    client = wire(_photo_message(cached))

    answer = _results(await rm.download_rich_media(-100123, 970, account="acct"))

    assert answer["path"] == "C:/have.jpg" and answer["file_id"] == 5
    assert "downloadFile" not in client.types(), "asked for bytes it already had"


@pytest.mark.asyncio
async def test_a_file_tdlib_lacks_is_fetched_and_its_path_returned(wire):
    client = wire(
        _photo_message({"id": 9, "local": {"is_downloading_completed": False}}),
        downloaded={"size": 77, "local": {"path": "C:/got.jpg", "is_downloading_completed": True}},
    )

    answer = _results(await rm.download_rich_media(-100123, 970, account="acct"))

    assert answer["path"] == "C:/got.jpg" and answer["size_bytes"] == 77
    assert "downloadFile" in client.types()


@pytest.mark.asyncio
async def test_an_unfinished_transfer_is_reported_rather_than_returned(wire):
    """A path from an incomplete download is a truncated file wearing the name
    of a whole one."""
    wire(
        _photo_message({"id": 9, "local": {"is_downloading_completed": False}}),
        downloaded={"local": {"path": "C:/partial.jpg", "is_downloading_completed": False}},
    )

    answer = _results(await rm.download_rich_media(-100123, 970, account="acct"))

    assert answer["saved"] is False and "timeout" in answer["reason"]


@pytest.mark.asyncio
async def test_a_block_without_media_says_so_instead_of_failing(wire):
    wire(
        {
            "content": {
                "@type": "messageRichMessage",
                "message": {"blocks": [{"@type": "pageBlockDivider"}]},
            }
        }
    )

    assert "carries media" in await rm.download_rich_media(-100123, 970, account="acct")
