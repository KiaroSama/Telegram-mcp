"""Rich message parsing (Telethon TL -> tdexport Data) and the rich message visitor.

Each expectation is derived by hand from Telegram Desktop v7.2.10's export_data_types.cpp
(ParseInlineButtonAction, ParseRichButtonStyle, ParseRichText, ParseRichBlock, ParseRichChannel,
ParseRichMapPoint, ParseRichTableCell, ParseRichRelatedArticle) and export_api_wrap.cpp
(VisitRichBlock, RefreshRichMessageFileReference). Synthetic TL objects only.
"""

from datetime import datetime, timezone

import pytest
from telethon.tl import types as tl

from telegram_mcp.tdexport.model import Document, File, FileLocation, Image, Photo
from telegram_mcp.tdexport.model_rich import (
    InlineButtonAction,
    InlineButtonPeerType,
    RichBlock,
    RichButtonAlignment,
    RichButtonStyle,
    RichCaption,
    RichChannel,
    RichListItem,
    RichListItemContent,
    RichListKind,
    RichMapPoint,
    RichMessage,
    RichQuoteContent,
    RichRelatedArticle,
    RichTableAlignment,
    RichTableCell,
    RichTableRow,
    RichTableVerticalAlignment,
    RichTaskState,
    RichText,
)
from telegram_mcp.tdexport.model_rich_parse import (
    RichVisitor,
    parse_inline_button_action,
    parse_rich_block,
    parse_rich_text,
    refresh_rich_message_file_reference,
    visit_rich_message,
)

A = InlineButtonAction.Type
T = RichText.Type
K = RichBlock.Kind
WHEN = datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc)
STAMP = int(WHEN.timestamp())


def plain(text="x"):
    return tl.TextPlain(text=text)


def caption(text="c"):
    return tl.PageCaption(text=plain(text), credit=tl.TextEmpty())


def parsed_plain(text="x"):
    return RichText(type=T.Plain, text=text)


PARSED_CAPTION = RichCaption(text=parsed_plain("c"), credit=RichText(type=T.Empty))


# Inline button actions and styles.


@pytest.mark.parametrize(
    ("data", "expected"),
    [
        (tl.InlineButtonTypeUrl(url="https://u"), InlineButtonAction(url="https://u")),
        (
            tl.InlineButtonTypeUrlAuth(url="https://a", button_id=4, fwd_text="f"),
            InlineButtonAction(url="https://a", button_id=4, forward_text="f", type=A.Auth),
        ),
        (
            tl.InlineButtonTypeUrlAuth(url="https://a", button_id=4),
            InlineButtonAction(url="https://a", button_id=4, type=A.Auth),
        ),
        (
            tl.InlineButtonTypeWebView(url="https://w"),
            InlineButtonAction(url="https://w", type=A.WebView),
        ),
        (
            tl.InlineButtonTypeCallback(data=b"\x01"),
            InlineButtonAction(callback_data=b"\x01", type=A.Callback),
        ),
        (
            tl.InlineButtonTypeCallback(data=b"\x02", requires_password=True),
            InlineButtonAction(
                callback_data=b"\x02", type=A.CallbackWithPassword, requires_password=True
            ),
        ),
        (tl.InlineButtonTypeGame(), InlineButtonAction(type=A.Game)),
        (tl.InlineButtonTypeBuy(), InlineButtonAction(type=A.Buy)),
        (
            tl.InlineButtonTypeSwitchInline(query="q"),
            InlineButtonAction(query="q", type=A.SwitchInline),
        ),
        (
            tl.InlineButtonTypeSwitchInline(
                query="q",
                same_peer=True,
                peer_types=[tl.InlineQueryPeerTypeSameBotPM(), tl.InlineQueryPeerTypeBotPM()],
            ),
            InlineButtonAction(
                query="q",
                type=A.SwitchInlineSame,
                same_peer=True,
                peer_types=[InlineButtonPeerType.SameBotPM, InlineButtonPeerType.BotPM],
            ),
        ),
        (
            tl.InlineButtonTypeUserProfile(user_id=42),
            InlineButtonAction(user_id=42, type=A.UserProfile),
        ),
        (
            tl.InlineButtonTypeCopy(copy_text="t"),
            InlineButtonAction(copy_text="t", type=A.CopyText),
        ),
        (tl.InlineButtonTypeDisabled(), InlineButtonAction(type=A.Disabled)),
    ],
)
def test_inline_button_actions(data, expected):
    assert parse_inline_button_action(data) == expected


