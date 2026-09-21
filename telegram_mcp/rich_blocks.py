"""Rich-message page blocks, rendered from what MTProto returns.

A rich message keeps its body outside the message: the message itself parses cleanly
and reports nothing, and the content comes back from a separate fetch by chat and
message id. This module turns what that fetch returns into the records
`read_rich_message` has always published.

**Why it is not in `tools/rich_messages.py`**: that file stood at 589 lines, and the
project closes a file to new code at about 700. The tool stays there; the rendering
lives here.

**The shape is not a new design.** Every record below reproduces what the previous
backend's renderer produced, field for field, because the whole point of the migration
is that a caller cannot tell it happened. Where the two protocols describe the same
thing differently the difference is absorbed here and nowhere else - alignment is the
clearest case: one names a type (`pageBlockHorizontalAlignmentCenter`) and the other
sets a flag (`align_center=True`), and both mean "centre".
"""

from typing import Any

from telethon.tl import types

from telegram_mcp.text_fidelity import display_text

__all__ = ["render_blocks", "flatten"]

#: What each block is CALLED in the answer this tool has always published.
#:
#: The two protocols spell the same block differently - `PageBlockTable` here,
#: `pageBlockTable` there - and `type` is a published field, so a caller that
#: switches on it would break on a rename it never asked for. The published
#: spelling wins; anything unmapped falls back to its own class name so a block
#: Telegram adds later is reported rather than hidden.
_BLOCK_NAMES = {
    "PageBlockTable": "pageBlockTable",
    "PageBlockParagraph": "pageBlockParagraph",
    "PageBlockBlockquote": "pageBlockBlockQuote",
    "PageBlockPullquote": "pageBlockPullQuote",
    "PageBlockList": "pageBlockList",
    "PageBlockOrderedList": "pageBlockList",
    "PageBlockDetails": "pageBlockDetails",
    "PageBlockCollage": "pageBlockCollage",
    "PageBlockSlideshow": "pageBlockSlideshow",
    "PageBlockDivider": "pageBlockDivider",
    "PageBlockHeader": "pageBlockHeader",
    "PageBlockSubheader": "pageBlockSubheader",
    "PageBlockTitle": "pageBlockTitle",
    "PageBlockSubtitle": "pageBlockSubtitle",
    "PageBlockPreformatted": "pageBlockPreformatted",
    "PageBlockFooter": "pageBlockFooter",
    "PageBlockAuthorDate": "pageBlockAuthorDate",
    "PageBlockAnchor": "pageBlockAnchor",
    "PageBlockKicker": "pageBlockKicker",
    "PageBlockPhoto": "pageBlockPhoto",
    "PageBlockVideo": "pageBlockVideo",
    "PageBlockAudio": "pageBlockAudio",
    "PageBlockEmbedPost": "pageBlockEmbedPost",
}


def _published_name(block) -> str:
    """The block's name as this tool has always reported it."""
    return _BLOCK_NAMES.get(type(block).__name__, type(block).__name__)


def _paragraph(text: str) -> dict:
    """A paragraph record, which is how the published shape wraps loose text.

    A quote and a list item both come back here as nested paragraphs rather than as
    a bare string, because that is what the answer carried before. MTProto hands the
    text over directly; the nesting is the published surface, not a protocol fact.
    """
    return {"type": "pageBlockParagraph", "text": text}


#: Block classes that carry one file, and the media kind each one means.
_MEDIA_BLOCKS = {
    "PageBlockPhoto": "photo",
    "PageBlockVideo": "video",
    "PageBlockAudio": "audio",
    "PageBlockEmbedPost": "embed_post",
}


#: Formatting, as the published answer has always spelled it.
#:
#: The text of a cell is not the whole of what a cell says: an underlined warning and
#: a plain sentence read back identically if the decoration is dropped. The previous
#: backend re-emitted each one as a marker, so this does too - same markers, same
#: order - because dropping them is a capability loss wearing a translation's clothes.
#: `||x||` is Telegram's own spoiler syntax and round-trips.
_EMPHASIS = {
    types.TextBold: ("**", "**"),
    types.TextItalic: ("*", "*"),
    types.TextStrike: ("~~", "~~"),
    types.TextFixed: ("`", "`"),
    types.TextUnderline: ("__", "__"),
    types.TextMarked: ("==", "=="),
    types.TextSubscript: ("~", "~"),
    types.TextSuperscript: ("^", "^"),
    types.TextSpoiler: ("||", "||"),
}


