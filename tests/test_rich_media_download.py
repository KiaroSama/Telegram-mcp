"""Getting the BYTES out of a rich message.

Its own file because it is its own job: a rich message's blocks NAME their photos and
documents rather than carrying them, so fetching one is a second act with a second
failure mode. Splitting it out was not bookkeeping — `tests/test_rich_messages.py`
crossed the 800-line ceiling, and "read the blocks" and "fetch what a block points at"
are two responsibilities.

What the rewire onto plain MTProto changed, and what these pin:

* the block's pointer and the file itself arrive SEPARATELY — `photo_id` on the block,
  the photo in the message's own `photos` list — so the two are matched here, and a
  pointer with nothing to match is reported rather than silently returning nothing;
* the bytes land under the operator's allowed roots through the project's own guard,
  where the previous backend returned a path inside its private database directory.
"""

import json
from types import SimpleNamespace

import pytest
from telethon import errors
from telethon.tl import types

from telegram_mcp.tools import rich_messages as rm


def _caption(text=""):
    node = types.TextPlain(text) if text else types.TextEmpty()
    return types.PageCaption(node, types.TextEmpty())


def _photo(identifier):
    return SimpleNamespace(id=identifier)


class FakeTelethon:
    """Records the fetch, and stands in for the download."""

    def __init__(self, rich=None, error=None, saved="/downloads/out.jpg"):
        self.requests = []
        self.downloads = []
        self.rich = rich
        self.error = error
        self.saved = saved

    async def get_input_entity(self, chat_id):
        return f"peer:{chat_id}"

    async def __call__(self, request):
        self.requests.append(request)
        if self.error:
            raise self.error
        message = SimpleNamespace(id=getattr(request, "id", 0), rich_message=self.rich)
        return SimpleNamespace(messages=[message], chats=[], users=[])

    async def download_media(self, handle, file=None):
        self.downloads.append((handle, file))
        return self.saved


@pytest.fixture
def wire(monkeypatch, tmp_path):
    def _wire(rich=None, error=None, saved=None):
        # `is None`, not `or`: a test that asks for an empty path is asking for the
        # "transfer produced nothing" case, and `or` would hand it the default.
        default = str(tmp_path / "out.jpg")
        client = FakeTelethon(rich, error, default if saved is None else saved)
        monkeypatch.setattr(rm, "get_client", lambda account=None: client)

        async def _connected(cl):
            return None

        async def _resolve(**kwargs):
            return tmp_path / kwargs["default_filename"], None

        monkeypatch.setattr(rm, "ensure_connected", _connected)
        monkeypatch.setattr(rm, "_resolve_writable_file_path", _resolve)
        return client

    return _wire


def _results(raw):
    return json.loads(raw)["results"]


def _message(*blocks, photos=(), documents=()):
    return types.RichMessage(
        blocks=list(blocks), photos=list(photos), documents=list(documents), rtl=False
    )


@pytest.mark.asyncio
async def test_the_first_block_carrying_media_is_taken_when_none_is_named(wire):
    """`block_index` is optional, and omitting it must not mean "the first block" —
    a message whose table comes before its photo would download nothing."""
    rich = _message(
        types.PageBlockParagraph(types.TextPlain("words")),
        types.PageBlockPhoto(photo_id=7, caption=_caption()),
        photos=[_photo(7)],
    )
    client = wire(rich)

    answer = _results(await rm.download_rich_media(-100123, 970, account="acct"))

    assert answer["saved"] is True
    assert answer["block_index"] == 1, "the paragraph is not the block with the photo"
    assert answer["file_id"] == "7"
    assert client.downloads, "nothing was actually fetched"


@pytest.mark.asyncio
async def test_a_named_block_is_the_one_fetched(wire):
    rich = _message(
        types.PageBlockPhoto(photo_id=1, caption=_caption()),
        types.PageBlockPhoto(photo_id=2, caption=_caption()),
        photos=[_photo(1), _photo(2)],
    )
    wire(rich)

    answer = _results(await rm.download_rich_media(-100123, 970, 1, account="acct"))

    assert answer["block_index"] == 1 and answer["file_id"] == "2"


@pytest.mark.asyncio
async def test_a_block_index_outside_the_message_says_how_many_there_are(wire):
    wire(_message(types.PageBlockPhoto(photo_id=1, caption=_caption()), photos=[_photo(1)]))

    answer = await rm.download_rich_media(-100123, 970, 9, account="acct")

    assert "outside this message's 1 blocks" in answer


@pytest.mark.asyncio
async def test_a_block_without_media_says_so_instead_of_failing(wire):
    wire(_message(types.PageBlockParagraph(types.TextPlain("just words"))))

    answer = await rm.download_rich_media(-100123, 970, 0, account="acct")

    assert "carries no media" in answer


@pytest.mark.asyncio
async def test_a_message_with_no_media_at_all_says_so(wire):
    wire(_message(types.PageBlockParagraph(types.TextPlain("just words"))))

    assert "No block in this message carries media" in await rm.download_rich_media(
        -100123, 970, account="acct"
    )


@pytest.mark.asyncio
async def test_a_pointer_with_nothing_to_match_is_reported_not_swallowed(wire):
    """The block names a file the message did not carry. Returning "saved: false" with
    no reason, or an empty path, would read as a transfer problem rather than an
    incomplete message."""
    wire(_message(types.PageBlockPhoto(photo_id=42, caption=_caption()), photos=[]))

    answer = await rm.download_rich_media(-100123, 970, account="acct")

    assert "names file 42" in answer and "did not carry" in answer


@pytest.mark.asyncio
async def test_an_ordinary_message_is_sent_to_the_ordinary_tool(wire):
    wire(None)

    answer = await rm.download_rich_media(-100123, 970, account="acct")

    assert "not a rich message" in answer and "download_media" in answer


@pytest.mark.asyncio
async def test_a_transfer_that_produced_no_file_is_reported_rather_than_claimed(wire):
    wire(
        _message(types.PageBlockPhoto(photo_id=7, caption=_caption()), photos=[_photo(7)]),
        saved="",
    )

    answer = _results(await rm.download_rich_media(-100123, 970, account="acct"))

    assert answer["saved"] is False and "produced no file" in answer["reason"]


@pytest.mark.asyncio
async def test_telegrams_refusal_is_shown_not_filed_under_a_code(wire):
    wire(error=errors.RPCError(request=None, message="MESSAGE_ID_INVALID", code=400))

    assert "MESSAGE_ID_INVALID" in await rm.download_rich_media(-100123, 970, account="acct")