def test_unknown_button_type_drops_the_button_and_keeps_its_text():
    assert parse_inline_button_action(tl.TextEmpty()) is None
    button = tl.TextButton(text=plain("k"), type=tl.TextEmpty())
    assert parse_rich_text(button) == parsed_plain("k")
    row = tl.PageBlockButtonRow(
        buttons=[tl.PageButton(text=plain(), type=tl.TextEmpty())], align_right=True
    )
    parsed = parse_rich_block(row)
    assert (parsed.kind, parsed.buttons, parsed.button_alignment) == (
        K.ButtonRow,
        [],
        RichButtonAlignment.Right,
    )


def _style(text_type, style, inline=True):
    if inline:
        return parse_rich_text(tl.TextButton(text=plain(), type=text_type, style=style)).button
    row = tl.PageBlockButtonRow(buttons=[tl.PageButton(text=plain(), type=text_type, style=style)])
    return parse_rich_block(row).buttons[0].button


CALLBACK = tl.InlineButtonTypeCallback(data=b"")
URL = tl.InlineButtonTypeUrl(url="https://u")


@pytest.mark.parametrize(
    ("text_type", "style", "inline", "expected"),
    [
        (CALLBACK, None, True, None),
        (CALLBACK, tl.RichButtonStyle(link=True, bg_danger=True), True, RichButtonStyle.Link),
        (
            tl.InlineButtonTypeDisabled(),
            tl.RichButtonStyle(link=True),
            True,
            RichButtonStyle.Link,
        ),
        # A link style is only for callback/disabled inline buttons.
        (URL, tl.RichButtonStyle(link=True), True, RichButtonStyle.Default),
        (CALLBACK, tl.RichButtonStyle(link=True), False, RichButtonStyle.Default),
        (URL, tl.RichButtonStyle(bg_primary=True, bg_success=True), True, RichButtonStyle.Primary),
        (URL, tl.RichButtonStyle(bg_success=True), False, RichButtonStyle.Success),
    ],
)
def test_button_styles(text_type, style, inline, expected):
    assert _style(text_type, style, inline).style == expected


# Rich text.


@pytest.mark.parametrize(
    ("wrapper", "kind"),
    [
        (tl.TextBold, T.Bold),
        (tl.TextItalic, T.Italic),
        (tl.TextUnderline, T.Underline),
        (tl.TextStrike, T.Strike),
        (tl.TextFixed, T.Fixed),
        (tl.TextSubscript, T.Subscript),
        (tl.TextSuperscript, T.Superscript),
        (tl.TextMarked, T.Marked),
        (tl.TextSpoiler, T.Spoiler),
        (tl.TextMention, T.Mention),
        (tl.TextHashtag, T.Hashtag),
        (tl.TextBotCommand, T.BotCommand),
        (tl.TextCashtag, T.Cashtag),
        (tl.TextAutoUrl, T.AutoUrl),
        (tl.TextAutoEmail, T.AutoEmail),
        (tl.TextAutoPhone, T.AutoPhone),
        (tl.TextBankCard, T.BankCard),
    ],
)
def test_wrapper_texts(wrapper, kind):
    assert parse_rich_text(wrapper(text=plain())) == RichText(type=kind, children=[parsed_plain()])


def test_data_and_leaf_texts():
    child = [parsed_plain()]
    assert parse_rich_text(tl.TextUrl(text=plain(), url="https://u", webpage_id=9)) == RichText(
        type=T.Url, children=child, data="https://u", id=9
    )
    assert parse_rich_text(tl.TextEmail(text=plain(), email="a@b")) == RichText(
        type=T.Email, children=child, data="a@b"
    )
    assert parse_rich_text(tl.TextPhone(text=plain(), phone="+1")) == RichText(
        type=T.Phone, children=child, data="+1"
    )
    assert parse_rich_text(tl.TextAnchor(text=plain(), name="n")) == RichText(
        type=T.Anchor, children=child, data="n"
    )
    assert parse_rich_text(tl.TextConcat(texts=[plain("a"), tl.TextEmpty()])) == RichText(
        type=T.Concat, children=[parsed_plain("a"), RichText(type=T.Empty)]
    )
    assert parse_rich_text(tl.TextImage(document_id=5, w=16, h=8)) == RichText(
        type=T.InlineImage, id=5, width=16, height=8
    )
    assert parse_rich_text(tl.TextMath(source="x^2")) == RichText(type=T.Math, data="x^2")
    assert parse_rich_text(tl.TextMentionName(text=plain(), user_id=7)) == RichText(
        type=T.MentionName, children=child, id=7
    )
    assert parse_rich_text(tl.TextEmpty()) == RichText(type=T.Empty)