def flatten(node: Any) -> str:
    """One string from a `RichText` tree, with its formatting kept as markers.

    The tree nests: a bold cell containing a link containing plain text is three
    levels. Recursion is the whole algorithm; the only care needed is that every
    shape is handled, because an unhandled one silently contributes nothing and the
    cell comes out blank with no sign anything was lost.

    **The custom-emoji case is the one that has already cost this project once.** A
    real message carried 23 of them and every one came back missing, which read as
    "rich messages cannot hold premium emoji" when they hold them perfectly well. The
    fallback glyph is the text and the id is what makes it reproducible, so both are
    reported - exactly as before.
    """
    if node is None or isinstance(node, types.TextEmpty):
        return ""
    if isinstance(node, str):
        return node
    if isinstance(node, (list, tuple)):
        return "".join(flatten(item) for item in node)
    if isinstance(node, types.TextPlain):
        return node.text or ""
    if isinstance(node, types.TextConcat):
        return "".join(flatten(child) for child in (node.texts or []))
    if isinstance(node, types.TextUrl):
        # A bare label loses the destination, which for a price list or a
        # "click here" cell is the only part that mattered.
        label = flatten(node.text)
        return f"[{label}]({node.url})" if node.url else label
    for kind, (opener, closer) in _EMPHASIS.items():
        if isinstance(node, kind):
            inner = flatten(node.text)
            return f"{opener}{inner}{closer}" if inner else ""
    if isinstance(node, types.TextCustomEmoji):
        # `alt` is a plain string here and `document_id` is the id - neither is a
        # nested node, so recursing into them silently produced an object repr where
        # the fallback glyph should have been.
        alt = node.alt or ""
        emoji_id = getattr(node, "document_id", None)
        return f"{alt}<tg-emoji id={emoji_id}>" if emoji_id else alt
    if isinstance(node, types.TextMath):
        # Not under `text` like every other node - a bare expression string.
        return f"${node.source or ''}$"
    if isinstance(node, types.TextImage):
        # A document rendered inline - a sticker or an image, not an emoji. There is
        # no text to take, so it is named rather than dropped.
        return "[icon]"
    if isinstance(node, types.TextAnchor):
        return flatten(node.text)
    # Every other wrapper carries its content under `text` too; taking that keeps the
    # words even when this does not know the decoration.
    inner = getattr(node, "text", None)
    return flatten(inner) if inner is not None else ""


def _alignment(cell) -> str:
    """Horizontal alignment, named the way this tool has always named it.

    The two protocols disagree about the DEFAULT, not about the meaning: the previous
    backend spelled out `left` for a cell that chose nothing, while MTProto simply
    sets no flag. Measured against a real table on 2026-09-21, where the old backend
    reported `align: "left"` for every body cell. Returning `None` here would look
    like a faithful translation and would silently change a published field from a
    word to a null, so the default is spelled out here too.
    """
    if getattr(cell, "align_center", False):
        return "center"
    if getattr(cell, "align_right", False):
        return "right"
    return "left"


def _valignment(cell) -> str:
    """Vertical alignment, with the same spelled-out default - measured as `middle`."""
    if getattr(cell, "valign_bottom", False):
        return "bottom"
    if getattr(cell, "valign_top", False):
        return "top"
    return "middle"


def _table_rows(block) -> list:
    """`[[cell, ...], ...]` for one table, with every cell's own properties."""
    rows = []
    for row in block.rows or []:
        rows.append(
            [
                {
                    "text": display_text(flatten(cell.text)),
                    "is_header": bool(getattr(cell, "header", False)),
                    # Absent means one, in both protocols. Reporting the absence as
                    # None would make an ordinary cell look like a merged one.
                    "colspan": getattr(cell, "colspan", None) or 1,
                    "rowspan": getattr(cell, "rowspan", None) or 1,
                    "align": _alignment(cell),
                    "valign": _valignment(cell),
                }
                for cell in (row.cells or [])
            ]
        )
    return rows


def _as_markdown(rows: list) -> str:
    """A Markdown table, so the shape survives into whatever reads this.

    Cells are rendered in the order Telegram stored them. `colspan`/`rowspan` are
    reported per cell in the structured block rather than simulated here: Markdown has
    no way to express them, and a renderer that quietly dropped or duplicated a merged
    cell would misreport the table's actual content.
    """
    if not rows:
        return ""
    width = max(len(row) for row in rows)
    lines = []
    for index, row in enumerate(rows):
        cells = [cell["text"].replace("|", "\\|").replace("\n", " ") for cell in row]
        cells += [""] * (width - len(cells))
        lines.append("| " + " | ".join(cells) + " |")
        # A Markdown table needs its separator after the first row whether or not
        # Telegram marked that row as headers.
        if index == 0:
            lines.append("|" + "|".join([" --- "] * width) + "|")
    return "\n".join(lines)


