"""JSON for rich messages: rich text, page blocks, captions, tables, lists and buttons.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_json.cpp
SerializeRichText ... SerializeRichMessage), GPL-3.0.

Each function keeps tdesktop's push/pop of the nesting context around the values it computes,
because the indentation of every nested value depends on it.
"""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import Callable, Sequence, TypeVar

from .files import SkipReason
from .json_serialize import (
    K_ARRAY,
    K_OBJECT,
    JsonContext,
    Pairs,
    number,
    serialize_array,
    serialize_date,
    serialize_date_raw,
    serialize_object,
    serialize_string,
    u64,
)
from .model import Document, File, Photo
from .model_rich import (
    InlineButtonAction,
    RichBlock,
    RichCaption,
    RichChannel,
    RichListItem,
    RichListItemContent,
    RichListKind,
    RichMapPoint,
    RichMessage,
    RichQuoteContent,
    RichRelatedArticle,
    RichTableCell,
    RichTableRow,
    RichText,
    inline_button_peer_type_to_string,
    rich_button_alignment_to_string,
    rich_button_style_to_string,
)

_V = TypeVar("_V")


@dataclass
class RichSerializeContext:
    json: JsonContext
    message: RichMessage


def _bool(value: bool) -> bytes:
    return b"true" if value else b"false"


def _id(value: int) -> bytes:
    """SerializeRichNumberString for a uint64 field."""
    return serialize_string(number(u64(value)))


def _signed(value: int) -> bytes:
    """SerializeRichNumberString for an int64 field (access hashes)."""
    return serialize_string(number(value))


def base64_url(data: bytes) -> bytes:
    """QByteArray::toBase64(Base64UrlEncoding | OmitTrailingEquals)."""
    return base64.urlsafe_b64encode(data).rstrip(b"=")


_SKIP_NAMES = {
    SkipReason.Unavailable: "unavailable",
    SkipReason.FileType: "file_type",
    SkipReason.FileSize: "file_size",
    SkipReason.DateLimits: "date_limits",
}


def _append_file_availability(
    values: Pairs, file: File | None, path_key: str, reason_key: str
) -> None:
    """AppendRichFileAvailability."""
    if file is not None:
        if file.skip_reason in _SKIP_NAMES:
            values.append((reason_key, serialize_string(_SKIP_NAMES[file.skip_reason])))
            return
        if file.relative_path:
            values.append((path_key, serialize_string(file.relative_path)))
            return
    values.append((reason_key, serialize_string("unavailable")))


def _append_photo_metadata(values: Pairs, photo: Photo | None, *keys: str) -> None:
    """AppendRichPhotoMetadata(path, reason, size, width, height keys)."""
    path_key, reason_key, size_key, width_key, height_key = keys
    _append_file_availability(values, photo.image.file if photo else None, path_key, reason_key)
    if photo is None:
        return
    values.append((size_key, number(photo.image.file.size)))
    if photo.image.width and photo.image.height:
        values.append((width_key, number(photo.image.width)))
        values.append((height_key, number(photo.image.height)))


def _append_document_metadata(
    values: Pairs, document: Document | None, include_dimensions: bool
) -> None:
    """AppendRichDocumentMetadata."""
    _append_file_availability(
        values, document.file if document else None, "file", "file_skip_reason"
    )
    if document is None:
        return
    if document.name:
        values.append(("file_name", serialize_string(document.name)))
    values.append(("file_size", number(document.file.size)))
    if document.thumb.width > 0:
        _append_file_availability(
            values, document.thumb.file, "thumbnail", "thumbnail_skip_reason"
        )
        values.append(("thumbnail_file_size", number(document.thumb.file.size)))
    media_type = ""
    if document.is_sticker:
        media_type = "sticker"
    elif document.is_video_message:
        media_type = "video_message"
    elif document.is_voice_message:
        media_type = "voice_message"
    elif document.is_animated:
        media_type = "animation"
    elif document.is_video_file:
        media_type = "video_file"
    elif document.is_audio_file:
        media_type = "audio_file"
    if media_type:
        values.append(("media_type", serialize_string(media_type)))
    if document.is_sticker and document.sticker_emoji:
        values.append(("sticker_emoji", serialize_string(document.sticker_emoji)))
    elif media_type == "audio_file":
        if document.song_performer:
            values.append(("performer", serialize_string(document.song_performer)))
        if document.song_title:
            values.append(("title", serialize_string(document.song_title)))
    if document.mime:
        values.append(("mime_type", serialize_string(document.mime)))
    if document.duration:
        values.append(("duration_seconds", number(document.duration)))
    if include_dimensions and document.width and document.height:
        values.append(("width", number(document.width)))
        values.append(("height", number(document.height)))


