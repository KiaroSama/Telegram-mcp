"""Rich-message rendering, part one: anchors, inline rich text, links and buttons.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
RichHtmlRenderer::collect*Anchors, registerAnchor, fragmentHref, textAnchorId, blockAnchorId,
renderTexts, renderTextChildren, renderTextLink, renderButton, renderCustomEmojiLink,
renderText), GPL-3.0. The block half lives in html_rich.py.
"""

from __future__ import annotations

from .html_safe import (
    anchor_token,
    is_ascii_decimal,
    is_strict_target,
    safe_action_data,
    safe_email_href,
    safe_http_href,
    safe_mention_href,
    safe_message_href,
    safe_phone_href,
    safe_relative_emoji_href,
)
from .html_rich_support import plain_target, rich_bool, rich_button_attributes
from .html_text import HtmlContext, iso_date_time, serialize_string
from .model import TextPart
from .model_format import format_date_time, number_to_string
from .model_rich import (
    InlineButtonAction,
    RichBlock,
    RichCaption,
    RichListItemContent,
    RichMessage,
    RichQuoteContent,
    RichText,
)

T = RichText.Type
K = RichBlock.Kind

_WRAPPED = {
    T.Bold: ("strong", {}),
    T.Italic: ("em", {}),
    T.Underline: ("u", {}),
    T.Strike: ("s", {}),
    T.Fixed: ("code", {"class": "rich_inline_code"}),
    T.Subscript: ("sub", {}),
    T.Superscript: ("sup", {}),
    T.Marked: ("mark", {}),
    T.BankCard: ("span", {"class": "rich_bank_card"}),
}
_ANCHOR_CHILDREN = {
    T.Concat,
    T.Bold,
    T.Italic,
    T.Underline,
    T.Strike,
    T.Fixed,
    T.Url,
    T.Email,
    T.Phone,
    T.Subscript,
    T.Superscript,
    T.Marked,
    T.Spoiler,
    T.Mention,
    T.Hashtag,
    T.BotCommand,
    T.Cashtag,
    T.AutoUrl,
    T.AutoEmail,
    T.AutoPhone,
    T.BankCard,
    T.MentionName,
    T.FormattedDate,
    T.Button,
}
_TEXT_ANCHOR_BLOCKS = {K.Heading, K.Paragraph, K.Footer, K.Thinking, K.AuthorDate, K.Code}
_CAPTION_ANCHOR_BLOCKS = {K.Photo, K.Video, K.Audio, K.File, K.Embed, K.Map, K.InputMap}
_UNSUPPORTED_TEXT = {
    "class": "rich_inline_fallback",
    "data-rich-kind": "unsupported-text",
    "inline": "",
}


