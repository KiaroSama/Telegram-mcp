"""Reading a table that an ordinary read reports as an empty message.

These drive `read_rich_message` end to end: the fetch, the identifier boundary, and
the answer it publishes. The RENDERING they used to test directly now lives in
`tests/test_rich_blocks.py`, which exercises it against the block objects themselves
rather than through a tool - one level down, and without a wire.

The fixture keeps the SHAPE of a real message: a two-column, three-row table with a
merged bottom cell, a captioned link and bold runs, captured from a live read of a
message that came back `[empty]`. Inventing a shape here would test the renderer
against an idea of the protocol rather than the protocol.
"""

import json
from types import SimpleNamespace

import pytest
from telethon import errors
from telethon.tl import types

from telegram_mcp.tools import rich_messages as rm


def _plain(text):
    return {"@type": "richTextPlain", "text": text}


def _cell(text, **kw):
    cell = {"@type": "pageTableCell", "text": text, "is_header": False}
    cell.update(kw)
    return cell


# The real message, reduced to what the renderer touches.
TABLE_MESSAGE = {
    "content": {
        "@type": "messageRichMessage",
        "message": {
            "@type": "richMessage",
            "is_rtl": False,
            "blocks": [
                {
                    "@type": "pageBlockTable",
                    "is_bordered": True,
                    "caption": {
                        "@type": "richTextUrl",
                        "text": _plain("ShaparakVPN | Services"),
                        "url": "https://t.me/shaparakvpn",
                    },
                    "cells": [
                        [
                            _cell(
                                {
                                    "@type": "richTexts",
                                    "texts": [
                                        {
                                            "@type": "richTextBold",
                                            "text": _plain("Chatgpt plus"),
                                        },
                                        _plain(" personal email"),
                                    ],
                                }
                            ),
                            _cell(_plain("v2ray residential")),
                        ],
                        [_cell(_plain("Gemini pro")), _cell(_plain("panel | multi"))],
                        [_cell(_plain("other subscriptions"), colspan=2)],
                    ],
                }
            ],
        },
    }
}


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
    """The TDLib wire, still used by `download_rich_media`."""

    def _wire(message=None, error=None, downloaded=None):
        client = FakeTDLib(message, error, downloaded)
        monkeypatch.setattr(rm, "account_label", lambda account=None: "acct")

        async def _client(label):
            return client

        monkeypatch.setattr(rm, "secret_client", _client)
        return client

    return _wire


# The same table as TABLE_MESSAGE, as MTProto hands it over. `read_rich_message`
# moved onto plain Telethon, so its tests drive this shape; the TDLib fixture above
# still serves `download_rich_media`, which has not moved yet.
def _tcell(text, **kw):
    return types.PageTableCell(text=text, **kw)


RICH_TABLE = types.RichMessage(
    blocks=[
        types.PageBlockTable(
            title=types.TextUrl(
                types.TextPlain("ShaparakVPN | Services"), "https://t.me/shaparakvpn", 0
            ),
            rows=[
                types.PageTableRow(
                    [
                        _tcell(
                            types.TextConcat(
                                [
                                    types.TextBold(types.TextPlain("Chatgpt plus")),
                                    types.TextPlain(" personal email"),
                                ]
                            )
                        ),
                        _tcell(types.TextPlain("v2ray residential")),
                    ]
                ),
                types.PageTableRow(
                    [
                        _tcell(types.TextPlain("Gemini pro")),
                        _tcell(types.TextPlain("panel | multi")),
                    ]
                ),
                types.PageTableRow([_tcell(types.TextPlain("other subscriptions"), colspan=2)]),
            ],
            bordered=True,
        )
    ],
    photos=[],
    documents=[],
    rtl=False,
)


class FakeTelethon:
    """Enough of a client for the fetch: it records what was asked for."""

    def __init__(self, rich=None, error=None):
        self.requests = []
        self.rich = rich
        self.error = error

    async def get_input_entity(self, chat_id):
        return f"peer:{chat_id}"

    async def __call__(self, request):
        self.requests.append(request)
        if self.error:
            raise self.error
        # A stand-in rather than a real `Message`: the tool reads exactly one field
        # off it, and constructing the full type here would pin this test to a
        # constructor signature it does not care about.
        message = SimpleNamespace(id=getattr(request, "id", 0), rich_message=self.rich)
        return SimpleNamespace(messages=[message], chats=[], users=[])


@pytest.fixture
def telethon_wire(monkeypatch):
    def _wire(rich=None, error=None):
        client = FakeTelethon(rich, error)
        monkeypatch.setattr(rm, "get_client", lambda account=None: client)

        async def _connected(cl):
            return None

        monkeypatch.setattr(rm, "ensure_connected", _connected)
        return client

    return _wire


def _results(raw):
    return json.loads(raw)["results"]