def test_formatted_date_and_diff():
    date = tl.TextDate(text=plain(), date=WHEN, short_time=True, long_date=True)
    assert parse_rich_text(date) == RichText(
        type=T.FormattedDate,
        children=[parsed_plain()],
        date=STAMP,
        short_time=True,
        long_date=True,
    )
    diff = tl.TextDiff(text=plain("new"), old_text=plain("old"))
    assert parse_rich_text(diff) == RichText(
        type=T.Diff,
        unsupported=True,
        children=[parsed_plain("new")],
        old_children=[parsed_plain("old")],
    )


# Blocks.


@pytest.mark.parametrize(
    ("block", "level"),
    [
        (tl.PageBlockTitle, 1),
        (tl.PageBlockSubtitle, 2),
        (tl.PageBlockHeader, 3),
        (tl.PageBlockSubheader, 4),
        (tl.PageBlockKicker, 5),
        (tl.PageBlockHeading1, 1),
        (tl.PageBlockHeading3, 3),
        (tl.PageBlockHeading4, 4),
        (tl.PageBlockHeading5, 5),
        (tl.PageBlockHeading6, 6),
    ],
)
def test_headings(block, level):
    assert parse_rich_block(block(text=plain())) == RichBlock(
        kind=K.Heading, text=parsed_plain(), heading_level=level
    )


def test_text_and_simple_blocks():
    assert parse_rich_block(tl.PageBlockFooter(text=plain())) == RichBlock(
        kind=K.Footer, text=parsed_plain()
    )
    assert parse_rich_block(tl.PageBlockThinking(text=plain())) == RichBlock(
        kind=K.Thinking, text=parsed_plain()
    )
    assert parse_rich_block(tl.PageBlockUnsupported()) == RichBlock(
        kind=K.Unsupported, unsupported=True
    )
    assert parse_rich_block(tl.PageBlockAuthorDate(author=plain(), published_date=WHEN)) == (
        RichBlock(kind=K.AuthorDate, text=parsed_plain(), date=STAMP)
    )
    assert parse_rich_block(tl.PageBlockPreformatted(text=plain(), language="py")) == RichBlock(
        kind=K.Code, text=parsed_plain(), language="py"
    )
    assert parse_rich_block(tl.PageBlockAnchor(name="a")) == RichBlock(kind=K.Anchor, name="a")
    assert parse_rich_block(tl.PageBlockMath(source="e")) == RichBlock(kind=K.Math, formula="e")
    cover = tl.PageBlockCover(cover=tl.PageBlockDivider())
    assert parse_rich_block(cover) == RichBlock(kind=K.Cover, blocks=[RichBlock(kind=K.Divider)])
    # Anything else (here a text, not a block) is an Unknown block.
    assert parse_rich_block(tl.TextEmpty()) == RichBlock()


def test_media_blocks():
    photo = tl.PageBlockPhoto(
        photo_id=1, caption=caption(), spoiler=True, url="https://u", webpage_id=3
    )
    assert parse_rich_block(photo) == RichBlock(
        kind=K.Photo,
        photo_id=1,
        caption=PARSED_CAPTION,
        spoiler=True,
        optional_url="https://u",
        optional_webpage_id=3,
    )
    video = tl.PageBlockVideo(video_id=2, caption=caption(), autoplay=True, loop=True)
    assert parse_rich_block(video) == RichBlock(
        kind=K.Video, document_id=2, caption=PARSED_CAPTION, autoplay=True, loop=True
    )
    assert parse_rich_block(tl.PageBlockAudio(audio_id=4, caption=caption())) == RichBlock(
        kind=K.Audio, document_id=4, caption=PARSED_CAPTION
    )
    assert parse_rich_block(tl.PageBlockDocument(document_id=5, caption=caption())) == (
        RichBlock(kind=K.File, document_id=5, caption=PARSED_CAPTION)
    )
    embed = tl.PageBlockEmbed(
        caption=caption(), full_width=True, url="https://e", html="<b>", poster_photo_id=6, w=7
    )
    assert parse_rich_block(embed) == RichBlock(
        kind=K.Embed,
        caption=PARSED_CAPTION,
        full_width=True,
        optional_url="https://e",
        html="<b>",
        poster_photo_id=6,
        width=7,
    )
    post = tl.PageBlockEmbedPost(
        url="https://p",
        webpage_id=8,
        author_photo_id=9,
        author="A",
        date=WHEN,
        blocks=[tl.PageBlockDivider()],
        caption=caption(),
    )
    assert parse_rich_block(post) == RichBlock(
        kind=K.EmbedPost,
        url="https://p",
        webpage_id=8,
        author_photo_id=9,
        author="A",
        date=STAMP,
        blocks=[RichBlock(kind=K.Divider)],
        caption=PARSED_CAPTION,
    )
    for tl_kind, kind in ((tl.PageBlockCollage, K.Collage), (tl.PageBlockSlideshow, K.Slideshow)):
        group = tl_kind(items=[tl.PageBlockDivider()], caption=caption())
        assert parse_rich_block(group) == RichBlock(
            kind=kind, blocks=[RichBlock(kind=K.Divider)], caption=PARSED_CAPTION
        )


