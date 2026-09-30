"""Parsing of rich messages and the rich message visitor.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.cpp
ParseRichText/ParseRichBlock/ParseRichMessage, export_api_wrap.cpp VisitRichMessage/
ExtractFullRichMessage/RefreshRichMessageFileReference), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .model import (
    Document,
    FileLocation,
    ParseMediaContext,
    Photo,
    parse_document,
    parse_photo,
    prepare_photo_file_name,
    refresh_file_reference,
    to_time,
)
from .model_format import number_to_string
from .model_rich import (
    InlineButtonAction,
    InlineButtonPeerType,
    RichBlock,
    RichButtonAlignment,
    RichButtonPayload,
    RichButtonStyle,
    RichCaption,
    RichChannel,
    RichListItem,
    RichListItemContent,
    RichListKind,
    RichMapPoint,
    RichMessage,
    RichOrderedList,
    RichQuoteContent,
    RichRelatedArticle,
    RichTableAlignment,
    RichTableCell,
    RichTableRow,
    RichTableVerticalAlignment,
    RichTaskState,
    RichText,
)

_A = InlineButtonAction.Type

_INLINE_PEER_TYPES = {
    tl.InlineQueryPeerTypeSameBotPM: InlineButtonPeerType.SameBotPM,
    tl.InlineQueryPeerTypePM: InlineButtonPeerType.PM,
    tl.InlineQueryPeerTypeChat: InlineButtonPeerType.Chat,
    tl.InlineQueryPeerTypeMegagroup: InlineButtonPeerType.Megagroup,
    tl.InlineQueryPeerTypeBroadcast: InlineButtonPeerType.Broadcast,
    tl.InlineQueryPeerTypeBotPM: InlineButtonPeerType.BotPM,
}


def parse_inline_button_action(data: Any) -> InlineButtonAction | None:
    action = InlineButtonAction()
    if isinstance(data, tl.InlineButtonTypeUrl):
        action.type, action.url = _A.Url, data.url
    elif isinstance(data, tl.InlineButtonTypeUrlAuth):
        action.type, action.url, action.button_id = _A.Auth, data.url, data.button_id
        action.forward_text = data.fwd_text
    elif isinstance(data, tl.InlineButtonTypeWebView):
        action.type, action.url = _A.WebView, data.url
    elif isinstance(data, tl.InlineButtonTypeCallback):
        action.requires_password = bool(data.requires_password)
        action.type = _A.CallbackWithPassword if action.requires_password else _A.Callback
        action.callback_data = bytes(data.data)
    elif isinstance(data, tl.InlineButtonTypeGame):
        action.type = _A.Game
    elif isinstance(data, tl.InlineButtonTypeBuy):
        action.type = _A.Buy
    elif isinstance(data, tl.InlineButtonTypeSwitchInline):
        action.same_peer = bool(data.same_peer)
        action.type = _A.SwitchInlineSame if action.same_peer else _A.SwitchInline
        action.query = data.query
        if data.peer_types is not None:
            action.peer_types = [_INLINE_PEER_TYPES[type(t)] for t in data.peer_types]
    elif isinstance(data, tl.InlineButtonTypeUserProfile):
        action.type, action.user_id = _A.UserProfile, data.user_id
    elif isinstance(data, tl.InlineButtonTypeCopy):
        action.type, action.copy_text = _A.CopyText, data.copy_text
    elif isinstance(data, tl.InlineButtonTypeDisabled):
        action.type = _A.Disabled
    else:
        return None
    return action


def _parse_rich_button_style(
    style: Any, action_type: InlineButtonAction.Type, inline_button: bool
) -> RichButtonStyle | None:
    if style is None:
        return None
    link_allowed = action_type in (_A.Callback, _A.CallbackWithPassword, _A.Disabled)
    if inline_button and style.link and link_allowed:
        return RichButtonStyle.Link
    if style.bg_danger:
        return RichButtonStyle.Danger
    if style.bg_primary:
        return RichButtonStyle.Primary
    if style.bg_success:
        return RichButtonStyle.Success
    return RichButtonStyle.Default


def _parse_rich_button(
    text: Any, button_type: Any, style: Any, inline_button: bool
) -> RichText | None:
    action = parse_inline_button_action(button_type)
    if action is None:
        return None
    return RichText(
        type=RichText.Type.Button,
        children=[parse_rich_text(text)],
        button=RichButtonPayload(
            action=action,
            style=_parse_rich_button_style(style, action.type, inline_button),
        ),
    )


_RT = RichText.Type
_WRAPPERS: dict[type, RichText.Type] = {
    tl.TextBold: _RT.Bold,
    tl.TextItalic: _RT.Italic,
    tl.TextUnderline: _RT.Underline,
    tl.TextStrike: _RT.Strike,
    tl.TextFixed: _RT.Fixed,
    tl.TextSubscript: _RT.Subscript,
    tl.TextSuperscript: _RT.Superscript,
    tl.TextMarked: _RT.Marked,
    tl.TextSpoiler: _RT.Spoiler,
    tl.TextMention: _RT.Mention,
    tl.TextHashtag: _RT.Hashtag,
    tl.TextBotCommand: _RT.BotCommand,
    tl.TextCashtag: _RT.Cashtag,
    tl.TextAutoUrl: _RT.AutoUrl,
    tl.TextAutoEmail: _RT.AutoEmail,
    tl.TextAutoPhone: _RT.AutoPhone,
    tl.TextBankCard: _RT.BankCard,
}


def _wrapper(kind: RichText.Type, child: Any) -> RichText:
    return RichText(type=kind, children=[parse_rich_text(child)])


def parse_rich_text(text: Any) -> RichText:
    kind = _WRAPPERS.get(type(text))
    if kind is not None:
        return _wrapper(kind, text.text)
    if isinstance(text, tl.TextPlain):
        return RichText(type=_RT.Plain, text=text.text)
    if isinstance(text, tl.TextUrl):
        result = _wrapper(_RT.Url, text.text)
        result.data, result.id = text.url, text.webpage_id
        return result
    if isinstance(text, tl.TextEmail):
        result = _wrapper(_RT.Email, text.text)
        result.data = text.email
        return result
    if isinstance(text, tl.TextConcat):
        return RichText(type=_RT.Concat, children=[parse_rich_text(t) for t in text.texts])
    if isinstance(text, tl.TextPhone):
        result = _wrapper(_RT.Phone, text.text)
        result.data = text.phone
        return result
    if isinstance(text, tl.TextImage):
        return RichText(type=_RT.InlineImage, id=text.document_id, width=text.w, height=text.h)
    if isinstance(text, tl.TextAnchor):
        result = _wrapper(_RT.Anchor, text.text)
        result.data = text.name
        return result
    if isinstance(text, tl.TextMath):
        return RichText(type=_RT.Math, data=text.source)
    if isinstance(text, tl.TextCustomEmoji):
        return RichText(
            type=_RT.CustomEmoji,
            text=text.alt,
            id=text.document_id,
            custom_emoji_data=number_to_string(text.document_id),
        )
    if isinstance(text, tl.TextMentionName):
        result = _wrapper(_RT.MentionName, text.text)
        result.id = text.user_id
        return result
    if isinstance(text, tl.TextDate):
        result = _wrapper(_RT.FormattedDate, text.text)
        result.date = to_time(text.date)
        result.relative = bool(text.relative)
        result.short_time = bool(text.short_time)
        result.long_time = bool(text.long_time)
        result.short_date = bool(text.short_date)
        result.long_date = bool(text.long_date)
        result.day_of_week = bool(text.day_of_week)
        return result
    if isinstance(text, tl.TextDiff):
        return RichText(
            type=_RT.Diff,
            unsupported=True,
            children=[parse_rich_text(text.text)],
            old_children=[parse_rich_text(text.old_text)],
        )
    if isinstance(text, tl.TextButton):
        button = _parse_rich_button(text.text, text.type, text.style, True)
        return button if button is not None else parse_rich_text(text.text)
    return RichText(type=_RT.Empty)


def _parse_rich_caption(caption: Any) -> RichCaption:
    return RichCaption(text=parse_rich_text(caption.text), credit=parse_rich_text(caption.credit))


def _task_state(checkbox: Any, checked: Any) -> RichTaskState:
    if not checkbox:
        return RichTaskState.None_
    return RichTaskState.Checked if checked else RichTaskState.Unchecked


def _parse_list_item(item: Any, ordered: bool) -> RichListItem:
    result = RichListItem(task_state=_task_state(item.checkbox, item.checked))
    if ordered:
        result.num, result.value, result.type = item.num, item.value, item.type
    if isinstance(item, (tl.PageListItemBlocks, tl.PageListOrderedItemBlocks)):
        result.content = RichListItemContent.Blocks
        result.blocks = parse_rich_blocks(item.blocks)
    else:
        result.content = RichListItemContent.Text
        result.text = parse_rich_text(item.text)
    return result


def _parse_table_cell(data: tl.PageTableCell) -> RichTableCell:
    result = RichTableCell(header=bool(data.header))
    if data.align_right:
        result.alignment = RichTableAlignment.Right
    elif data.align_center:
        result.alignment = RichTableAlignment.Center
    if data.valign_bottom:
        result.vertical_alignment = RichTableVerticalAlignment.Bottom
    elif data.valign_middle:
        result.vertical_alignment = RichTableVerticalAlignment.Middle
    if data.text is not None:
        result.text = parse_rich_text(data.text)
    result.colspan, result.rowspan = data.colspan, data.rowspan
    return result


def _parse_related_article(data: tl.PageRelatedArticle) -> RichRelatedArticle:
    return RichRelatedArticle(
        url=data.url,
        webpage_id=data.webpage_id,
        title=data.title,
        description=data.description,
        photo_id=data.photo_id,
        author=data.author,
        published_date=None if data.published_date is None else to_time(data.published_date),
    )


def _parse_rich_channel(chat: Any) -> RichChannel:
    source = RichChannel.Source
    result = RichChannel(id=chat.id)
    if isinstance(chat, tl.ChatEmpty):
        result.source = source.ChatEmpty
        return result
    result.title = chat.title
    if isinstance(chat, tl.Chat):
        result.source = source.Chat
    elif isinstance(chat, tl.ChatForbidden):
        result.source = source.ChatForbidden
    elif isinstance(chat, (tl.Channel, tl.ChannelForbidden)):
        forbidden = isinstance(chat, tl.ChannelForbidden)
        result.source = source.ChannelForbidden if forbidden else source.Channel
        result.broadcast = bool(chat.broadcast)
        result.megagroup = bool(chat.megagroup)
        result.monoforum = bool(chat.monoforum)
        result.access_hash = chat.access_hash
        if not forbidden:
            result.username = chat.username
    elif isinstance(chat, (tl.Community, tl.CommunityForbidden)):
        forbidden = isinstance(chat, tl.CommunityForbidden)
        result.source = source.CommunityForbidden if forbidden else source.Community
        result.access_hash = chat.access_hash
    return result


def _parse_rich_map_point(point: Any) -> RichMapPoint:
    source = RichMapPoint.Source
    if isinstance(point, tl.GeoPoint):
        return RichMapPoint(
            source=source.GeoPoint,
            latitude=point.lat,
            longitude=point.long,
            access_hash=point.access_hash,
            accuracy_radius=point.accuracy_radius,
        )
    if isinstance(point, tl.InputGeoPoint):
        return RichMapPoint(
            source=source.InputGeoPoint,
            latitude=point.lat,
            longitude=point.long,
            accuracy_radius=point.accuracy_radius,
        )
    if isinstance(point, tl.InputGeoPointEmpty):
        return RichMapPoint(source=source.InputGeoPointEmpty)
    return RichMapPoint(source=source.GeoPointEmpty)


_K = RichBlock.Kind
_HEADINGS: dict[type, int] = {
    tl.PageBlockTitle: 1,
    tl.PageBlockSubtitle: 2,
    tl.PageBlockHeader: 3,
    tl.PageBlockSubheader: 4,
    tl.PageBlockKicker: 5,
    tl.PageBlockHeading1: 1,
    tl.PageBlockHeading2: 2,
    tl.PageBlockHeading3: 3,
    tl.PageBlockHeading4: 4,
    tl.PageBlockHeading5: 5,
    tl.PageBlockHeading6: 6,
}
_TEXT_BLOCKS: dict[type, RichBlock.Kind] = {
    tl.PageBlockParagraph: _K.Paragraph,
    tl.PageBlockFooter: _K.Footer,
    tl.PageBlockThinking: _K.Thinking,
}


def _text_block(kind: RichBlock.Kind, text: Any) -> RichBlock:
    return RichBlock(kind=kind, text=parse_rich_text(text))


def _parse_media_block(block: Any) -> RichBlock | None:
    if isinstance(block, tl.PageBlockPhoto):
        return RichBlock(
            kind=_K.Photo,
            photo_id=block.photo_id,
            caption=_parse_rich_caption(block.caption),
            spoiler=bool(block.spoiler),
            optional_url=block.url,
            optional_webpage_id=block.webpage_id,
        )
    if isinstance(block, tl.PageBlockVideo):
        return RichBlock(
            kind=_K.Video,
            document_id=block.video_id,
            caption=_parse_rich_caption(block.caption),
            autoplay=bool(block.autoplay),
            loop=bool(block.loop),
            spoiler=bool(block.spoiler),
        )
    if isinstance(block, tl.PageBlockAudio):
        return RichBlock(
            kind=_K.Audio, document_id=block.audio_id, caption=_parse_rich_caption(block.caption)
        )
    if isinstance(block, tl.PageBlockDocument):
        return RichBlock(
            kind=_K.File, document_id=block.document_id, caption=_parse_rich_caption(block.caption)
        )
    if isinstance(block, tl.PageBlockEmbed):
        return RichBlock(
            kind=_K.Embed,
            full_width=bool(block.full_width),
            allow_scrolling=bool(block.allow_scrolling),
            caption=_parse_rich_caption(block.caption),
            optional_url=block.url,
            html=block.html,
            poster_photo_id=block.poster_photo_id,
            width=block.w,
            height=block.h,
        )
    if isinstance(block, tl.PageBlockEmbedPost):
        return RichBlock(
            kind=_K.EmbedPost,
            url=block.url,
            webpage_id=block.webpage_id,
            author_photo_id=block.author_photo_id,
            author=block.author,
            date=to_time(block.date),
            blocks=parse_rich_blocks(block.blocks),
            caption=_parse_rich_caption(block.caption),
        )
    if isinstance(block, (tl.PageBlockCollage, tl.PageBlockSlideshow)):
        kind = _K.Collage if isinstance(block, tl.PageBlockCollage) else _K.Slideshow
        return RichBlock(
            kind=kind,
            blocks=parse_rich_blocks(block.items),
            caption=_parse_rich_caption(block.caption),
        )
    if isinstance(block, (tl.PageBlockMap, tl.InputPageBlockMap)):
        return RichBlock(
            kind=_K.Map if isinstance(block, tl.PageBlockMap) else _K.InputMap,
            map_point=_parse_rich_map_point(block.geo),
            zoom=block.zoom,
            map_width=block.w,
            map_height=block.h,
            caption=_parse_rich_caption(block.caption),
        )
    return None


def _parse_structure_block(block: Any) -> RichBlock | None:
    if isinstance(block, tl.PageBlockList):
        return RichBlock(
            kind=_K.List,
            list_kind=RichListKind.Bullet,
            list_items=[_parse_list_item(item, False) for item in block.items],
        )
    if isinstance(block, tl.PageBlockOrderedList):
        return RichBlock(
            kind=_K.List,
            list_kind=RichListKind.Ordered,
            ordered_list=RichOrderedList(
                type=block.type, start=block.start, reversed=bool(block.reversed)
            ),
            list_items=[_parse_list_item(item, True) for item in block.items],
        )
    if isinstance(block, (tl.PageBlockBlockquote, tl.PageBlockPullquote)):
        result = _text_block(_K.Quote, block.text)
        result.quote_content = RichQuoteContent.Text
        result.quote_caption = parse_rich_text(block.caption)
        result.pullquote = isinstance(block, tl.PageBlockPullquote)
        return result
    if isinstance(block, tl.PageBlockBlockquoteBlocks):
        return RichBlock(
            kind=_K.Quote,
            quote_content=RichQuoteContent.Blocks,
            blocks=parse_rich_blocks(block.blocks),
            quote_caption=parse_rich_text(block.caption),
        )
    if isinstance(block, tl.PageBlockTable):
        result = _text_block(_K.Table, block.title)
        result.bordered = bool(block.bordered)
        result.striped = bool(block.striped)
        result.compact = bool(block.compact)
        result.table_rows = [
            RichTableRow(cells=[_parse_table_cell(cell) for cell in row.cells])
            for row in block.rows
        ]
        return result
    if isinstance(block, tl.PageBlockDetails):
        result = _text_block(_K.Details, block.title)
        result.open = bool(block.open)
        result.blocks = parse_rich_blocks(block.blocks)
        return result
    if isinstance(block, tl.PageBlockRelatedArticles):
        return RichBlock(
            kind=_K.RelatedArticles,
            text=parse_rich_text(block.title),
            related_articles=[_parse_related_article(a) for a in block.articles],
        )
    if isinstance(block, tl.PageBlockButtonRow):
        alignment = (
            RichButtonAlignment.Left
            if block.align_left
            else (
                RichButtonAlignment.Center
                if block.align_center
                else (
                    RichButtonAlignment.Right if block.align_right else RichButtonAlignment.Stretch
                )
            )
        )
        buttons = [_parse_rich_button(b.text, b.type, b.style, False) for b in block.buttons]
        return RichBlock(
            kind=_K.ButtonRow,
            button_alignment=alignment,
            buttons=[b for b in buttons if b is not None],
        )
    return None


def parse_rich_block(block: Any) -> RichBlock:
    level = _HEADINGS.get(type(block))
    if level is not None:
        result = _text_block(_K.Heading, block.text)
        result.heading_level = level
        return result
    kind = _TEXT_BLOCKS.get(type(block))
    if kind is not None:
        return _text_block(kind, block.text)
    if isinstance(block, tl.PageBlockUnsupported):
        return RichBlock(kind=_K.Unsupported, unsupported=True)
    if isinstance(block, tl.PageBlockAuthorDate):
        result = _text_block(_K.AuthorDate, block.author)
        result.date = to_time(block.published_date)
        return result
    if isinstance(block, tl.PageBlockPreformatted):
        result = _text_block(_K.Code, block.text)
        result.language = block.language
        return result
    if isinstance(block, tl.PageBlockDivider):
        return RichBlock(kind=_K.Divider)
    if isinstance(block, tl.PageBlockAnchor):
        return RichBlock(kind=_K.Anchor, name=block.name)
    if isinstance(block, tl.PageBlockCover):
        return RichBlock(kind=_K.Cover, blocks=[parse_rich_block(block.cover)])
    if isinstance(block, tl.PageBlockChannel):
        return RichBlock(kind=_K.Channel, channel=_parse_rich_channel(block.channel))
    if isinstance(block, tl.PageBlockMath):
        return RichBlock(kind=_K.Math, formula=block.source)
    parsed = _parse_media_block(block)
    if parsed is None:
        parsed = _parse_structure_block(block)
    return parsed if parsed is not None else RichBlock()


def parse_rich_blocks(blocks: list[Any]) -> list[RichBlock]:
    return [parse_rich_block(block) for block in blocks]


def parse_rich_message(
    context: ParseMediaContext, data: Any, folder: str, date: int
) -> RichMessage:
    result = RichMessage(rtl=bool(data.rtl), part=bool(data.part))
    for photo in data.photos:
        context.photos += 1
        parsed = parse_photo(
            photo, folder + "photos/" + prepare_photo_file_name(context.photos, date)
        )
        result.photos[parsed.id] = parsed
    for document in data.documents:
        parsed_document = parse_document(context, document, folder, date)
        result.documents[parsed_document.id] = parsed_document
    result.photos = dict(sorted(result.photos.items()))
    result.documents = dict(sorted(result.documents.items()))
    result.blocks = parse_rich_blocks(data.blocks)
    return result


def extract_full_rich_message(messages: list[Any], message_id: int) -> Any | None:
    """ExtractFullRichMessage: the complete rich_message of exactly one matching message."""
    matches = 0
    rich = None
    for entry in messages:
        if getattr(entry, "id", None) != message_id:
            continue
        matches += 1
        if isinstance(entry, tl.Message):
            candidate = entry.rich_message
            if candidate is not None and not candidate.part:
                rich = candidate
    return rich if matches == 1 else None


def refresh_rich_message_file_reference(
    location: FileLocation, message: RichMessage
) -> FileLocation | None:
    refreshed = FileLocation(location.dc_id, location.data)
    for photo in message.photos.values():
        if refresh_file_reference(refreshed, photo.image.file.location):
            return refreshed
    for document in message.documents.values():
        if refresh_file_reference(refreshed, document.file.location):
            return refreshed
        if document.thumb.width > 0 and refresh_file_reference(
            refreshed, document.thumb.file.location
        ):
            return refreshed
    return None


@dataclass
class RichVisitor:
    """The three callbacks of export_api_wrap.cpp VisitRichMessage."""

    text: Callable[[RichText], None] = lambda _text: None
    photo: Callable[[Photo], None] = lambda _photo: None
    document: Callable[[Document], None] = lambda _document: None


def _visit_photo(message: RichMessage, photo_id: int, visitor: RichVisitor) -> None:
    if photo_id in message.photos:
        visitor.photo(message.photos[photo_id])


def _visit_document(message: RichMessage, document_id: int, visitor: RichVisitor) -> None:
    if document_id in message.documents:
        visitor.document(message.documents[document_id])


def _visit_text(message: RichMessage, text: RichText, visitor: RichVisitor) -> None:
    visitor.text(text)
    if text.type == _RT.InlineImage:
        _visit_document(message, text.id, visitor)
    for child in text.children:
        _visit_text(message, child, visitor)
    for child in text.old_children:
        _visit_text(message, child, visitor)


def _visit_caption(message: RichMessage, caption: RichCaption, visitor: RichVisitor) -> None:
    _visit_text(message, caption.text, visitor)
    _visit_text(message, caption.credit, visitor)


def _visit_blocks(message: RichMessage, blocks: list[RichBlock], visitor: RichVisitor) -> None:
    for block in blocks:
        _visit_block(message, block, visitor)


def _visit_block(message: RichMessage, block: RichBlock, visitor: RichVisitor) -> None:
    kind = block.kind
    if kind in (_K.Heading, _K.Paragraph, _K.Footer, _K.Thinking, _K.AuthorDate, _K.Code):
        _visit_text(message, block.text, visitor)
    elif kind == _K.List:
        for item in block.list_items:
            if item.content == RichListItemContent.Text:
                if item.text is not None:
                    _visit_text(message, item.text, visitor)
            else:
                _visit_blocks(message, item.blocks, visitor)
    elif kind == _K.Quote:
        if block.quote_content == RichQuoteContent.Text:
            _visit_text(message, block.text, visitor)
        else:
            _visit_blocks(message, block.blocks, visitor)
        _visit_text(message, block.quote_caption, visitor)
    elif kind == _K.Photo:
        _visit_photo(message, block.photo_id, visitor)
        _visit_caption(message, block.caption, visitor)
    elif kind in (_K.Video, _K.Audio, _K.File):
        _visit_document(message, block.document_id, visitor)
        _visit_caption(message, block.caption, visitor)
    elif kind == _K.Cover:
        _visit_block(message, block.blocks[0], visitor)
    elif kind == _K.Embed:
        if block.poster_photo_id is not None:
            _visit_photo(message, block.poster_photo_id, visitor)
        _visit_caption(message, block.caption, visitor)
    elif kind == _K.EmbedPost:
        _visit_photo(message, block.author_photo_id, visitor)
        _visit_blocks(message, block.blocks, visitor)
        _visit_caption(message, block.caption, visitor)
    elif kind in (_K.Collage, _K.Slideshow):
        _visit_blocks(message, block.blocks, visitor)
        _visit_caption(message, block.caption, visitor)
    elif kind == _K.Table:
        _visit_text(message, block.text, visitor)
        for row in block.table_rows:
            for cell in row.cells:
                if cell.text is not None:
                    _visit_text(message, cell.text, visitor)
    elif kind == _K.Details:
        _visit_text(message, block.text, visitor)
        _visit_blocks(message, block.blocks, visitor)
    elif kind == _K.RelatedArticles:
        _visit_text(message, block.text, visitor)
        for article in block.related_articles:
            if article.photo_id is not None:
                _visit_photo(message, article.photo_id, visitor)
    elif kind in (_K.Map, _K.InputMap):
        _visit_caption(message, block.caption, visitor)
    elif kind == _K.ButtonRow:
        for button in block.buttons:
            _visit_text(message, button, visitor)


def visit_rich_message(message: RichMessage, visitor: RichVisitor) -> None:
    _visit_blocks(message, message.blocks, visitor)