def _rich_array(
    context: JsonContext, data: Sequence[_V], serializer: Callable[[_V], bytes]
) -> bytes:
    """SerializeRichArray: "[]" when empty."""
    if not data:
        return b"[]"
    context.nesting.append(K_ARRAY)
    try:
        values = [serializer(value) for value in data]
    finally:
        context.nesting.pop()
    return serialize_array(context, values)


def _object(context: RichSerializeContext, values: Pairs, fill: Callable[[Pairs], None]) -> bytes:
    """The recurring shape: compute nested values one level deeper, then SerializeObject."""
    context.json.nesting.append(K_OBJECT)
    try:
        fill(values)
    finally:
        context.json.nesting.pop()
    return serialize_object(context.json, values)


_T = RichText.Type
_TEXT_TYPE_NAMES = {
    _T.Empty: "empty",
    _T.Plain: "plain",
    _T.Concat: "concat",
    _T.Bold: "bold",
    _T.Italic: "italic",
    _T.Underline: "underline",
    _T.Strike: "strikethrough",
    _T.Fixed: "code",
    _T.Url: "text_link",
    _T.Email: "email",
    _T.Phone: "phone",
    _T.Subscript: "subscript",
    _T.Superscript: "superscript",
    _T.Marked: "marked",
    _T.Anchor: "anchor",
    _T.Math: "math",
    _T.CustomEmoji: "custom_emoji",
    _T.Spoiler: "spoiler",
    _T.Mention: "mention",
    _T.Hashtag: "hashtag",
    _T.BotCommand: "bot_command",
    _T.Cashtag: "cashtag",
    _T.AutoUrl: "link",
    _T.AutoEmail: "email",
    _T.AutoPhone: "phone",
    _T.BankCard: "bank_card",
    _T.MentionName: "mention_name",
    _T.FormattedDate: "formatted_date",
    _T.InlineImage: "inline_image",
    _T.Diff: "diff",
    _T.Button: "button",
}
_WRAPPERS = {
    _T.Bold,
    _T.Italic,
    _T.Underline,
    _T.Strike,
    _T.Fixed,
    _T.Subscript,
    _T.Superscript,
    _T.Marked,
    _T.Spoiler,
    _T.Mention,
    _T.Hashtag,
    _T.BotCommand,
    _T.Cashtag,
    _T.AutoUrl,
    _T.AutoEmail,
    _T.AutoPhone,
    _T.BankCard,
}
_DATA_KEYS = {_T.Email: "email", _T.Phone: "phone", _T.Anchor: "name"}


