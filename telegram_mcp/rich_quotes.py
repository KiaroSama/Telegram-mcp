"""Article quote text, source coordinates and exact formatted fragment selection."""

from copy import copy
from typing import Optional

from telethon import types
from telegram_mcp.rich_blocks import flatten
from telegram_mcp.text_fidelity import display_text, fidelity_text, _normalize_breaks


class QuoteError(ValueError):
    """A selection that cannot be sent faithfully."""


def plain(node):
    if node is None or isinstance(node, types.TextEmpty):
        return ""
    if isinstance(node, types.TextPlain):
        return node.text or ""
    if isinstance(node, types.TextConcat):
        return "".join(plain(child) for child in node.texts or [])
    if isinstance(node, types.TextCustomEmoji):
        return node.alt or ""
    if isinstance(node, types.TextMath):
        return node.source or ""
    if isinstance(node, types.TextImage):
        return "￼"
    child = getattr(node, "text", None)
    if child is not None and not isinstance(child, str):
        return plain(child)
    raise QuoteError(f"Unsupported rich text: {type(node).__name__}.")


def _safe_tree(value):
    if isinstance(value, str):
        return display_text(value)
    if isinstance(value, dict):
        return {key: _safe_tree(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_safe_tree(item) for item in value]
    return value


def describe_rich_quote(msg) -> Optional[dict]:
    reply = getattr(msg, "reply_to", None)
    fragments = getattr(reply, "quote_rich", None)
    if not fragments:
        return None
    try:
        text = "\n".join(plain(fragment) for fragment in fragments)
    except QuoteError:
        text = "\n".join(flatten(fragment) for fragment in fragments)
    result = {
        "rich": True,
        "text": display_text(text),
        "markdown": display_text("\n".join(flatten(fragment) for fragment in fragments)),
        "fragments": [_safe_tree(fragment.to_dict()) for fragment in fragments],
    }
    filtered, _ = fidelity_text(_normalize_breaks(text))
    if filtered != text or result["text"] != filtered:
        result["modified"] = True
        result["note"] = (
            "Quote display was filtered or bounded; offsets refer to the original source."
        )
    offset = getattr(reply, "quote_offset", None)
    if offset is not None:
        result["offset"] = offset
    return result


def sources(rich):
    """Text-bearing locations, with table cells distinct (indexes are zero-based)."""
    found = []

    def add(node, location):
        if node is None:
            return
        try:
            text = plain(node)
        except QuoteError:
            return
        if text:
            base = (found[-1][0]["source_offset"] + _length(found[-1][2]) + 1) if found else 0
            found.append(({**location, "source_offset": base}, node, text))

    def blocks(items, path=()):
        for index, block in enumerate(items or []):
            location = {"block_path": list(path + (index,)), "block_index": (path or (index,))[0]}
            if isinstance(block, types.PageBlockTable):
                add(block.title, {**location, "title": True})
                for row_index, row in enumerate(block.rows or []):
                    for column, cell in enumerate(row.cells or []):
                        add(cell.text, {**location, "row": row_index, "column": column})
            else:
                node = getattr(block, "text", None)
                if node is not None and not isinstance(node, str):
                    add(node, location)
                for attribute in ("blocks", "items"):
                    children = getattr(block, attribute, None)
                    if children:
                        blocks(children, path + (index,))
                caption = getattr(block, "caption", None)
                if caption is not None:
                    add(getattr(caption, "text", caption), {**location, "caption": True})

    blocks(getattr(rich, "blocks", []))
    return found


def quote_sources(rich):
    return [
        {**location, "text": display_text(text), "markdown": display_text(flatten(node))}
        for location, node, text in sources(rich)
    ]


def _length(text):
    return len(text.encode("utf-16-le")) // 2


def _slice(node, start, end):
    text = plain(node)
    total = _length(text)
    if start == 0 and end == total:
        return node
    if isinstance(node, types.TextPlain):
        try:
            return types.TextPlain(
                text.encode("utf-16-le")[start * 2 : end * 2].decode("utf-16-le")
            )
        except UnicodeDecodeError as error:
            raise QuoteError("Quote offset splits a Unicode character.") from error
    if isinstance(node, types.TextConcat):
        offset, children = 0, []
        for child in node.texts or []:
            length = _length(plain(child))
            left, right = max(start, offset), min(end, offset + length)
            if left < right:
                children.append(_slice(child, left - offset, right - offset))
            offset += length
        return types.TextConcat(children)
    child = getattr(node, "text", None)
    if child is not None and not isinstance(child, str):
        cloned = copy(node)
        cloned.text = _slice(child, start, end)
        return cloned
    raise QuoteError(f"Cannot partially select {type(node).__name__}; select it whole.")


def select(rich, text, offset=None, block_index=None, row=None, column=None):
    if not text:
        raise QuoteError("quote_text must not be empty.")
    if len(text.encode("utf-8")) > 4096:
        raise QuoteError("quote_text is too long (maximum 4096 UTF-8 bytes).")
    if (row is None) != (column is None):
        raise QuoteError("quote_row and quote_column must be supplied together.")
    for value in (offset, block_index, row, column):
        if value is not None and (not isinstance(value, int) or value < 0):
            raise QuoteError("Quote coordinates must be nonnegative integers.")
    matches = []
    for location, node, value in sources(rich):
        if block_index is not None and location["block_index"] != block_index:
            continue
        if row is not None and (location.get("row"), location.get("column")) != (row, column):
            continue
        position = 0
        while True:
            position = value.find(text, position)
            if position < 0:
                break
            units = _length(value[:position])
            source_offset = location["source_offset"] + units
            if offset is None or source_offset == offset:
                matches.append((node, units, {**location, "offset": source_offset}))
            position += 1
    if len(matches) != 1:
        raise QuoteError(
            "Quote selection is ambiguous; specify block/row/column/offset."
            if matches
            else "quote_text was not found at the specified rich location."
        )
    node, start, location = matches[0]
    return [_slice(node, start, start + _length(text))], location