# --------------------------------------------------------------------------
# The identifier boundary
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_the_message_id_is_passed_through_untouched(telethon_wire):
    """The previous backend numbered messages `server_id << 20`, so the id had to be
    shifted on the way in and back on the way out. MTProto uses the id the caller
    already has - the one in a t.me link - and shifting it now would ask for a message
    roughly a million times younger, which exists and is somebody else's."""
    client = telethon_wire(RICH_TABLE)

    await rm.read_rich_message(chat_id=-1002032650056, message_id=4614680, account="acct")

    (asked,) = client.requests
    assert asked.id == 4614680


@pytest.mark.asyncio
async def test_the_chat_is_resolved_through_the_ordinary_client(telethon_wire):
    """`me`, `@name` and a saved alias all reach this tool, and all three died on the
    previous backend's `int()`. Resolving through the client every neighbouring tool
    uses is what keeps this one addressable the same way."""
    client = telethon_wire(RICH_TABLE)

    await rm.read_rich_message(chat_id="me", message_id=4614680, account="acct")

    assert client.requests[0].peer == "peer:me"


# --------------------------------------------------------------------------
# Rendering
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_every_cell_of_the_table_survives(telethon_wire):
    telethon_wire(RICH_TABLE)

    results = _results(
        await rm.read_rich_message(chat_id=-1002032650056, message_id=4614680, account="acct")
    )

    (block,) = results["blocks"]
    assert block["type"] == "pageBlockTable"
    assert block["row_count"] == 3
    assert block["column_count"] == 2
    flat = " ".join(cell["text"] for row in block["rows"] for cell in row)
    for expected in ("Chatgpt plus", "v2ray residential", "Gemini pro", "other subscriptions"):
        assert expected in flat, f"{expected!r} was lost"


@pytest.mark.asyncio
async def test_a_merged_cell_is_reported_rather_than_faked(telethon_wire):
    """Markdown cannot express a colspan. Duplicating or dropping the cell to
    make the grid rectangular would misreport what the table actually says, so
    the span is carried in the structured rows instead."""
    telethon_wire(RICH_TABLE)

    results = _results(
        await rm.read_rich_message(chat_id=-1002032650056, message_id=4614680, account="acct")
    )

    merged = results["blocks"][0]["rows"][2][0]
    assert merged["colspan"] == 2
    assert merged["text"] == "other subscriptions"


@pytest.mark.asyncio
async def test_nested_formatting_is_flattened_not_dropped(telethon_wire):
    """A cell is a TREE: bold wrapping plain, beside more plain. A flattener that
    only handled the outer node would return an empty cell and nothing would say
    text had been lost."""
    telethon_wire(RICH_TABLE)

    results = _results(
        await rm.read_rich_message(chat_id=-1002032650056, message_id=4614680, account="acct")
    )

    first = results["blocks"][0]["rows"][0][0]["text"]
    assert "Chatgpt plus" in first
    assert "personal email" in first, "the sibling text beside the bold run was dropped"


@pytest.mark.asyncio
async def test_a_captions_link_keeps_its_destination(telethon_wire):
    """ "ShaparakVPN | Services" without its URL is the half that does not
    matter."""
    telethon_wire(RICH_TABLE)

    results = _results(
        await rm.read_rich_message(chat_id=-1002032650056, message_id=4614680, account="acct")
    )

    assert "https://t.me/shaparakvpn" in results["blocks"][0]["caption"]


@pytest.mark.asyncio
async def test_the_markdown_view_is_a_usable_table(telethon_wire):
    telethon_wire(RICH_TABLE)

    results = _results(
        await rm.read_rich_message(chat_id=-1002032650056, message_id=4614680, account="acct")
    )

    markdown = results["blocks"][0]["markdown"]
    lines = markdown.splitlines()
    assert lines[1].startswith("|"), "no separator row, so it is not a table"
    assert set(lines[1].replace("|", "").replace(" ", "")) == {"-"}
    # A pipe inside a cell would end the column early and shift every value
    # after it into the wrong header.
    assert "\\|" in markdown, "a pipe inside a cell was not escaped"


# --------------------------------------------------------------------------
# Refusing
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_an_ordinary_message_is_sent_back_to_inspect_message(telethon_wire):
    """This tool exists for one content type. Answering with an empty block list
    for anything else would read as "the message is empty", which is the very
    confusion it was built to end.

    A message that is not rich carries no rich body, so the fetch comes back with
    nothing to render - and the answer says which of the two it was rather than an
    empty block list that looks like a rich message with no content."""
    telethon_wire(None)

    results = _results(await rm.read_rich_message(chat_id=1, message_id=2, account="acct"))

    assert results["content_type"] is None
    assert "inspect_message" in results["note"]
    assert "inspect_message" in results["note"]
    assert "blocks" not in results


@pytest.mark.asyncio
async def test_telegrams_refusal_is_shown_not_filed_under_a_code(telethon_wire):
    telethon_wire(error=errors.RPCError(request=None, message="MESSAGE_ID_INVALID", code=400))

    answer = await rm.read_rich_message(chat_id=1, message_id=2, account="acct")

    assert "MESSAGE_ID_INVALID" in answer
