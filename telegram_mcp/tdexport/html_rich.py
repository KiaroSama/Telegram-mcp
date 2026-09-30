"""Rich-message rendering, part three: structural blocks and the message wrapper.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
RichHtmlRenderer::renderMessage/renderTextBlock/renderAuthorDate/renderCode/renderList/
renderListItem/renderQuote/renderDetails/renderTable/renderTableRow/renderTableCell/
renderButtonRow/renderFallback/renderBlock, RenderRichMessage), GPL-3.0.
"""

from __future__ import annotations

from typing import Callable

from .html_rich_media import RichMediaCallbacks, RichMediaRenderer
from .html_rich_support import (
    RICH_TABLE_SPAN_MAX,
    rich_block_attributes,
    rich_bool,
    rich_heading_tag,
)
from .html_rich_text import K
from .html_text import HtmlContext, iso_date_time, serialize_string
from .model_format import format_date_time, number_to_string
from .model_rich import (
    RichBlock,
    RichListItem,
    RichListItemContent,
    RichListKind,
    RichMessage,
    RichQuoteContent,
    RichTableAlignment,
    RichTableCell,
    RichTableVerticalAlignment,
    RichTaskState,
    rich_button_alignment_to_string,
)

__all__ = ["RichHtmlRenderer", "RichMediaCallbacks", "render_rich_message"]


