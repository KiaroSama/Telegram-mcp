"""Rendering a rich message's page blocks from what MTProto returns.

These pin the answer `read_rich_message` has always published, because the backend
under it changed and a caller must not be able to tell. Every expectation here was
taken from a REAL message read over both backends on 2026-09-21 and compared field by
field; `specs/003-tdlib-removal/evidence/rich-proof.md` records that comparison.

The three that matter most are the three that were wrong first, each of which looked
like a faithful translation and was a capability quietly going missing.
"""

import pytest
from telethon.tl import types

from telegram_mcp import rich_blocks


def _cell(text, **kwargs):
    return types.PageTableCell(text=types.TextPlain(text), **kwargs)


def _table(*rows, **kwargs):
    return types.PageBlockTable(
        title=kwargs.pop("title", types.TextEmpty()),
        rows=[types.PageTableRow(list(row)) for row in rows],
        **kwargs,
    )


# --- formatting survives, because the text alone is not what the cell says -------


@pytest.mark.parametrize(
    "node, expected",
    [
        (types.TextBold(types.TextPlain("BTC")), "**BTC**"),
        (types.TextItalic(types.TextPlain("steady")), "*steady*"),
        (types.TextStrike(types.TextPlain("old")), "~~old~~"),
        (types.TextFixed(types.TextPlain("code")), "`code`"),
        (types.TextUnderline(types.TextPlain("warn")), "__warn__"),
        (types.TextMarked(types.TextPlain("hi")), "==hi=="),
        (types.TextSpoiler(types.TextPlain("s")), "||s||"),
    ],
)
def test_emphasis_is_re_emitted_as_the_marker_the_answer_has_always_carried(node, expected):
    """An underlined warning and a plain sentence must not read back identically."""
    assert rich_blocks.flatten(node) == expected


def test_a_premium_emoji_keeps_its_id():
    """The loss this project has already paid for once.

    A real message carried 23 custom emoji and every one came back missing, which read
    as "rich messages cannot hold premium emoji" when they hold them perfectly well.
    The fallback glyph is the text and the id is what makes it reproducible, so the
    answer carries both.
    """
    node = types.TextCustomEmoji(5787207924413637224, "⭕️")

    assert rich_blocks.flatten(node) == "⭕️<tg-emoji id=5787207924413637224>"


def test_a_link_keeps_its_destination():
    """For a price list or a "click here" cell the destination is the only part that
    mattered, and a bare label loses it."""
    node = types.TextUrl(types.TextPlain("go"), "https://example.test", 0)

    assert rich_blocks.flatten(node) == "[go](https://example.test)"


def test_formatting_nests():
    """A bold cell containing a link containing plain text is three levels."""
    node = types.TextBold(types.TextUrl(types.TextPlain("go"), "https://x.test", 0))

    assert rich_blocks.flatten(node) == "**[go](https://x.test)**"


def test_a_maths_expression_is_not_read_as_a_wrapper():
    """`TextMath` carries a bare `source` string, not a nested node. Recursing into
    `text` the way every other node allows returned nothing and the formula vanished."""
    assert rich_blocks.flatten(types.TextMath("a^2 + b^2")) == "$a^2 + b^2$"


def test_an_inline_image_is_named_rather_than_dropped():
    assert rich_blocks.flatten(types.TextImage(1, 10, 10)) == "[icon]"


def test_an_empty_node_contributes_nothing():
    assert rich_blocks.flatten(types.TextEmpty()) == ""
    assert rich_blocks.flatten(None) == ""


# --- the published block shape --------------------------------------------------


def test_a_table_reports_every_cell_property():
    block = _table(
        [_cell("Coin", header=True, align_center=True, valign_middle=True)],
        [_cell("BTC")],
        bordered=True,
    )

    record = rich_blocks.render_block(block)

    assert record["type"] == "pageBlockTable", "the published spelling, not the class name"
    assert record["row_count"] == 2
    assert record["column_count"] == 1
    assert record["is_bordered"] is True
    header, body = record["rows"]
    assert header[0] == {
        "text": "Coin",
        "is_header": True,
        "colspan": 1,
        "rowspan": 1,
        "align": "center",
        "valign": "middle",
    }
    assert body[0]["align"] == "left", "the default is spelled out, as it always was"
    assert body[0]["valign"] == "middle"
    assert body[0]["is_header"] is False


def test_a_merged_cell_reports_its_spans():
    """Markdown cannot express a span, so the structured record is where it lives."""
    block = _table([_cell("wide", colspan=3, rowspan=2)])

    cell = rich_blocks.render_block(block)["rows"][0][0]

    assert (cell["colspan"], cell["rowspan"]) == (3, 2)


def test_markdown_escapes_a_pipe_so_the_table_does_not_break():
    block = _table([_cell("a|b")], [_cell("plain")])

    assert "a\\|b" in rich_blocks.render_block(block)["markdown"]


def test_a_quote_is_published_as_nested_blocks():
    """Not as a bare string: the previous answer nested a paragraph, and `blocks` is
    what a caller walks."""
    block = types.PageBlockBlockquote(text=types.TextPlain("a quote"), caption=types.TextEmpty())

    record = rich_blocks.render_block(block)

    assert record["type"] == "pageBlockBlockQuote"
    assert record["blocks"] == [{"type": "pageBlockParagraph", "text": "a quote"}]


def test_a_list_reports_items_with_their_marker_and_a_count():
    block = types.PageBlockList(
        [
            types.PageListItemText(types.TextPlain("first")),
            types.PageListItemText(types.TextPlain("second")),
        ]
    )

    record = rich_blocks.render_block(block)

    assert record["item_count"] == 2
    assert record["items"][0] == {
        "blocks": [{"type": "pageBlockParagraph", "text": "first"}],
        "label": "•",
    }


def test_an_ordered_list_labels_each_item_with_its_number():
    block = types.PageBlockOrderedList(
        [types.PageListOrderedItemText(text=types.TextPlain("first"), num="1.")]
    )

    assert rich_blocks.render_block(block)["items"][0]["label"] == "1."


def test_an_unknown_block_is_reported_rather_than_hidden():
    """A block Telegram adds later keeps its own name instead of disappearing."""
    record = rich_blocks.render_block(types.PageBlockDivider())

    assert record["type"] == "pageBlockDivider"


def test_the_whole_message_reports_its_direction_and_count():
    rich = types.RichMessage(
        blocks=[types.PageBlockParagraph(types.TextPlain("hi"))],
        photos=[],
        documents=[],
        rtl=True,
    )

    rendered = rich_blocks.render_blocks(rich)

    assert rendered["block_count"] == 1
    assert rendered["rtl"] is True
    assert rendered["blocks"][0] == {"type": "pageBlockParagraph", "text": "hi"}