def _list_items(block) -> list:
    """List entries in the published shape: nested blocks plus the marker shown.

    An item may hold blocks rather than text - a list of paragraphs - and either way
    the answer carries `blocks`, so a caller walks one structure rather than two.
    `label` is the marker a reader sees: the number for an ordered list, a bullet for
    an unordered one, which is what the previous backend reported.
    """
    items = []
    for item in block.items or []:
        label = getattr(item, "num", None) or "•"
        text = getattr(item, "text", None)
        if text is not None:
            blocks = [_paragraph(display_text(flatten(text)))]
        else:
            blocks = [render_block(b) for b in (getattr(item, "blocks", None) or [])]
        items.append({"blocks": blocks, "label": str(label)})
    return items


def _caption_text(caption) -> str:
    """A `PageCaption` carries `text` and `credit`; the text is what a reader sees."""
    if caption is None:
        return ""
    return flatten(getattr(caption, "text", caption))


def _media_record(block, kind: str) -> dict:
    """A block that names a file, reported by kind and by what identifies the file."""
    record = {"type": type(block).__name__, "kind": kind}
    caption = _caption_text(getattr(block, "caption", None))
    if caption:
        record["caption"] = display_text(caption)
    for attribute in ("photo_id", "video_id", "audio_id", "webpage_id"):
        value = getattr(block, attribute, None)
        if value:
            record["file_id"] = str(value)
            break
    return record


def render_block(block) -> dict:
    """One page block as a structured record plus a rendered view."""
    name = type(block).__name__
    record: dict = {"type": _published_name(block)}

    if isinstance(block, types.PageBlockTable):
        rows = _table_rows(block)
        record["rows"] = rows
        record["row_count"] = len(rows)
        record["column_count"] = max((len(row) for row in rows), default=0)
        record["markdown"] = _as_markdown(rows)
        caption = flatten(getattr(block, "title", None))
        if caption:
            record["caption"] = display_text(caption)
        for flag, reported in (
            ("bordered", "is_bordered"),
            ("striped", "is_striped"),
            ("compact", "is_compact"),
        ):
            if getattr(block, flag, False):
                record[reported] = True
        return record

    if isinstance(block, (types.PageBlockBlockquote, types.PageBlockPullquote)):
        record["blocks"] = [_paragraph(display_text(flatten(block.text)))]
        caption = flatten(getattr(block, "caption", None))
        if caption:
            record["caption"] = display_text(caption)
        return record

    if isinstance(block, (types.PageBlockList, types.PageBlockOrderedList)):
        record["items"] = _list_items(block)
        record["item_count"] = len(record["items"])
        return record

    if isinstance(block, types.PageBlockDetails):
        record["title"] = display_text(flatten(getattr(block, "title", None)))
        record["blocks"] = [render_block(b) for b in (block.blocks or [])]
        record["open"] = bool(getattr(block, "open", False))
        return record

    if isinstance(block, (types.PageBlockCollage, types.PageBlockSlideshow)):
        record["items"] = [render_block(b) for b in (block.items or [])]
        caption = _caption_text(getattr(block, "caption", None))
        if caption:
            record["caption"] = display_text(caption)
        return record

    if isinstance(block, types.PageBlockDivider):
        return record

    if name in _MEDIA_BLOCKS:
        return _media_record(block, _MEDIA_BLOCKS[name])

    # Everything with a `text` - headings, subtitles, paragraphs, preformatted,
    # authors, footers. Reported by its own type name so a reader can tell a heading
    # from a paragraph without this module having to enumerate every one.
    for attribute in ("text", "title", "subtitle", "author", "caption"):
        value = getattr(block, attribute, None)
        if value is not None:
            rendered = display_text(flatten(value))
            if rendered:
                record["text"] = rendered
                break
    return record


def render_blocks(rich) -> dict:
    """A whole `RichMessage` as the record `read_rich_message` publishes."""
    blocks = [render_block(block) for block in (rich.blocks or [])]
    return {
        "blocks": blocks,
        "block_count": len(blocks),
        "rtl": bool(getattr(rich, "rtl", False)),
        "partial": bool(getattr(rich, "part", False)),
    }