def _button_action(context: JsonContext, action: InlineButtonAction) -> bytes:
    """SerializeRichButtonAction."""
    kind = InlineButtonAction.Type
    values: Pairs = [("type", serialize_string(InlineButtonAction.type_to_string(action)))]
    context.nesting.append(K_OBJECT)
    try:
        if action.type in (kind.Url, kind.WebView):
            values.append(("url", serialize_string(action.url)))
        elif action.type == kind.Auth:
            values.append(("url", serialize_string(action.url)))
            if action.forward_text is not None:
                values.append(("forward_text", serialize_string(action.forward_text)))
            values.append(("button_id", number(action.button_id)))
        elif action.type in (kind.Callback, kind.CallbackWithPassword):
            values.append(("requires_password", _bool(action.requires_password)))
            values.append(("dataBase64", serialize_string(base64_url(action.callback_data))))
            values.append(("data", serialize_string(b"")))
        elif action.type in (kind.SwitchInline, kind.SwitchInlineSame):
            values.append(("query", serialize_string(action.query)))
            values.append(("same_peer", _bool(action.same_peer)))
            if action.peer_types is not None:
                values.append(
                    (
                        "peer_types",
                        _rich_array(
                            context,
                            action.peer_types,
                            lambda peer: serialize_string(inline_button_peer_type_to_string(peer)),
                        ),
                    )
                )
        elif action.type == kind.UserProfile:
            values.append(("user_id", _id(action.user_id)))
        elif action.type == kind.CopyText:
            values.append(("copy_text", serialize_string(action.copy_text)))
    finally:
        context.nesting.pop()
    return serialize_object(context, values)


def _append_button_fields(values: Pairs, context: RichSerializeContext, button: RichText) -> None:
    """AppendRichButtonFields."""
    assert button.button is not None and len(button.children) == 1
    values.append(("text", serialize_rich_text(context, button.children[0])))
    values.append(("button", _button_action(context.json, button.button.action)))
    if button.button.style is not None:
        values.append(
            ("style", serialize_string(rich_button_style_to_string(button.button.style)))
        )


def _child(context: RichSerializeContext, children: list[RichText]) -> bytes:
    """SerializeRichTextChild."""
    assert len(children) == 1
    return serialize_rich_text(context, children[0])


def _fill_rich_text(context: RichSerializeContext, data: RichText, values: Pairs) -> None:
    kind = data.type
    if kind == _T.Plain:
        values.append(("text", serialize_string(data.text)))
    elif kind == _T.Concat:
        values.append(("text", serialize_rich_texts(context, data.children)))
    elif kind in _WRAPPERS:
        values.append(("text", _child(context, data.children)))
    elif kind == _T.Url:
        values.append(("href", serialize_string(data.data)))
        if data.id:
            values.append(("webpage_id", _id(data.id)))
        values.append(("text", _child(context, data.children)))
    elif kind in _DATA_KEYS:
        values.append((_DATA_KEYS[kind], serialize_string(data.data)))
        values.append(("text", _child(context, data.children)))
    elif kind == _T.Math:
        values.append(("source", serialize_string(data.data)))
    elif kind == _T.CustomEmoji:
        values.append(("text", serialize_string(data.text)))
        values.append(("document_id", serialize_string(data.custom_emoji_data)))
    elif kind == _T.MentionName:
        values.append(("user_id", _id(data.id)))
        values.append(("text", _child(context, data.children)))
    elif kind == _T.FormattedDate:
        values.append(("text", _child(context, data.children)))
        values.append(("date", serialize_date(data.date)))
        values.append(("date_unixtime", serialize_date_raw(data.date)))
        values.append(("relative", _bool(data.relative)))
        values.append(("short_time", _bool(data.short_time)))
        values.append(("long_time", _bool(data.long_time)))
        values.append(("short_date", _bool(data.short_date)))
        values.append(("long_date", _bool(data.long_date)))
        values.append(("day_of_week", _bool(data.day_of_week)))
    elif kind == _T.InlineImage:
        values.append(("document_id", _id(data.id)))
        _append_document_metadata(values, context.message.documents.get(data.id), False)
        values.append(("width", number(data.width)))
        values.append(("height", number(data.height)))
    elif kind == _T.Diff:
        values.append(("unsupported", b"true"))
        values.append(("text", _child(context, data.children)))
        values.append(("old_text", _child(context, data.old_children)))
    elif kind == _T.Button:
        _append_button_fields(values, context, data)


