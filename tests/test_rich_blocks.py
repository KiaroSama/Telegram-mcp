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


# --- blocks whose content is neither text nor a table -----------------------


def test_a_formula_block_keeps_its_expression():
    """`PageBlockMath` carries a bare source string. Falling through to the generic
    text branch found no `text` and reported an empty block."""
    record = rich_blocks.render_block(types.PageBlockMath("E = mc^2"))

    assert record["type"] == "pageBlockMathematicalExpression"
    assert record["expression"] == "E = mc^2"


def test_a_map_block_reports_where_it_points():
    """A map is the one block whose content is neither text nor a file, so reporting
    only its type read as an empty block."""
    block = types.PageBlockMap(
        geo=types.GeoPoint(51.4, 35.7, 0),
        zoom=14,
        w=600,
        h=400,
        caption=types.PageCaption(types.TextPlain("the office"), types.TextEmpty()),
    )

    record = rich_blocks.render_block(block)

    assert record["type"] == "pageBlockMap"
    assert record["location"] == {"latitude": 35.7, "longitude": 51.4}
    assert (record["zoom"], record["width"], record["height"]) == (14, 600, 400)
    assert record["caption"] == "the office"


def test_a_photo_block_names_the_file_it_points_at():
    """The block does not CARRY the picture - it names one travelling beside it on
    the message, and `download_rich_media` resolves the two."""
    block = types.PageBlockPhoto(
        photo_id=77, caption=types.PageCaption(types.TextPlain("a pic"), types.TextEmpty())
    )

    record = rich_blocks.render_block(block)

    assert record["type"] == "pageBlockPhoto"
    assert record["media"] == {"kind": "photo", "file_id": "77"}
    assert record["caption"] == "a pic"


def test_a_track_reports_what_it_is_rather_than_an_empty_record():
    record = rich_blocks.render_block(
        types.PageBlockAudio(
            audio_id=9, caption=types.PageCaption(types.TextEmpty(), types.TextEmpty())
        )
    )

    assert record["media"] == {"kind": "audio", "file_id": "9"}


def test_a_gallery_reports_the_pictures_inside_it():
    block = types.PageBlockCollage(
        items=[
            types.PageBlockPhoto(
                photo_id=1, caption=types.PageCaption(types.TextEmpty(), types.TextEmpty())
            )
        ],
        caption=types.PageCaption(types.TextPlain("two shots"), types.TextEmpty()),
    )

    record = rich_blocks.render_block(block)

    assert record["type"] == "pageBlockCollage"
    assert record["items"][0]["media"]["file_id"] == "1"
    assert record["caption"] == "two shots"


def test_a_slideshow_is_read_the_same_way_as_a_collage():
    block = types.PageBlockSlideshow(
        items=[
            types.PageBlockPhoto(
                photo_id=2, caption=types.PageCaption(types.TextEmpty(), types.TextEmpty())
            )
        ],
        caption=types.PageCaption(types.TextEmpty(), types.TextEmpty()),
    )

    assert rich_blocks.render_block(block)["items"][0]["media"]["file_id"] == "2"


# --- text the renderer must not mangle --------------------------------------


def test_a_button_keeps_its_label_and_target():
    """A button is not text with a link: the label hangs off the button and the
    destination off its type, so nothing sits under `text` in the usual shape and the
    generic fallback read a whole button back as an empty paragraph."""
    node = types.TextButton(types.TextPlain("Open"), types.InlineButtonTypeUrl("https://x.test"))

    assert rich_blocks.flatten(node) == "[Open]<tg-button url=https://x.test>"


def test_a_button_without_a_url_is_named_by_its_type():
    node = types.TextButton(types.TextPlain("Pay"), types.InlineButtonTypeBuy())

    assert "[Pay]<tg-button url=InlineButtonTypeBuy>" == rich_blocks.flatten(node)


def test_flattening_never_raises_on_a_shape_it_has_not_met():
    """An unhandled node must contribute nothing, not explode: a whole message would
    be lost to one block type Telegram added since."""

    class Unheard:
        pass

    assert rich_blocks.flatten(Unheard()) == ""


def test_a_cell_keeps_the_joiners_that_are_part_of_its_words_and_emoji():
    """The generic sanitiser strips every Cf character, which silently rewrites the
    message: Persian spells a word with ZWNJ, and a multi-part emoji is one emoji only
    because a ZWJ holds it together."""
    body = "صرافی‌های \U0001f636‍\U0001f32b️"
    block = _table([_cell(body)])

    assert rich_blocks.render_block(block)["rows"][0][0]["text"] == body


def test_a_long_cell_is_not_cut_to_a_display_name_length():
    """Body text is not a display name. The display-name sanitiser caps at 256 and
    flattens newlines, so a long table cell came back truncated."""
    body = "x" * 400
    block = _table([_cell(body)])

    cell = rich_blocks.render_block(block)["rows"][0][0]

    assert cell["text"] == body and "truncated" not in cell["text"]