def test_maps():
    geo = tl.GeoPoint(long=2.5, lat=1.5, access_hash=-4, accuracy_radius=10)
    parsed = parse_rich_block(tl.PageBlockMap(geo=geo, zoom=15, w=600, h=300, caption=caption()))
    assert parsed == RichBlock(
        kind=K.Map,
        map_point=RichMapPoint(
            access_hash=-4,
            accuracy_radius=10,
            latitude=1.5,
            longitude=2.5,
            source=RichMapPoint.Source.GeoPoint,
        ),
        zoom=15,
        map_width=600,
        map_height=300,
        caption=PARSED_CAPTION,
    )
    empty = tl.PageBlockMap(geo=tl.GeoPointEmpty(), zoom=1, w=2, h=3, caption=caption())
    assert parse_rich_block(empty).map_point == RichMapPoint()
    point = tl.InputGeoPoint(lat=-1.0, long=-2.0)
    typed = tl.InputPageBlockMap(geo=point, zoom=1, w=2, h=3, caption=caption())
    assert parse_rich_block(typed).kind == K.InputMap
    assert parse_rich_block(typed).map_point == RichMapPoint(
        latitude=-1.0, longitude=-2.0, source=RichMapPoint.Source.InputGeoPoint
    )
    blank = tl.InputPageBlockMap(geo=tl.InputGeoPointEmpty(), zoom=1, w=2, h=3, caption=caption())
    assert parse_rich_block(blank).map_point.source == RichMapPoint.Source.InputGeoPointEmpty


def test_lists():
    bullet = tl.PageBlockList(
        items=[
            tl.PageListItemText(text=plain(), checkbox=True),
            tl.PageListItemBlocks(blocks=[tl.PageBlockDivider()]),
        ]
    )
    assert parse_rich_block(bullet) == RichBlock(
        kind=K.List,
        list_kind=RichListKind.Bullet,
        list_items=[
            RichListItem(text=parsed_plain(), task_state=RichTaskState.Unchecked),
            RichListItem(blocks=[RichBlock(kind=K.Divider)], content=RichListItemContent.Blocks),
        ],
    )
    ordered = tl.PageBlockOrderedList(
        items=[
            tl.PageListOrderedItemBlocks(blocks=[], num="b", value=2, type="a"),
        ],
        reversed=True,
        type="I",
    )
    parsed = parse_rich_block(ordered)
    assert (parsed.list_kind, parsed.ordered_list.reversed, parsed.ordered_list.type) == (
        RichListKind.Ordered,
        True,
        "I",
    )
    assert parsed.ordered_list.start is None
    assert parsed.list_items == [
        RichListItem(num="b", type="a", value=2, content=RichListItemContent.Blocks)
    ]