def serialize_rich_text(context: RichSerializeContext, data: RichText) -> bytes:
    values: Pairs = [("type", serialize_string(_TEXT_TYPE_NAMES[data.type]))]
    return _object(context, values, lambda out: _fill_rich_text(context, data, out))


def serialize_rich_texts(context: RichSerializeContext, data: list[RichText]) -> bytes:
    return _rich_array(context.json, data, lambda text: serialize_rich_text(context, text))


def _rich_button(context: RichSerializeContext, data: RichText) -> bytes:
    return _object(context, [], lambda out: _append_button_fields(out, context, data))


def _caption(context: RichSerializeContext, data: RichCaption) -> bytes:
    def fill(values: Pairs) -> None:
        values.append(("text", serialize_rich_text(context, data.text)))
        values.append(("credit", serialize_rich_text(context, data.credit)))

    return _object(context, [], fill)


def _list_item(context: RichSerializeContext, data: RichListItem) -> bytes:
    """SerializeRichListItem."""
    values: Pairs = [
        ("task_state", serialize_string(data.task_state.name.rstrip("_").lower())),
        ("content", serialize_string(data.content.name.lower())),
    ]
    context.json.nesting.append(K_OBJECT)
    try:
        if data.content == RichListItemContent.Text:
            text = data.text if data.text is not None else RichText()
            values.append(("text", serialize_rich_text(context, text)))
        else:
            values.append(("blocks", serialize_rich_blocks(context, data.blocks)))
    finally:
        context.json.nesting.pop()
    if data.num is not None:
        values.append(("num", serialize_string(data.num)))
    if data.value is not None:
        values.append(("value", number(data.value)))
    if data.type is not None:
        values.append(("item_type", serialize_string(data.type)))
    return serialize_object(context.json, values)


def _table_cell(context: RichSerializeContext, data: RichTableCell) -> bytes:
    """SerializeRichTableCell."""
    values: Pairs = []
    if data.text is not None:
        context.json.nesting.append(K_OBJECT)
        try:
            values.append(("text", serialize_rich_text(context, data.text)))
        finally:
            context.json.nesting.pop()
    if data.colspan is not None:
        values.append(("colspan", number(data.colspan)))
    if data.rowspan is not None:
        values.append(("rowspan", number(data.rowspan)))
    values.append(("header", _bool(data.header)))
    values.append(("align", serialize_string(data.alignment.name.lower())))
    values.append(("vertical_align", serialize_string(data.vertical_alignment.name.lower())))
    return serialize_object(context.json, values)


def _table_row(context: RichSerializeContext, data: RichTableRow) -> bytes:
    context.json.nesting.append(K_OBJECT)
    try:
        cells = _rich_array(context.json, data.cells, lambda cell: _table_cell(context, cell))
    finally:
        context.json.nesting.pop()
    return serialize_object(context.json, [("cells", cells)])


def _related_article(context: RichSerializeContext, data: RichRelatedArticle) -> bytes:
    """SerializeRichRelatedArticle."""
    values: Pairs = [("url", serialize_string(data.url)), ("webpage_id", _id(data.webpage_id))]
    if data.title is not None:
        values.append(("title", serialize_string(data.title)))
    if data.description is not None:
        values.append(("description", serialize_string(data.description)))
    if data.photo_id is not None:
        values.append(("photo_id", _id(data.photo_id)))
        _append_photo_metadata(
            values,
            context.message.photos.get(data.photo_id),
            "photo",
            "photo_skip_reason",
            "photo_file_size",
            "photo_width",
            "photo_height",
        )
    if data.author is not None:
        values.append(("author", serialize_string(data.author)))
    if data.published_date is not None:
        values.append(("published_date", serialize_date(data.published_date)))
        values.append(("published_date_unixtime", serialize_date_raw(data.published_date)))
    return serialize_object(context.json, values)