class RichHtmlRenderer(RichMediaRenderer):
    def render_message(self) -> str:
        self.collect_anchors()
        result = self._context.push_tag(
            "div",
            {
                "class": "text rich_message",
                "data-rich-message": number_to_string(self._message_id),
                "data-rich-part": rich_bool(self._message.part),
                "dir": "rtl" if self._message.rtl else "auto",
                "inline": "",
            },
        )
        result += self._render_blocks(self._message.blocks)
        return result + self._context.pop_tag()

    def _render_text_block(self, tag: str, attributes: dict[str, str], block: RichBlock) -> str:
        result = self._context.push_tag(tag, attributes)
        return result + self.render_text(block.text) + self._context.pop_tag()

    def _render_author_date(self, block: RichBlock) -> str:
        attributes = rich_block_attributes("rich_author_date", "author-date")
        attributes.setdefault("dir", "auto")
        result = self._context.push_tag("address", attributes)
        result += self.render_text(block.text)
        if block.date:
            result += " \u2022 "
            result += self._context.push_tag(
                "time",
                {
                    "data-date": number_to_string(block.date),
                    "datetime": iso_date_time(block.date),
                    "dir": "ltr",
                    "inline": "",
                },
            )
            result += serialize_string(format_date_time(block.date)) + self._context.pop_tag()
        return result + self._context.pop_tag()

    def _render_code(self, block: RichBlock) -> str:
        attributes = rich_block_attributes("rich_code", "code")
        attributes.setdefault("dir", "auto")
        attributes.setdefault("inline", "")
        if block.language:
            attributes.setdefault("data-language", block.language)
        result = self._context.push_tag("pre", attributes)
        result += self._context.push_tag("code", {"class": "rich_code_content", "inline": ""})
        result += self.render_text(block.text)
        return result + self._context.pop_tag() + self._context.pop_tag()

    def _render_list(self, block: RichBlock) -> str:
        ordered = block.list_kind == RichListKind.Ordered
        attributes = rich_block_attributes("rich_list", "ordered-list" if ordered else "list")
        if ordered:
            if block.ordered_list.reversed:
                attributes.setdefault("reversed", "")
            if block.ordered_list.start is not None:
                attributes.setdefault("start", number_to_string(block.ordered_list.start))
            list_type = block.ordered_list.type
            if list_type is not None:
                native = list_type in ("1", "a", "A", "i", "I")
                attributes.setdefault("type" if native else "data-list-type", list_type)
        result = self._context.push_tag("ol" if ordered else "ul", attributes)
        for item in block.list_items:
            result += self._render_list_item(item, ordered)
        return result + self._context.pop_tag()

    def _render_list_item(self, item: RichListItem, ordered: bool) -> str:
        task = item.task_state != RichTaskState.None_
        checked = item.task_state == RichTaskState.Checked
        custom_marker = ordered and not task and bool(item.num)
        classes = "rich_list_item"
        if task:
            classes += " rich_task_item"
        elif custom_marker:
            classes += " rich_order_item"
        blocks = item.content == RichListItemContent.Blocks
        attributes = {"class": classes, "data-rich-content": "blocks" if blocks else "text"}
        if item.num is not None:
            attributes.setdefault("data-num", item.num)
        if item.type is not None:
            attributes.setdefault("data-item-type", item.type)
        if item.value is not None:
            attributes.setdefault("value", number_to_string(item.value))
        result = self._context.push_tag("li", attributes)
        if task:
            result += self._context.push_tag(
                "span",
                {
                    "aria-checked": rich_bool(checked),
                    "class": "rich_task_marker",
                    "inline": "",
                    "role": "checkbox",
                },
            )
            result += ("\u2611" if checked else "\u2610") + self._context.pop_tag()
        elif custom_marker:
            result += self._context.push_tag("span", {"class": "rich_order_marker", "inline": ""})
            result += serialize_string(item.num or "") + self._context.pop_tag()
        result += self._context.push_tag(
            "div" if blocks else "span", {"class": "rich_list_content", "inline": ""}
        )
        if blocks:
            result += self._render_blocks(item.blocks)
        elif item.text is not None:
            result += self.render_text(item.text)
        return result + self._context.pop_tag() + self._context.pop_tag()

    def _render_quote(self, block: RichBlock) -> str:
        blocks = block.quote_content == RichQuoteContent.Blocks
        kind = "pullquote" if block.pullquote else "quote-blocks" if blocks else "quote"
        classes = "rich_quote rich_pullquote" if block.pullquote else "rich_quote"
        result = self._context.push_tag(
            "aside" if block.pullquote else "blockquote", rich_block_attributes(classes, kind)
        )
        result += self._context.push_tag("div", {"class": "rich_quote_body"})
        result += self._render_blocks(block.blocks) if blocks else self.render_text(block.text)
        result += self._context.pop_tag()
        caption = self.render_text(block.quote_caption)
        if caption:
            result += self._context.push_tag("cite", {"class": "rich_quote_caption"})
            result += caption + self._context.pop_tag()
        return result + self._context.pop_tag()

    def _render_details(self, block: RichBlock) -> str:
        attributes = rich_block_attributes("rich_details", "details")
        if block.open:
            attributes.setdefault("open", "")
        result = self._context.push_tag("details", attributes)
        result += self._context.push_tag("summary", {})
        result += self.render_text(block.text) + self._context.pop_tag()
        result += self._context.push_tag("div", {"class": "rich_details_body"})
        result += self._render_blocks(block.blocks) + self._context.pop_tag()
        return result + self._context.pop_tag()

    def _render_table(self, block: RichBlock) -> str:
        wrapper = rich_block_attributes("rich_table_wrap", "table")
        wrapper.setdefault("tabindex", "0")
        result = self._context.push_tag("div", wrapper)
        classes = "rich_table"
        if block.bordered:
            classes += " bordered"
        if block.striped:
            classes += " striped"
        if block.compact:
            classes += " compact"
        result += self._context.push_tag("table", {"class": classes})
        caption = self.render_text(block.text)
        if caption:
            result += self._context.push_tag("caption", {}) + caption + self._context.pop_tag()
        result += self._context.push_tag("tbody", {})
        for row in block.table_rows:
            result += self._context.push_tag("tr", {})
            result += "".join(self._render_table_cell(cell) for cell in row.cells)
            result += self._context.pop_tag()
        return result + self._context.pop_tag() + self._context.pop_tag() + self._context.pop_tag()

    def _render_table_cell(self, cell: RichTableCell) -> str:
        horizontal = {
            RichTableAlignment.Left: "rich_align_left",
            RichTableAlignment.Center: "rich_align_center",
            RichTableAlignment.Right: "rich_align_right",
        }[cell.alignment]
        vertical = {
            RichTableVerticalAlignment.Top: "rich_valign_top",
            RichTableVerticalAlignment.Middle: "rich_valign_middle",
            RichTableVerticalAlignment.Bottom: "rich_valign_bottom",
        }[cell.vertical_alignment]
        attributes = {"class": horizontal + " " + vertical}
        for name, source in (("colspan", cell.colspan), ("rowspan", cell.rowspan)):
            if source is not None and source > 0:
                span = min(source, RICH_TABLE_SPAN_MAX)
                attributes.setdefault(name, number_to_string(span))
                if span != source:
                    attributes.setdefault("data-source-" + name, number_to_string(source))
        result = self._context.push_tag("th" if cell.header else "td", attributes)
        if cell.text is not None:
            result += self.render_text(cell.text)
        return result + self._context.pop_tag()

    def _render_button_row(self, block: RichBlock) -> str:
        alignment = rich_button_alignment_to_string(block.button_alignment)
        attributes = rich_block_attributes(
            "rich_button_row rich_button_align_" + alignment, "button-row"
        )
        attributes.setdefault("data-button-alignment", alignment)
        result = self._context.push_tag("div", attributes)
        result += self._context.push_tag("div", {"class": "rich_button_group"})
        for button in block.buttons:
            result += self.render_button(button, False)
        return result + self._context.pop_tag() + self._context.pop_tag()

    def _render_fallback(self, kind: str, label: str) -> str:
        result = self._context.push_tag("div", rich_block_attributes("rich_fallback", kind))
        return result + serialize_string(label) + self._context.pop_tag()

    def _render_block(self, block: RichBlock) -> str:
        kind = block.kind
        if kind == K.Unsupported:
            return self._render_fallback("unsupported", "Unsupported rich content")
        if kind == K.Heading:
            attributes = rich_block_attributes("rich_heading", "heading")
            attributes.setdefault("data-level", number_to_string(block.heading_level))
            return self._render_text_block(
                rich_heading_tag(block.heading_level), attributes, block
            )
        if kind in (K.Paragraph, K.Footer, K.Thinking):
            name = {K.Paragraph: "paragraph", K.Footer: "footer", K.Thinking: "thinking"}[kind]
            attributes = rich_block_attributes("rich_" + name, name)
            attributes.setdefault("dir", "auto")
            return self._render_text_block(
                "footer" if kind == K.Footer else "p", attributes, block
            )
        if kind == K.AuthorDate:
            return self._render_author_date(block)
        if kind == K.Code:
            return self._render_code(block)
        if kind == K.Divider:
            attributes = rich_block_attributes("rich_divider", "divider")
            attributes.setdefault("empty", "")
            return self._context.push_tag("hr", attributes)
        if kind == K.Anchor:
            anchor = self.block_anchor_id(block)
            if anchor is None:
                return self._render_fallback("malformed", "Malformed rich content")
            attributes = rich_block_attributes("rich_anchor", "anchor")
            attributes.setdefault("id", anchor)
            attributes.setdefault("inline", "")
            return self._context.push_tag("span", attributes) + self._context.pop_tag()
        if kind == K.Math:
            attributes = rich_block_attributes("rich_math_display", "math")
            attributes.setdefault("dir", "ltr")
            attributes.setdefault("inline", "")
            result = self._context.push_tag("div", attributes)
            return result + serialize_string(block.formula) + self._context.pop_tag()
        simple: dict[RichBlock.Kind, Callable[[RichBlock], str]] = {
            K.List: self._render_list,
            K.Quote: self._render_quote,
            K.Photo: self._render_photo,
            K.Video: lambda b: self._render_document(b, "video"),
            K.Audio: lambda b: self._render_document(b, "audio"),
            K.File: lambda b: self._render_document(b, "file"),
            K.Cover: lambda b: self._render_media_group(b, "cover", False),
            K.Embed: self._render_embed,
            K.EmbedPost: self._render_embed_post,
            K.Collage: self._render_collage,
            K.Slideshow: self._render_slideshow,
            K.Channel: self._render_channel,
            K.Table: self._render_table,
            K.Details: self._render_details,
            K.RelatedArticles: self._render_related_articles,
            K.Map: lambda b: self._render_map(b, "map"),
            K.InputMap: lambda b: self._render_map(b, "input-map"),
            K.ButtonRow: self._render_button_row,
            K.Unknown: lambda b: self._render_fallback("unknown", "Unknown rich content"),
        }
        if kind not in simple:
            raise ValueError("Kind in RichHtmlRenderer::renderBlock.")
        return simple[kind](block)


def render_rich_message(
    context: HtmlContext,
    message: RichMessage,
    message_id: int,
    internal_links_domain: str,
    relative_base: str,
    media: RichMediaCallbacks,
) -> str:
    return RichHtmlRenderer(
        context, message, message_id, internal_links_domain, relative_base, media
    ).render_message()