def test_quotes_tables_details_and_related_articles():
    assert parse_rich_block(tl.PageBlockBlockquote(text=plain("q"), caption=plain("c"))) == (
        RichBlock(kind=K.Quote, text=parsed_plain("q"), quote_caption=parsed_plain("c"))
    )
    pull = parse_rich_block(tl.PageBlockPullquote(text=plain(), caption=tl.TextEmpty()))
    assert pull.pullquote and pull.quote_content == RichQuoteContent.Text
    blocks = tl.PageBlockBlockquoteBlocks(blocks=[tl.PageBlockDivider()], caption=plain())
    assert parse_rich_block(blocks) == RichBlock(
        kind=K.Quote,
        quote_content=RichQuoteContent.Blocks,
        blocks=[RichBlock(kind=K.Divider)],
        quote_caption=parsed_plain(),
    )
    cells = [
        tl.PageTableCell(align_center=True, valign_middle=True, rowspan=3),
        tl.PageTableCell(align_right=True, align_center=True, valign_bottom=True),
    ]
    table = tl.PageBlockTable(
        title=plain(), rows=[tl.PageTableRow(cells=cells)], striped=True, compact=True
    )
    assert parse_rich_block(table) == RichBlock(
        kind=K.Table,
        text=parsed_plain(),
        striped=True,
        compact=True,
        table_rows=[
            RichTableRow(
                [
                    RichTableCell(
                        alignment=RichTableAlignment.Center,
                        vertical_alignment=RichTableVerticalAlignment.Middle,
                        rowspan=3,
                    ),
                    RichTableCell(
                        alignment=RichTableAlignment.Right,
                        vertical_alignment=RichTableVerticalAlignment.Bottom,
                    ),
                ]
            )
        ],
    )
    details = tl.PageBlockDetails(blocks=[], title=plain())
    assert parse_rich_block(details) == RichBlock(kind=K.Details, text=parsed_plain())
    articles = tl.PageBlockRelatedArticles(
        title=plain(),
        articles=[
            tl.PageRelatedArticle(
                url="https://r",
                webpage_id=1,
                title="t",
                description="d",
                photo_id=2,
                author="a",
                published_date=WHEN,
            ),
            tl.PageRelatedArticle(url="", webpage_id=3),
        ],
    )
    assert parse_rich_block(articles) == RichBlock(
        kind=K.RelatedArticles,
        text=parsed_plain(),
        related_articles=[
            RichRelatedArticle("https://r", "t", "d", "a", 2, STAMP, 1),
            RichRelatedArticle(webpage_id=3),
        ],
    )


def test_button_row_alignments():
    button = tl.PageButton(text=plain(), type=tl.InlineButtonTypeGame())
    for flags, alignment in (
        ({"align_left": True}, RichButtonAlignment.Left),
        ({"align_center": True}, RichButtonAlignment.Center),
        ({}, RichButtonAlignment.Stretch),
    ):
        row = parse_rich_block(tl.PageBlockButtonRow(buttons=[button], **flags))
        assert row.button_alignment == alignment and len(row.buttons) == 1


@pytest.mark.parametrize(
    ("chat", "expected"),
    [
        (tl.ChatEmpty(id=1), RichChannel(id=1)),
        (
            tl.Chat(
                id=2,
                title="G",
                photo=tl.ChatPhotoEmpty(),
                participants_count=0,
                date=WHEN,
                version=1,
            ),
            RichChannel(title="G", id=2, source=RichChannel.Source.Chat),
        ),
        (
            tl.ChatForbidden(id=3, title="F"),
            RichChannel(title="F", id=3, source=RichChannel.Source.ChatForbidden),
        ),
        (
            tl.Channel(
                id=4,
                title="C",
                photo=tl.ChatPhotoEmpty(),
                date=WHEN,
                broadcast=True,
                access_hash=5,
                username="chan",
            ),
            RichChannel(
                title="C",
                username="chan",
                access_hash=5,
                id=4,
                source=RichChannel.Source.Channel,
                broadcast=True,
            ),
        ),
        (
            tl.ChannelForbidden(id=6, access_hash=7, title="X", megagroup=True, monoforum=True),
            RichChannel(
                title="X",
                access_hash=7,
                id=6,
                source=RichChannel.Source.ChannelForbidden,
                megagroup=True,
                monoforum=True,
            ),
        ),
        (
            tl.Community(id=8, title="M", photo=tl.ChatPhotoEmpty(), date=WHEN, access_hash=9),
            RichChannel(title="M", access_hash=9, id=8, source=RichChannel.Source.Community),
        ),
        (
            tl.CommunityForbidden(id=10, title="N"),
            RichChannel(title="N", id=10, source=RichChannel.Source.CommunityForbidden),
        ),
    ],
)
def test_channel_blocks(chat, expected):
    assert parse_rich_block(tl.PageBlockChannel(channel=chat)) == RichBlock(
        kind=K.Channel, channel=expected
    )


# The visitor and file-reference refresh.


def _location(kind, file_id, dc_id=2, thumb=""):
    if kind == "photo":
        data = tl.InputPhotoFileLocation(
            id=file_id, access_hash=1, file_reference=b"new", thumb_size=thumb
        )
    else:
        data = tl.InputDocumentFileLocation(
            id=file_id, access_hash=1, file_reference=b"new", thumb_size=thumb
        )
    return FileLocation(dc_id, data)