_CHANNEL_SOURCES = {
    RichChannel.Source.ChatEmpty: "chat_empty",
    RichChannel.Source.Chat: "chat",
    RichChannel.Source.ChatForbidden: "chat_forbidden",
    RichChannel.Source.Channel: "channel",
    RichChannel.Source.ChannelForbidden: "channel_forbidden",
    RichChannel.Source.Community: "community",
    RichChannel.Source.CommunityForbidden: "community_forbidden",
}
_MAP_SOURCES = {
    RichMapPoint.Source.GeoPointEmpty: "geo_point_empty",
    RichMapPoint.Source.GeoPoint: "geo_point",
    RichMapPoint.Source.InputGeoPointEmpty: "input_geo_point_empty",
    RichMapPoint.Source.InputGeoPoint: "input_geo_point",
}


def _channel(context: RichSerializeContext, data: RichChannel) -> bytes:
    """SerializeRichChannel."""
    values: Pairs = [
        ("source_type", serialize_string(_CHANNEL_SOURCES[data.source])),
        ("id", _id(data.id)),
    ]
    if data.access_hash is not None:
        values.append(("access_hash", _signed(data.access_hash)))
    if data.title is not None:
        values.append(("title", serialize_string(data.title)))
    if data.username is not None:
        values.append(("username", serialize_string(data.username)))
    if data.source in (RichChannel.Source.Channel, RichChannel.Source.ChannelForbidden):
        values.append(("broadcast", _bool(data.broadcast)))
        values.append(("megagroup", _bool(data.megagroup)))
        values.append(("monoforum", _bool(data.monoforum)))
    return serialize_object(context.json, values)


def _map_point(context: RichSerializeContext, data: RichMapPoint) -> bytes:
    """SerializeRichMapPoint."""
    values: Pairs = [("source_type", serialize_string(_MAP_SOURCES[data.source]))]
    if data.source in (RichMapPoint.Source.GeoPoint, RichMapPoint.Source.InputGeoPoint):
        values.append(("latitude", number(data.latitude)))
        values.append(("longitude", number(data.longitude)))
    if data.access_hash is not None:
        values.append(("access_hash", _signed(data.access_hash)))
    if data.accuracy_radius is not None:
        values.append(("accuracy_radius", number(data.accuracy_radius)))
    return serialize_object(context.json, values)


_K = RichBlock.Kind
_BLOCK_KIND_NAMES = {
    _K.Unsupported: "unsupported",
    _K.Heading: "heading",
    _K.Paragraph: "paragraph",
    _K.Footer: "footer",
    _K.Thinking: "thinking",
    _K.AuthorDate: "author_date",
    _K.Code: "code",
    _K.Divider: "divider",
    _K.Anchor: "anchor",
    _K.List: "list",
    _K.Quote: "quote",
    _K.Photo: "photo",
    _K.Video: "video",
    _K.Cover: "cover",
    _K.Embed: "embed",
    _K.EmbedPost: "embed_post",
    _K.Collage: "collage",
    _K.Slideshow: "slideshow",
    _K.Channel: "channel",
    _K.Audio: "audio",
    _K.File: "file",
    _K.Math: "math",
    _K.Table: "table",
    _K.Details: "details",
    _K.RelatedArticles: "related_articles",
    _K.Map: "map",
    _K.InputMap: "input_map",
    _K.ButtonRow: "button_row",
    _K.Unknown: "unsupported",
}