class RichTextRenderer:
    def __init__(
        self,
        context: HtmlContext,
        message: RichMessage,
        message_id: int,
        internal_links_domain: str,
        relative_base: str,
    ) -> None:
        self._context = context
        self._message = message
        self._message_id = message_id
        self._link_active = False
        self._internal_links_domain = internal_links_domain
        self._relative_base = relative_base
        self._first_anchor_ids: dict[str, str] = {}
        self._anchor_occurrences: dict[str, int] = {}
        self._text_anchor_ids: dict[int, str] = {}
        self._block_anchor_ids: dict[int, str] = {}

    # Anchors.

    def collect_anchors(self) -> None:
        self._first_anchor_ids.clear()
        self._anchor_occurrences.clear()
        self._text_anchor_ids.clear()
        self._block_anchor_ids.clear()
        self._collect_blocks(self._message.blocks)

    def _register_anchor(self, name: str) -> str:
        base = (
            "rich-message-" + number_to_string(self._message_id) + "-anchor-" + anchor_token(name)
        )
        occurrence = self._anchor_occurrences.get(base, 0) + 1
        self._anchor_occurrences[base] = occurrence
        result = base + ("-" + number_to_string(occurrence) if occurrence > 1 else "")
        self._first_anchor_ids.setdefault(name, result)
        return result

    def _collect_text(self, text: RichText) -> None:
        if text.type == T.Anchor:
            self._text_anchor_ids.setdefault(id(text), self._register_anchor(text.data))
            self._collect_texts(text.children)
        elif text.type in _ANCHOR_CHILDREN:
            self._collect_texts(text.children)
        elif text.type == T.Diff:
            self._collect_texts(text.children)
            self._collect_texts(text.old_children)

    def _collect_texts(self, texts: list[RichText]) -> None:
        for text in texts:
            self._collect_text(text)

    def _collect_caption(self, caption: RichCaption) -> None:
        self._collect_text(caption.text)
        self._collect_text(caption.credit)

    def _collect_blocks(self, blocks: list[RichBlock]) -> None:
        for block in blocks:
            self._collect_block(block)

    def _collect_block(self, block: RichBlock) -> None:
        kind = block.kind
        if kind in _TEXT_ANCHOR_BLOCKS or kind == K.RelatedArticles:
            self._collect_text(block.text)
        elif kind == K.Anchor:
            self._block_anchor_ids.setdefault(id(block), self._register_anchor(block.name))
        elif kind == K.List:
            for item in block.list_items:
                if item.content == RichListItemContent.Text:
                    if item.text is not None:
                        self._collect_text(item.text)
                else:
                    self._collect_blocks(item.blocks)
        elif kind == K.Quote:
            if block.quote_content == RichQuoteContent.Text:
                self._collect_text(block.text)
            else:
                self._collect_blocks(block.blocks)
            self._collect_text(block.quote_caption)
        elif kind in _CAPTION_ANCHOR_BLOCKS:
            self._collect_caption(block.caption)
        elif kind in (K.EmbedPost, K.Collage, K.Slideshow):
            self._collect_blocks(block.blocks)
            self._collect_caption(block.caption)
        elif kind == K.Cover:
            self._collect_blocks(block.blocks)
        elif kind == K.Table:
            self._collect_text(block.text)
            for row in block.table_rows:
                for cell in row.cells:
                    if cell.text is not None:
                        self._collect_text(cell.text)
        elif kind == K.Details:
            self._collect_text(block.text)
            self._collect_blocks(block.blocks)
        elif kind == K.ButtonRow:
            for button in block.buttons:
                self._collect_text(button)

    def _fragment_href(self, target: str) -> str | None:
        if not is_strict_target(target) or target[0] != "#":
            return None
        found = self._first_anchor_ids.get(target[1:])
        return "#" + found if found is not None else None

    def block_anchor_id(self, block: RichBlock) -> str | None:
        return self._block_anchor_ids.get(id(block))

    # Inline text.

    def render_texts(self, texts: list[RichText]) -> str:
        return "".join(self.render_text(text) for text in texts)

    def _render_children(self, text: RichText) -> str:
        if text.children:
            return self.render_texts(text.children)
        return (
            self._context.push_tag("span", dict(_UNSUPPORTED_TEXT))
            + "Unsupported rich text"
            + self._context.pop_tag()
        )

    def _render_link(self, text: RichText, href: str | None, attributes: dict[str, str]) -> str:
        attributes.setdefault("inline", "")
        if href is None or self._link_active:
            attributes.pop("href", None)
            attributes.pop("onclick", None)
            attributes.setdefault("class", "rich_inert_link")
            result = self._context.push_tag("span", attributes)
            result += self._render_children(text)
            return result + self._context.pop_tag()
        attributes.setdefault("href", href)
        result = self._context.push_tag("a", attributes)
        previous, self._link_active = self._link_active, True
        try:
            result += self._render_children(text)
        finally:
            self._link_active = previous
        return result + self._context.pop_tag()

    def render_button(self, button: RichText, inline_button: bool) -> str:
        payload = button.button
        if button.type != T.Button or len(button.children) != 1 or payload is None:
            raise ValueError("Malformed rich button in RichHtmlRenderer::renderButton.")
        action = payload.action
        attributes = rich_button_attributes(action, payload.style, inline_button)
        tag = "span"
        nested = self._link_active
        url = safe_message_href(action.url) if action.type == InlineButtonAction.Type.Url else None
        if not nested and url is not None:
            tag = "a"
            attributes.setdefault("href", url)
        elif not nested and action.type == InlineButtonAction.Type.CopyText:
            tag = "button"
            attributes.setdefault("type", "button")
            attributes.setdefault(
                "onclick",
                "return ShowTextCopied(decodeURIComponent(this.dataset.copyText))",
            )
        result = self._context.push_tag(tag, attributes)
        previous, self._link_active = self._link_active, True
        try:
            result += self.render_text(button.children[0])
        finally:
            self._link_active = previous
        return result + self._context.pop_tag()

    def _render_custom_emoji_link(self, alt: str, attributes: dict[str, str]) -> str:
        attributes.setdefault("inline", "")
        if self._link_active:
            attributes.pop("href", None)
            attributes.pop("onclick", None)
            attributes.setdefault("class", "rich_inert_link")
            return self._context.push_tag("span", attributes) + alt + self._context.pop_tag()
        result = self._context.push_tag("a", attributes)
        return result + alt + self._context.pop_tag()

    def _wrap_children(self, tag: str, attributes: dict[str, str], text: RichText) -> str:
        attributes = dict(attributes)
        attributes.setdefault("inline", "")
        result = self._context.push_tag(tag, attributes)
        return result + self._render_children(text) + self._context.pop_tag()

    def render_text(self, text: RichText) -> str:
        kind = text.type
        if kind == T.Empty:
            return ""
        if kind == T.Plain:
            return serialize_string(text.text)
        if kind == T.Concat:
            return self.render_texts(text.children)
        if kind in _WRAPPED:
            tag, attributes = _WRAPPED[kind]
            return self._wrap_children(tag, attributes, text)
        if kind == T.Url:
            href = (
                self._fragment_href(text.data)
                if text.data.startswith("#")
                else safe_http_href(text.data)
            )
            extra = {"data-webpage-id": number_to_string(text.id)} if text.id else {}
            return self._render_link(text, href, extra)
        if kind == T.Email:
            return self._render_link(text, safe_email_href(text.data), {})
        if kind == T.Phone:
            return self._render_link(text, safe_phone_href(text.data), {})
        if kind == T.Anchor:
            attributes = {"class": "rich_reference", "inline": ""}
            anchor = self._text_anchor_ids.get(id(text))
            if anchor is not None:
                attributes.setdefault("id", anchor)
            result = self._context.push_tag("span", attributes)
            return result + self.render_texts(text.children) + self._context.pop_tag()
        if kind == T.Math:
            result = self._context.push_tag(
                "span",
                {
                    "class": "rich_math_inline",
                    "data-rich-kind": "inline-math",
                    "dir": "ltr",
                    "inline": "",
                },
            )
            return result + serialize_string(text.data) + self._context.pop_tag()
        if kind == T.CustomEmoji:
            return self._render_custom_emoji(text)
        if kind == T.Spoiler:
            result = self._context.push_tag(
                "span", {"class": "spoiler hidden", "inline": "", "onclick": "ShowSpoiler(this)"}
            )
            result += self._context.push_tag("span", {"aria-hidden": "true", "inline": ""})
            result += self._render_children(text)
            return result + self._context.pop_tag() + self._context.pop_tag()
        if kind == T.Mention:
            target = plain_target(text)
            href = (
                safe_mention_href(target, self._internal_links_domain)
                if target is not None
                else None
            )
            return self._render_link(text, href, {})
        if kind in (T.Hashtag, T.BotCommand, T.Cashtag):
            return self._render_action(text)
        if kind in (T.AutoUrl, T.AutoEmail, T.AutoPhone):
            target = plain_target(text)
            check = {
                T.AutoUrl: safe_http_href,
                T.AutoEmail: safe_email_href,
                T.AutoPhone: safe_phone_href,
            }[kind]
            return self._render_link(text, check(target) if target is not None else None, {})
        if kind == T.MentionName:
            return self._render_link(
                text,
                "",
                {
                    "data-user-id": number_to_string(text.id),
                    "onclick": "return ShowMentionName()",
                },
            )
        if kind == T.FormattedDate:
            return self._render_formatted_date(text)
        if kind == T.InlineImage:
            result = self._context.push_tag(
                "span",
                {
                    "class": "rich_inline_fallback",
                    "data-document-id": number_to_string(text.id),
                    "data-rich-kind": "inline-image",
                    "data-source-height": number_to_string(text.height),
                    "data-source-width": number_to_string(text.width),
                    "inline": "",
                },
            )
            return result + "Inline image" + self._context.pop_tag()
        if kind == T.Diff:
            return self._render_diff(text)
        if kind == T.Button:
            return self.render_button(text, True)
        raise ValueError("Type in RichHtmlRenderer::renderText.")

    def _render_custom_emoji(self, text: RichText) -> str:
        result = self._context.push_tag(
            "span",
            {
                "class": "rich_custom_emoji",
                "data-document-id": number_to_string(text.id),
                "inline": "",
            },
        )
        alt = serialize_string(text.text)
        data = text.custom_emoji_data
        unavailable = data == TextPart.unavailable_emoji()
        not_loaded = not data or is_ascii_decimal(data)
        href = (
            None
            if unavailable or not_loaded
            else safe_relative_emoji_href(data, self._relative_base)
        )
        if href is not None:
            result += self._render_custom_emoji_link(alt, {"href": href})
        elif unavailable or not_loaded:
            onclick = (
                "return ShowNotAvailableEmoji()" if unavailable else "return ShowNotLoadedEmoji()"
            )
            result += self._render_custom_emoji_link(alt, {"href": "", "onclick": onclick})
        else:
            result += alt
        return result + self._context.pop_tag()

    def _render_action(self, text: RichText) -> str:
        target = plain_target(text)
        prefix = {T.Hashtag: "#", T.BotCommand: "/", T.Cashtag: "$"}[text.type]
        action = safe_action_data(target, prefix) if target is not None else None
        if action is None:
            return self._render_link(text, None, {})
        attribute = "data-command" if text.type == T.BotCommand else "data-tag"
        callback = {
            T.Hashtag: "return ShowHashtag(this.dataset.tag)",
            T.BotCommand: "return ShowBotCommand(this.dataset.command)",
            T.Cashtag: "return ShowCashtag(this.dataset.tag)",
        }[text.type]
        return self._render_link(text, "", {attribute: action, "onclick": callback})

    def _render_formatted_date(self, text: RichText) -> str:
        result = self._context.push_tag(
            "time",
            {
                "class": "rich_formatted_date",
                "data-date": number_to_string(text.date),
                "data-day-of-week": rich_bool(text.day_of_week),
                "data-long-date": rich_bool(text.long_date),
                "data-long-time": rich_bool(text.long_time),
                "data-relative": rich_bool(text.relative),
                "data-short-date": rich_bool(text.short_date),
                "data-short-time": rich_bool(text.short_time),
                "datetime": iso_date_time(text.date),
                "dir": "ltr",
                "inline": "",
                "title": format_date_time(text.date),
            },
        )
        return result + self._render_children(text) + self._context.pop_tag()

    def _render_diff(self, text: RichText) -> str:
        result = self._context.push_tag(
            "span", {"class": "rich_diff", "data-rich-kind": "diff", "inline": ""}
        )
        result += self._render_children(text)
        result += self._context.push_tag("del", {"class": "rich_diff_previous", "inline": ""})
        result += "Previous: "
        if not text.old_children:
            result += self._context.push_tag("span", dict(_UNSUPPORTED_TEXT))
            result += "Unsupported rich text"
            result += self._context.pop_tag()
        else:
            result += self.render_texts(text.old_children)
        return result + self._context.pop_tag() + self._context.pop_tag()
