"""Rich-message rendering, part two: media blocks, groups, embeds and references.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
RichMediaCallbacks, RichTailState, RichHtmlRenderer::findPhoto/findDocument/mediaItemSize/
mediaGroupSizes/tailState/renderCaption/renderSourceMeta/renderPhoto/renderVideo/renderAudio/
renderFile/renderMediaGroup/renderCollage/renderSlideshow/renderEmbed/renderEmbedPost/
renderChannel/renderMap/renderRelatedArticle(s)), GPL-3.0. The structural blocks and the
message wrapper live in html_rich.py.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Callable

from .html_layout import Rect, layout_media_group_geometry, united
from .html_rich_support import (
    MediaData,
    rich_aspect_ratio_style,
    rich_block_attributes,
    rich_bool,
    rich_caption_has_output,
    rich_channel_source_name,
    rich_channel_status,
    rich_file_description,
    rich_map_coordinates,
    rich_map_point_source_name,
    rich_percent_value,
    rich_text_has_output,
)
from .html_rich_text import K, RichTextRenderer
from .html_safe import (
    RICH_USERNAME_MAX_LENGTH,
    RICH_USERNAME_MIN_LENGTH,
    is_ascii_word,
    safe_http_href,
)
from .html_text import HtmlContext, serialize_string
from .model import Document, Photo
from .model_format import format_date_time, number_to_string
from .model_rich import (
    RichBlock,
    RichCaption,
    RichListItemContent,
    RichMessage,
    RichQuoteContent,
    RichRelatedArticle,
)

GROUP_WIDTH_MAX = 430
GROUP_WIDTH_MIN = 100
GROUP_SKIP = 4


@dataclass
class RichMediaCallbacks:
    photo: Callable[[Photo | None], str]
    video: Callable[[Document | None], str]
    audio: Callable[[Document | None], str]
    file: Callable[[Document | None], str]
    generic: Callable[[MediaData], str]
    photo_card: Callable[[MediaData, Photo | None], str]


class RichTailState(Enum):
    Empty = 0
    Media = 1
    Other = 2


_TAIL_OTHER = {
    K.Unsupported,
    K.Heading,
    K.Paragraph,
    K.Footer,
    K.Thinking,
    K.AuthorDate,
    K.Code,
    K.Divider,
    K.Math,
    K.Table,
    K.ButtonRow,
    K.Unknown,
}


class RichMediaRenderer(RichTextRenderer):
    def __init__(
        self,
        context: HtmlContext,
        message: RichMessage,
        message_id: int,
        internal_links_domain: str,
        relative_base: str,
        media: RichMediaCallbacks,
    ) -> None:
        super().__init__(context, message, message_id, internal_links_domain, relative_base)
        self._media = media

    def _find_photo(self, photo_id: int) -> Photo | None:
        return self._message.photos.get(photo_id) if photo_id else None

    def _find_document(self, document_id: int) -> Document | None:
        return self._message.documents.get(document_id) if document_id else None

    def _media_item_size(self, block: RichBlock) -> tuple[int, int] | None:
        if block.kind == K.Photo:
            photo = self._find_photo(block.photo_id)
            if photo is None or not photo.image.file.relative_path:
                return None
            size = (photo.image.width, photo.image.height)
        elif block.kind == K.Video:
            document = self._find_document(block.document_id)
            if (
                document is None
                or not document.file.relative_path
                or not document.thumb.file.relative_path
            ):
                return None
            size = (document.width, document.height)
        else:
            return None
        return size if size[0] > 0 and size[1] > 0 else None

    def _media_group_sizes(self, blocks: list[RichBlock]) -> list[tuple[int, int]]:
        result = []
        for block in blocks:
            size = self._media_item_size(block)
            if size is None or rich_caption_has_output(block.caption) or block.optional_url:
                return []
            result.append(size)
        return result

    def _tail_state_of(self, blocks: list[RichBlock]) -> RichTailState:
        for block in reversed(blocks):
            state = self._tail_state(block)
            if state != RichTailState.Empty:
                return state
        return RichTailState.Empty

    def _tail_state(self, block: RichBlock) -> RichTailState:
        kind, other, media = block.kind, RichTailState.Other, RichTailState.Media
        if kind in _TAIL_OTHER:
            return other
        if kind == K.Anchor:
            return RichTailState.Empty
        if kind == K.List:
            if not block.list_items:
                return RichTailState.Empty
            item = block.list_items[-1]
            if item.content == RichListItemContent.Blocks:
                state = self._tail_state_of(item.blocks)
                if state != RichTailState.Empty:
                    return state
            return other
        if kind == K.Quote:
            if rich_text_has_output(block.quote_caption):
                return other
            if block.quote_content == RichQuoteContent.Blocks:
                state = self._tail_state_of(block.blocks)
                if state != RichTailState.Empty:
                    return state
            return other
        if kind == K.Photo:
            has_url = bool(block.optional_url)
            return other if rich_caption_has_output(block.caption) or has_url else media
        if kind in (K.Video, K.Audio, K.File, K.Map, K.InputMap):
            return other if rich_caption_has_output(block.caption) else media
        if kind == K.Cover:
            return self._tail_state_of(block.blocks)
        if kind == K.Embed:
            has_meta = block.poster_photo_id is not None and bool(block.optional_url)
            return other if rich_caption_has_output(block.caption) or has_meta else media
        if kind == K.EmbedPost:
            if rich_caption_has_output(block.caption):
                return other
            state = self._tail_state_of(block.blocks)
            if state != RichTailState.Empty:
                return state
            return media if not block.url else other
        if kind in (K.Collage, K.Slideshow):
            if rich_caption_has_output(block.caption):
                return other
            return self._tail_state_of(block.blocks)
        if kind == K.Channel:
            return media
        if kind == K.Details:
            if block.open:
                state = self._tail_state_of(block.blocks)
                if state != RichTailState.Empty:
                    return state
            return other
        if kind == K.RelatedArticles:
            if not block.related_articles:
                return other if rich_text_has_output(block.text) else RichTailState.Empty
            article = block.related_articles[-1]
            if article.url:
                return other
            description = article.description or ""
            if article.photo_id is not None and description:
                photo = self._find_photo(article.photo_id)
                if rich_file_description(photo.image.file if photo else None):
                    return other
            return media
        raise ValueError("Kind in RichHtmlRenderer::tailState.")

    def _render_caption(self, caption: RichCaption) -> str:
        text = self.render_text(caption.text)
        credit = self.render_text(caption.credit)
        if not text and not credit:
            return ""
        result = self._context.push_tag(
            "figcaption", {"class": "rich_media_caption", "dir": "auto"}
        )
        if text:
            result += self._context.push_tag("div", {"class": "rich_caption_text"})
            result += text + self._context.pop_tag()
        if credit:
            result += self._context.push_tag("cite", {"class": "rich_caption_credit"})
            result += credit + self._context.pop_tag()
        return result + self._context.pop_tag()

    def _render_source_meta(self, source: str, embed_post: bool) -> str:
        if not source:
            return ""
        container = "rich_embed_post_meta" if embed_post else "rich_source_meta"
        result = self._context.push_tag("div", {"class": container})
        href = safe_http_href(source)
        if href is not None and not self._link_active:
            result += self._context.push_tag(
                "a", {"class": "rich_source_link", "href": href, "inline": ""}
            )
        else:
            result += self._context.push_tag("span", {"class": "rich_source_text", "inline": ""})
        result += serialize_string(source) + self._context.pop_tag()
        return result + self._context.pop_tag()

    def _render_photo(self, block: RichBlock) -> str:
        attributes = rich_block_attributes("rich_media_item", "photo")
        attributes.setdefault("data-photo-id", number_to_string(block.photo_id))
        attributes.setdefault("data-spoiler", rich_bool(block.spoiler))
        if block.optional_webpage_id is not None:
            attributes.setdefault("data-webpage-id", number_to_string(block.optional_webpage_id))
        result = self._context.push_tag("figure", attributes)
        result += self._media.photo(self._find_photo(block.photo_id))
        if block.optional_url is not None:
            result += self._render_source_meta(block.optional_url, False)
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_document(self, block: RichBlock, kind: str) -> str:
        attributes = rich_block_attributes("rich_media_item", kind)
        attributes.setdefault("data-document-id", number_to_string(block.document_id))
        if kind == "video":
            attributes.setdefault("data-autoplay", rich_bool(block.autoplay))
            attributes.setdefault("data-loop", rich_bool(block.loop))
            attributes.setdefault("data-spoiler", rich_bool(block.spoiler))
        callback = {"video": self._media.video, "audio": self._media.audio}.get(
            kind, self._media.file
        )
        result = self._context.push_tag("figure", attributes)
        result += callback(self._find_document(block.document_id))
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_media_group(self, block: RichBlock, kind: str, group_caption: bool) -> str:
        attributes = rich_block_attributes("rich_media_group", kind)
        attributes.setdefault("data-rich-has-items", rich_bool(bool(block.blocks)))
        attributes.setdefault(
            "data-rich-items-end-media",
            rich_bool(self._tail_state_of(block.blocks) == RichTailState.Media),
        )
        result = self._context.push_tag("section", attributes)
        result += self._context.push_tag("div", {"class": "rich_media_group_items"})
        result += self._render_blocks(block.blocks) + self._context.pop_tag()
        if group_caption:
            result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _full_group_attributes(self, kind: str) -> dict[str, str]:
        attributes = rich_block_attributes("rich_media_group", kind)
        attributes.setdefault("data-rich-has-items", rich_bool(True))
        attributes.setdefault("data-rich-items-end-media", rich_bool(True))
        return attributes

    def _render_collage(self, block: RichBlock) -> str:
        sizes = self._media_group_sizes(block.blocks)
        if not sizes:
            return self._render_media_group(block, "collage", True)
        layout = layout_media_group_geometry(sizes, GROUP_WIDTH_MAX, GROUP_WIDTH_MIN, GROUP_SKIP)
        full: Rect | None = None
        for part in layout:
            full = united(full, part)
        if len(layout) != len(sizes) or full is None or full[2] <= 0 or full[3] <= 0:
            return self._render_media_group(block, "collage", True)
        fx, fy, fw, fh = full
        result = self._context.push_tag("section", self._full_group_attributes("collage"))
        result += self._context.push_tag(
            "div", {"class": "rich_collage_box", "style": rich_aspect_ratio_style(fw, fh)}
        )
        for (x, y, w, h), item in zip(layout, block.blocks):
            style = (
                f"left: {rich_percent_value(x - fx, fw)}; top: {rich_percent_value(y - fy, fh)};"
                f" width: {rich_percent_value(w, fw)}; height: {rich_percent_value(h, fh)}"
            )
            result += self._context.push_tag("div", {"class": "rich_collage_item", "style": style})
            result += self._render_block(item) + self._context.pop_tag()
        result += self._context.pop_tag()
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_slideshow(self, block: RichBlock) -> str:
        sizes = self._media_group_sizes(block.blocks)
        if not sizes:
            return self._render_media_group(block, "slideshow", True)
        widest = sizes[0]
        for size in sizes:
            if size[0] * widest[1] > widest[0] * size[1]:
                widest = size
        count = len(block.blocks)
        result = self._context.push_tag("section", self._full_group_attributes("slideshow"))
        result += self._context.push_tag(
            "div", {"class": "rich_slideshow_box", "style": rich_aspect_ratio_style(*widest)}
        )
        result += self._context.push_tag("div", {"class": "rich_slideshow_track"})
        for item in block.blocks:
            result += self._context.push_tag("div", {"class": "rich_slide"})
            result += self._render_block(item) + self._context.pop_tag()
        result += self._context.pop_tag()
        if count > 1:
            for name, arrow in (("prev", "&#8592;"), ("next", "&#8594;")):
                result += self._context.push_tag(
                    "button", {"class": f"rich_slideshow_{name}", "type": "button", "inline": ""}
                )
                result += arrow + self._context.pop_tag()
            result += self._context.push_tag("div", {"class": "rich_slideshow_dots"})
            for _ in range(count):
                result += self._context.push_tag(
                    "button", {"class": "rich_slideshow_dot", "type": "button", "inline": ""}
                )
                result += self._context.pop_tag()
            result += self._context.pop_tag()
        result += self._context.pop_tag()
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_embed(self, block: RichBlock) -> str:
        has_poster = block.poster_photo_id is not None
        has_meta = has_poster and bool(block.optional_url)
        attributes = rich_block_attributes("rich_media_item rich_embed", "embed")
        attributes.setdefault("data-full-width", rich_bool(block.full_width))
        attributes.setdefault("data-allow-scrolling", rich_bool(block.allow_scrolling))
        attributes.setdefault("data-rich-has-meta", rich_bool(has_meta))
        if block.poster_photo_id is not None:
            attributes.setdefault("data-poster-photo-id", number_to_string(block.poster_photo_id))
        if block.width is not None:
            attributes.setdefault("data-source-width", number_to_string(block.width))
        if block.height is not None:
            attributes.setdefault("data-source-height", number_to_string(block.height))
        result = self._context.push_tag("figure", attributes)
        if block.poster_photo_id is not None:
            result += self._media.photo(self._find_photo(block.poster_photo_id))
            if block.optional_url is not None:
                result += self._render_source_meta(block.optional_url, False)
        else:
            media = MediaData(title="Embed", classes="media_file")
            if block.optional_url is not None:
                media.status = block.optional_url
                href = safe_http_href(block.optional_url)
                if href is not None:
                    media.link = href
            result += self._media.generic(media)
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_embed_post(self, block: RichBlock) -> str:
        photo = self._find_photo(block.author_photo_id)
        attributes = rich_block_attributes("rich_embed_post", "embed-post")
        attributes.setdefault("data-webpage-id", number_to_string(block.webpage_id))
        attributes.setdefault("data-author-photo-id", number_to_string(block.author_photo_id))
        attributes.setdefault("data-date", number_to_string(block.date))
        attributes.setdefault("data-rich-has-meta", rich_bool(bool(block.url)))
        attributes.setdefault("data-rich-has-items", rich_bool(bool(block.blocks)))
        attributes.setdefault(
            "data-rich-items-end-media",
            rich_bool(self._tail_state_of(block.blocks) == RichTailState.Media),
        )
        result = self._context.push_tag("section", attributes)
        result += self._context.push_tag("div", {"class": "rich_embed_post_author"})
        media = MediaData(
            title=block.author or "Embed post",
            description=rich_file_description(photo.image.file if photo else None),
            status=format_date_time(block.date) if block.date else "",
            classes="media_contact",
        )
        result += self._media.photo_card(media, photo) + self._context.pop_tag()
        result += self._render_source_meta(block.url, True)
        result += self._context.push_tag("div", {"class": "rich_embed_post_blocks"})
        result += self._render_blocks(block.blocks) + self._context.pop_tag()
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_channel(self, block: RichBlock) -> str:
        channel = block.channel
        attributes = rich_block_attributes("rich_reference_block", "channel")
        attributes.setdefault("data-channel-source", rich_channel_source_name(channel.source))
        attributes.setdefault("data-channel-id", number_to_string(channel.id))
        attributes.setdefault("data-broadcast", rich_bool(channel.broadcast))
        attributes.setdefault("data-megagroup", rich_bool(channel.megagroup))
        attributes.setdefault("data-monoforum", rich_bool(channel.monoforum))
        if channel.access_hash is not None:
            attributes.setdefault("data-access-hash", number_to_string(channel.access_hash))
        result = self._context.push_tag("section", attributes)
        media = MediaData(
            title=channel.title or "Channel",
            status=rich_channel_status(channel),
            classes="media_contact",
        )
        if channel.username is not None and is_ascii_word(
            channel.username, RICH_USERNAME_MIN_LENGTH, RICH_USERNAME_MAX_LENGTH
        ):
            href = safe_http_href(self._internal_links_domain + channel.username)
            if href is not None:
                media.link = href
        result += self._media.generic(media)
        return result + self._context.pop_tag()

    def _render_map(self, block: RichBlock, kind: str) -> str:
        point = block.map_point
        attributes = rich_block_attributes("rich_reference_block", kind)
        attributes.setdefault("data-point-source", rich_map_point_source_name(point.source))
        attributes.setdefault("data-zoom", number_to_string(block.zoom))
        attributes.setdefault("data-source-width", number_to_string(block.map_width))
        attributes.setdefault("data-source-height", number_to_string(block.map_height))
        if point.access_hash is not None:
            attributes.setdefault("data-access-hash", number_to_string(point.access_hash))
        if point.accuracy_radius is not None:
            attributes.setdefault("data-accuracy-radius", number_to_string(point.accuracy_radius))
        result = self._context.push_tag("figure", attributes)
        media = MediaData(title="Location", classes="media_location")
        coordinates = rich_map_coordinates(point)
        if coordinates is not None:
            latitude, longitude = coordinates
            joined = latitude + "," + longitude
            media.status = latitude + ", " + longitude
            target = f"https://maps.google.com/maps?q={joined}&ll={joined}&z=16"
            href = safe_http_href(target)
            if href is not None:
                media.link = href
        result += self._media.generic(media)
        result += self._render_caption(block.caption)
        return result + self._context.pop_tag()

    def _render_related_article(self, article: RichRelatedArticle, index: int) -> str:
        attributes = {
            "class": "rich_related_article",
            "data-rich-article-index": number_to_string(index),
            "data-webpage-id": number_to_string(article.webpage_id),
        }
        if article.photo_id is not None:
            attributes.setdefault("data-photo-id", number_to_string(article.photo_id))
        if article.published_date is not None:
            attributes.setdefault("data-published-date", number_to_string(article.published_date))
        result = self._context.push_tag("article", attributes)
        media = MediaData(title=article.title or "Related article", status=article.author or "")
        published = format_date_time(article.published_date) if article.published_date else ""
        if published:
            if media.status:
                media.status += ", "
            media.status += published
        description = article.description or ""
        displaced = False
        if article.photo_id is not None:
            photo = self._find_photo(article.photo_id)
            file_description = rich_file_description(photo.image.file if photo else None)
            media.classes = "media_photo"
            media.description = file_description or description
            displaced = bool(file_description) and bool(description)
            result += self._media.photo_card(media, photo)
        else:
            media.classes = "media_file"
            media.description = description
            result += self._media.generic(media)
        if displaced:
            result += self._context.push_tag("div", {"class": "rich_article_description"})
            result += serialize_string(description) + self._context.pop_tag()
        result += self._render_source_meta(article.url, False)
        return result + self._context.pop_tag()

    def _render_related_articles(self, block: RichBlock) -> str:
        title = self.render_text(block.text)
        result = self._context.push_tag(
            "section", rich_block_attributes("rich_related_articles", "related-articles")
        )
        if title:
            result += self._context.push_tag("div", {"class": "rich_related_title", "dir": "auto"})
            result += title + self._context.pop_tag()
        result += self._context.push_tag("div", {"class": "rich_related_items"})
        for index, article in enumerate(block.related_articles):
            result += self._render_related_article(article, index)
        return result + self._context.pop_tag() + self._context.pop_tag()

    def _render_blocks(self, blocks: list[RichBlock]) -> str:
        return "".join(self._render_block(block) for block in blocks)

    def _render_block(self, block: RichBlock) -> str:
        raise NotImplementedError