def _fill_block(context: RichSerializeContext, data: RichBlock, values: Pairs) -> None:
    kind = data.kind
    photo_keys = ("photo", "photo_skip_reason", "photo_file_size", "width", "height")
    if kind == _K.Unsupported:
        values.append(("source_type", serialize_string("page_block_unsupported")))
        values.append(("unsupported", b"true"))
    elif kind == _K.Heading:
        values.append(("level", number(data.heading_level)))
        values.append(("text", serialize_rich_text(context, data.text)))
    elif kind in (_K.Paragraph, _K.Footer, _K.Thinking):
        values.append(("text", serialize_rich_text(context, data.text)))
    elif kind == _K.AuthorDate:
        values.append(("author", serialize_rich_text(context, data.text)))
        values.append(("date", serialize_date(data.date)))
        values.append(("date_unixtime", serialize_date_raw(data.date)))
    elif kind == _K.Code:
        values.append(("text", serialize_rich_text(context, data.text)))
        values.append(("language", serialize_string(data.language)))
    elif kind == _K.Anchor:
        values.append(("name", serialize_string(data.name)))
    elif kind == _K.List:
        _fill_list(context, data, values)
    elif kind == _K.Quote:
        values.append(("content", serialize_string(data.quote_content.name.lower())))
        values.append(("pullquote", _bool(data.pullquote)))
        if data.quote_content == RichQuoteContent.Text:
            values.append(("text", serialize_rich_text(context, data.text)))
        else:
            values.append(("blocks", serialize_rich_blocks(context, data.blocks)))
        values.append(("caption", serialize_rich_text(context, data.quote_caption)))
    elif kind == _K.Photo:
        values.append(("photo_id", _id(data.photo_id)))
        _append_photo_metadata(values, context.message.photos.get(data.photo_id), *photo_keys)
        values.append(("spoiler", _bool(data.spoiler)))
        if data.optional_url is not None:
            values.append(("url", serialize_string(data.optional_url)))
        if data.optional_webpage_id is not None:
            values.append(("webpage_id", _id(data.optional_webpage_id)))
        values.append(("caption", _caption(context, data.caption)))
    elif kind == _K.Video:
        values.append(("document_id", _id(data.document_id)))
        _append_document_metadata(values, context.message.documents.get(data.document_id), True)
        values.append(("autoplay", _bool(data.autoplay)))
        values.append(("loop", _bool(data.loop)))
        values.append(("spoiler", _bool(data.spoiler)))
        values.append(("caption", _caption(context, data.caption)))
    elif kind == _K.Cover:
        assert len(data.blocks) == 1
        values.append(("block", serialize_rich_block(context, data.blocks[0])))
    elif kind == _K.Embed:
        _fill_embed(context, data, values)
    elif kind == _K.EmbedPost:
        _fill_embed_post(context, data, values)
    elif kind in (_K.Collage, _K.Slideshow):
        values.append(("items", serialize_rich_blocks(context, data.blocks)))
        values.append(("caption", _caption(context, data.caption)))
    elif kind == _K.Channel:
        values.append(("channel", _channel(context, data.channel)))
    elif kind in (_K.Audio, _K.File):
        values.append(("document_id", _id(data.document_id)))
        _append_document_metadata(values, context.message.documents.get(data.document_id), True)
        values.append(("caption", _caption(context, data.caption)))
    elif kind == _K.Math:
        values.append(("formula", serialize_string(data.formula)))
    elif kind == _K.Table:
        values.append(("title", serialize_rich_text(context, data.text)))
        values.append(("bordered", _bool(data.bordered)))
        values.append(("striped", _bool(data.striped)))
        values.append(("compact", _bool(data.compact)))
        rows = _rich_array(context.json, data.table_rows, lambda row: _table_row(context, row))
        values.append(("rows", rows))
    elif kind == _K.Details:
        values.append(("title", serialize_rich_text(context, data.text)))
        values.append(("open", _bool(data.open)))
        values.append(("blocks", serialize_rich_blocks(context, data.blocks)))
    elif kind == _K.RelatedArticles:
        values.append(("title", serialize_rich_text(context, data.text)))
        articles = _rich_array(
            context.json, data.related_articles, lambda one: _related_article(context, one)
        )
        values.append(("articles", articles))
    elif kind in (_K.Map, _K.InputMap):
        values.append(("geo", _map_point(context, data.map_point)))
        values.append(("zoom", number(data.zoom)))
        values.append(("width", number(data.map_width)))
        values.append(("height", number(data.map_height)))
        values.append(("caption", _caption(context, data.caption)))
    elif kind == _K.ButtonRow:
        alignment = rich_button_alignment_to_string(data.button_alignment)
        values.append(("alignment", serialize_string(alignment)))
        buttons = _rich_array(context.json, data.buttons, lambda one: _rich_button(context, one))
        values.append(("buttons", buttons))
    elif kind == _K.Unknown:
        values.append(("source_type", serialize_string("unknown")))
        values.append(("unsupported", b"true"))


