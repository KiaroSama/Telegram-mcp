"""Real rich quote wire and selection, with no account content in fixtures."""

import struct
from types import SimpleNamespace

import pytest
from telethon import types
from telethon.extensions import BinaryReader


def test_rich_reply_header_decodes_official_constructor():
    from telegram_mcp.rich_quotes import describe_rich_quote

    # Desktop v7.3.3 api.tl: ddb5ab75, reply_id bit4, quote bit9, rich bit14.
    fragment = types.TextBold(types.TextPlain("Launch day 🎉"))
    wire = struct.pack(
        "<IIiIi", 0xDDB5AB75, (1 << 4) | (1 << 9) | (1 << 14), 42, 0x1CB5C415, 1
    ) + bytes(fragment)
    with BinaryReader(wire) as reader:
        header = reader.tgread_object()
    result = describe_rich_quote(SimpleNamespace(reply_to=header))
    assert result["text"] == "Launch day 🎉"
    assert result["markdown"] == "**Launch day 🎉**"
    assert result["rich"] is True
    assert header.reply_to_msg_id == 42


@pytest.mark.asyncio
async def test_table_quote_sends_native_fragment_and_keeps_topic(wire_client):
    from telegram_mcp.tools import messages as mod
    from telethon import functions
    from telegram_mcp.rich_quotes import plain

    class Client:
        def __init__(self):
            self.requests = []

        async def get_input_entity(self, entity):
            return types.InputPeerSelf()

        async def __call__(self, request):
            self.requests.append(request)
            if isinstance(request, functions.messages.GetRichMessageRequest):
                table = types.PageBlockTable(
                    title=types.TextEmpty(),
                    rows=[
                        types.PageTableRow(
                            cells=[
                                types.PageTableCell(
                                    text=types.TextBold(types.TextPlain("Launch day 🎉"))
                                )
                            ]
                        )
                    ],
                )
                return SimpleNamespace(
                    messages=[
                        SimpleNamespace(
                            rich_message=types.RichMessage(blocks=[table], photos=[], documents=[])
                        )
                    ]
                )
            return SimpleNamespace(updates=[SimpleNamespace(id=55)])

    client = Client()
    wire_client(mod, client)
    result = await mod.reply_to_message(
        "me",
        42,
        "yes",
        quote_text="day 🎉",
        quote_block_index=0,
        quote_row=0,
        quote_column=0,
        topic_id=9,
        account="default",
    )
    assert '"message_id": 55' in result, result
    request = client.requests[-1]
    assert isinstance(request, functions.messages.SendMessageRequest)
    assert request.reply_to.reply_to_msg_id == 42
    assert request.reply_to.top_msg_id == 9
    assert request.reply_to.quote_text is None
    assert request.reply_to.quote_offset == 7
    fragment = request.reply_to.quote_rich[0]
    assert isinstance(fragment, types.TextBold)
    assert plain(fragment) == "day 🎉"
    assert struct.unpack_from("<I", bytes(request.reply_to))[0] == 0x75C1AD70


@pytest.mark.parametrize(
    "text,offset,block,row,column",
    [
        ("same", None, None, None, None),
        ("absent", None, 0, 0, 0),
        ("same", None, 0, 0, None),
        ("same", -1, 0, 0, 0),
        ("", None, 0, 0, 0),
    ],
)
def test_invalid_or_ambiguous_selection_is_refused(text, offset, block, row, column):
    from telegram_mcp.rich_quotes import select, QuoteError

    rich = types.RichMessage(
        blocks=[
            types.PageBlockTable(
                title=types.TextEmpty(),
                rows=[
                    types.PageTableRow(
                        cells=[
                            types.PageTableCell(text=types.TextPlain("same")),
                            types.PageTableCell(text=types.TextPlain("same")),
                        ]
                    )
                ],
            )
        ],
        photos=[],
        documents=[],
    )
    with pytest.raises(QuoteError):
        select(rich, text, offset, block, row, column)


def test_selection_retains_custom_emoji_and_utf16_span():
    from telegram_mcp.rich_quotes import select, plain

    node = types.TextConcat(
        [
            types.TextPlain("🚀 start "),
            types.TextCustomEmoji(document_id=123, alt="🎉"),
            types.TextPlain(" finish"),
        ]
    )
    rich = types.RichMessage(blocks=[types.PageBlockParagraph(text=node)], photos=[], documents=[])
    fragments, location = select(rich, "🎉", offset=9, block_index=0)
    assert plain(fragments[0]) == "🎉"
    assert fragments[0].texts[0].document_id == 123
    assert location["block_index"] == 0


def test_rich_quote_filters_hidden_controls_in_fragments():
    from telegram_mcp.rich_quotes import describe_rich_quote

    header = types.MessageReplyHeader(quote_rich=[types.TextPlain("safe‮evil")])
    result = describe_rich_quote(SimpleNamespace(reply_to=header))
    assert result["modified"] is True
    assert result["text"] == "safeevil"
    assert result["fragments"][0]["text"] == "safeevil"


def test_repeated_cell_selection_keeps_global_projection_offset():
    from telegram_mcp.rich_quotes import select

    rich = types.RichMessage(
        blocks=[
            types.PageBlockTable(
                title=types.TextPlain("Title"),
                rows=[
                    types.PageTableRow(
                        cells=[
                            types.PageTableCell(text=types.TextPlain("same")),
                            types.PageTableCell(text=types.TextPlain("same")),
                        ]
                    )
                ],
            )
        ],
        photos=[],
        documents=[],
    )
    _, location = select(rich, "same", offset=11, block_index=0, row=0, column=1)
    assert location["offset"] == 11


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "selection",
    [
        {"quote_block_index": 0},
        {"quote_text": "same", "quote_block_index": 0},
        {"quote_text": "same", "quote_row": 0},
        {"quote_text": "absent", "quote_block_index": 0},
    ],
)
async def test_invalid_public_quote_selection_sends_nothing(wire_client, selection):
    from telegram_mcp.tools import messages as mod
    from telethon import functions

    class Client:
        async def get_input_entity(self, entity):
            return types.InputPeerSelf()

        async def __call__(self, request):
            assert isinstance(request, functions.messages.GetRichMessageRequest)
            node = types.TextPlain("same same")
            return SimpleNamespace(
                messages=[
                    SimpleNamespace(
                        rich_message=types.RichMessage(
                            blocks=[types.PageBlockParagraph(text=node)], photos=[], documents=[]
                        )
                    )
                ]
            )

        async def send_message(self, *args, **kwargs):
            raise AssertionError("Invalid selection reached the sender")

    wire_client(mod, Client())
    result = await mod.reply_to_message("me", 42, "reply", account="default", **selection)
    assert "Nothing was sent" in result, result