def test_visitor_walks_every_block_kind_in_tdesktop_order():
    image = RichText(type=T.InlineImage, id=20)
    diff = RichText(type=T.Diff, children=[image], old_children=[RichText(type=T.Plain)])
    blocks = [
        RichBlock(kind=K.Paragraph, text=diff),
        RichBlock(
            kind=K.List,
            list_items=[
                RichListItem(text=RichText(type=T.Bold)),
                RichListItem(),
                RichListItem(
                    content=RichListItemContent.Blocks,
                    blocks=[RichBlock(kind=K.Photo, photo_id=1)],
                ),
            ],
        ),
        RichBlock(
            kind=K.Quote,
            quote_content=RichQuoteContent.Blocks,
            blocks=[RichBlock(kind=K.Audio, document_id=21)],
        ),
        RichBlock(kind=K.Cover, blocks=[RichBlock(kind=K.Embed, poster_photo_id=2)]),
        RichBlock(kind=K.Embed),
        RichBlock(kind=K.EmbedPost, author_photo_id=3, blocks=[RichBlock(kind=K.Video)]),
        RichBlock(kind=K.Slideshow, blocks=[RichBlock(kind=K.File, document_id=22)]),
        RichBlock(
            kind=K.Table,
            table_rows=[RichTableRow([RichTableCell(), RichTableCell(text=RichText(type=T.Url))])],
        ),
        RichBlock(kind=K.Details, blocks=[RichBlock(kind=K.Divider)]),
        RichBlock(
            kind=K.RelatedArticles,
            related_articles=[RichRelatedArticle(photo_id=4), RichRelatedArticle()],
        ),
        RichBlock(kind=K.InputMap),
        RichBlock(kind=K.ButtonRow, buttons=[RichText(type=T.Button)]),
        RichBlock(kind=K.Channel),
    ]
    photos = {n: Photo(id=n) for n in (1, 2, 3, 4)}
    documents = {n: Document(id=n) for n in (20, 21, 22)}
    seen = []
    visit_rich_message(
        RichMessage(blocks=blocks, photos=photos, documents=documents),
        RichVisitor(
            text=lambda text: seen.append(text.type.name),
            photo=lambda photo: seen.append(f"photo{photo.id}"),
            document=lambda document: seen.append(f"document{document.id}"),
        ),
    )
    caption = ["Empty", "Empty"]
    assert seen == [
        *("Diff", "InlineImage", "document20", "Plain"),
        "Bold",
        *("photo1", *caption),
        *("document21", *caption, "Empty"),
        *("photo2", *caption),
        *caption,
        *("photo3", *caption, *caption),
        *("document22", *caption, *caption),
        *("Empty", "Url"),
        *("Empty",),
        *("Empty", "photo4"),
        *caption,
        "Button",
    ]
    # The default visitor ignores everything.
    visit_rich_message(RichMessage(blocks=blocks, photos=photos), RichVisitor())


def test_refresh_file_reference_from_photos_documents_and_thumbnails():
    photo = Photo(id=1, image=Image(file=File(location=_location("photo", 1))))
    thumb = Image(width=90, file=File(location=_location("document", 7, thumb="m")))
    hidden_thumb = Image(width=0, file=File(location=_location("document", 8, thumb="m")))
    documents = {
        7: Document(id=7, file=File(location=_location("document", 7)), thumb=thumb),
        8: Document(id=8, thumb=hidden_thumb),
    }
    message = RichMessage(photos={1: photo}, documents=documents)
    stale = _location("photo", 1)
    stale.data.file_reference = b"old"
    refreshed = refresh_rich_message_file_reference(stale, message)
    assert refreshed is not None and refreshed.data.file_reference == b"new"
    assert stale.data.file_reference == b"old"
    for file_id, thumb_size in ((7, ""), (7, "m")):
        wanted = _location("document", file_id, thumb=thumb_size)
        assert refresh_rich_message_file_reference(wanted, message).data is (
            documents[7].file.location.data if not thumb_size else thumb.file.location.data
        )
    # A thumbnail of a document whose thumb has no width is never consulted.
    assert (
        refresh_rich_message_file_reference(_location("document", 8, thumb="m"), message) is None
    )
    assert refresh_rich_message_file_reference(_location("photo", 1, dc_id=5), message) is None