def _fill_list(context: RichSerializeContext, data: RichBlock, values: Pairs) -> None:
    values.append(("kind", serialize_string(data.list_kind.name.lower())))
    if data.list_kind == RichListKind.Ordered:
        values.append(("reversed", _bool(data.ordered_list.reversed)))
        if data.ordered_list.start is not None:
            values.append(("start", number(data.ordered_list.start)))
        if data.ordered_list.type is not None:
            values.append(("list_type", serialize_string(data.ordered_list.type)))
    items = _rich_array(context.json, data.list_items, lambda item: _list_item(context, item))
    values.append(("items", items))


def _fill_embed(context: RichSerializeContext, data: RichBlock, values: Pairs) -> None:
    if data.optional_url is not None:
        values.append(("url", serialize_string(data.optional_url)))
    if data.html is not None:
        values.append(("html", serialize_string(data.html)))
    if data.poster_photo_id is not None:
        values.append(("poster_photo_id", _id(data.poster_photo_id)))
        _append_photo_metadata(
            values,
            context.message.photos.get(data.poster_photo_id),
            "poster_photo",
            "poster_photo_skip_reason",
            "poster_photo_file_size",
            "poster_photo_width",
            "poster_photo_height",
        )
    if data.width is not None:
        values.append(("width", number(data.width)))
    if data.height is not None:
        values.append(("height", number(data.height)))
    values.append(("full_width", _bool(data.full_width)))
    values.append(("allow_scrolling", _bool(data.allow_scrolling)))
    values.append(("caption", _caption(context, data.caption)))


def _fill_embed_post(context: RichSerializeContext, data: RichBlock, values: Pairs) -> None:
    values.append(("url", serialize_string(data.url)))
    values.append(("webpage_id", _id(data.webpage_id)))
    values.append(("author_photo_id", _id(data.author_photo_id)))
    _append_photo_metadata(
        values,
        context.message.photos.get(data.author_photo_id),
        "author_photo",
        "author_photo_skip_reason",
        "author_photo_file_size",
        "author_photo_width",
        "author_photo_height",
    )
    values.append(("author", serialize_string(data.author)))
    values.append(("date", serialize_date(data.date)))
    values.append(("date_unixtime", serialize_date_raw(data.date)))
    values.append(("blocks", serialize_rich_blocks(context, data.blocks)))
    values.append(("caption", _caption(context, data.caption)))


def serialize_rich_block(context: RichSerializeContext, data: RichBlock) -> bytes:
    values: Pairs = [("type", serialize_string(_BLOCK_KIND_NAMES[data.kind]))]
    return _object(context, values, lambda out: _fill_block(context, data, out))


def serialize_rich_blocks(context: RichSerializeContext, data: list[RichBlock]) -> bytes:
    return _rich_array(context.json, data, lambda block: serialize_rich_block(context, block))


def serialize_rich_message(context: JsonContext, data: RichMessage) -> bytes:
    """SerializeRichMessage."""
    rich = RichSerializeContext(context, data)
    values: Pairs = [("rtl", _bool(data.rtl)), ("part", _bool(data.part))]
    return _object(
        rich, values, lambda out: out.append(("blocks", serialize_rich_blocks(rich, data.blocks)))
    )
