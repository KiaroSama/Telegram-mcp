"""The JSON export writer: rich messages (rich text, page blocks, media metadata, buttons).

Every expectation is hand-derived from Telegram Desktop v7.2.10's export_output_json.cpp
(SerializeRichText, SerializeRichBlock, AppendRichFileAvailability, AppendRichPhotoMetadata,
AppendRichDocumentMetadata, SerializeRichButtonAction and friends): key order, which values
are quoted (uint64/int64 ids are strings), and the nesting-driven indentation. Synthetic data.
"""

from datetime import datetime, timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.json_rich import (
    RichSerializeContext,
    serialize_rich_block,
    serialize_rich_text,
)
from telegram_mcp.tdexport.json_serialize import JsonContext
from telegram_mcp.tdexport.model import Document, File, Image, Photo
from telegram_mcp.tdexport.model_rich import (
    InlineButtonAction,
    InlineButtonPeerType,
    RichBlock,
    RichButtonAlignment,
    RichButtonPayload,
    RichButtonStyle,
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

T = RichText.Type
K = RichBlock.Kind
A = InlineButtonAction.Type
D = int(datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc).timestamp())
ISO = '"2024-01-12T10:00:05"'
RAW = '"1705053605"'
EMPTY_CAPTION = (
    ' "caption": {',
    '  "text": {',
    '   "type": "empty"',
    "  },",
    '  "credit": {',
    '   "type": "empty"',
    "  }",
    " }",
)


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def lines(*parts: str) -> bytes:
    return "\n".join(parts).encode("utf-8")


def plain(text):
    return RichText(type=T.Plain, text=text)


def wrapped(kind, *children, **fields):
    return RichText(type=kind, children=list(children), **fields)


def context(**message):
    return RichSerializeContext(JsonContext(), RichMessage(**message))


def text_json(text, **message):
    return serialize_rich_text(context(**message), text)


def block_json(block, **message):
    return serialize_rich_block(context(**message), block)


def block(kind, **fields):
    return RichBlock(kind=kind, **fields)


PLAIN_X = ('  "type": "plain",', '  "text": "x"')


# Rich text.


@pytest.mark.parametrize(
    ("kind", "name", "key"),
    [(T.Email, "email", "email"), (T.Phone, "phone", "phone"), (T.Anchor, "anchor", "name")],
)
def test_data_texts_put_their_target_before_the_child(kind, name, key):
    assert text_json(wrapped(kind, plain("x"), data="d")) == lines(
        "{", f' "type": "{name}",', f' "{key}": "d",', ' "text": {', *PLAIN_X, " }", "}"
    )


@pytest.mark.parametrize(
    ("kind", "name"),
    [
        (T.Italic, "italic"),
        (T.Underline, "underline"),
        (T.Strike, "strikethrough"),
        (T.Fixed, "code"),
        (T.Subscript, "subscript"),
        (T.Superscript, "superscript"),
        (T.Marked, "marked"),
        (T.Spoiler, "spoiler"),
        (T.Mention, "mention"),
        (T.Hashtag, "hashtag"),
        (T.BotCommand, "bot_command"),
        (T.Cashtag, "cashtag"),
        (T.AutoUrl, "link"),
        (T.AutoEmail, "email"),
        (T.AutoPhone, "phone"),
        (T.BankCard, "bank_card"),
    ],
)
def test_wrapper_texts_have_one_child(kind, name):
    assert text_json(wrapped(kind, plain("x"))) == lines(
        "{", f' "type": "{name}",', ' "text": {', *PLAIN_X, " }", "}"
    )


def test_leaf_texts():
    assert text_json(RichText()) == lines("{", ' "type": "empty"', "}")
    assert text_json(RichText(type=T.Math, data="x^2")) == lines(
        "{", ' "type": "math",', ' "source": "x^2"', "}"
    )
    emoji = RichText(type=T.CustomEmoji, text="E", custom_emoji_data="123")
    assert text_json(emoji) == lines(
        "{", ' "type": "custom_emoji",', ' "text": "E",', ' "document_id": "123"', "}"
    )
    assert text_json(wrapped(T.Url, plain("x"), data="https://a")) == lines(
        "{", ' "type": "text_link",', ' "href": "https://a",', ' "text": {', *PLAIN_X, " }", "}"
    )


def test_mention_name_and_formatted_date():
    assert text_json(wrapped(T.MentionName, plain("x"), id=-1)) == lines(
        "{",
        ' "type": "mention_name",',
        ' "user_id": "18446744073709551615",',
        ' "text": {',
        *PLAIN_X,
        " }",
        "}",
    )
    date = wrapped(T.FormattedDate, plain("x"), date=D, relative=True, day_of_week=True)
    assert text_json(date) == lines(
        "{",
        ' "type": "formatted_date",',
        ' "text": {',
        *PLAIN_X,
        " },",
        f' "date": {ISO},',
        f' "date_unixtime": {RAW},',
        ' "relative": true,',
        ' "short_time": false,',
        ' "long_time": false,',
        ' "short_date": false,',
        ' "long_date": false,',
        ' "day_of_week": true',
        "}",
    )


def test_inline_image_uses_its_own_size_and_the_document_without_dimensions():
    icon = Document(
        name="i.webp",
        mime="image/webp",
        width=512,
        height=512,
        file=File(relative_path="stickers/i.webp", size=10),
    )
    image = RichText(type=T.InlineImage, id=5, width=16, height=16)
    assert text_json(image, documents={5: icon}) == lines(
        "{",
        ' "type": "inline_image",',
        ' "document_id": "5",',
        ' "file": "stickers/i.webp",',
        ' "file_name": "i.webp",',
        ' "file_size": 10,',
        ' "mime_type": "image/webp",',
        ' "width": 16,',
        ' "height": 16',
        "}",
    )
    assert text_json(RichText(type=T.InlineImage, id=6)) == lines(
        "{",
        ' "type": "inline_image",',
        ' "document_id": "6",',
        ' "file_skip_reason": "unavailable",',
        ' "width": 0,',
        ' "height": 0',
        "}",
    )


def test_diff_is_marked_unsupported():
    diff = RichText(type=T.Diff, children=[plain("x")], old_children=[plain("x")])
    assert text_json(diff) == lines(
        "{",
        ' "type": "diff",',
        ' "unsupported": true,',
        ' "text": {',
        *PLAIN_X,
        " },",
        ' "old_text": {',
        *PLAIN_X,
        " }",
        "}",
    )


# Button actions: each "button" object as it sits inside a button row.


def _row(action, style=None):
    button = RichText(
        type=T.Button, children=[plain("B")], button=RichButtonPayload(action, style)
    )
    row = block(K.ButtonRow, buttons=[button], button_alignment=RichButtonAlignment.Right)
    return block_json(row)


@pytest.mark.parametrize(
    ("action", "fields"),
    [
        (InlineButtonAction(type=A.Url, url="https://x"), ['"url": "https://x"']),
        (InlineButtonAction(type=A.WebView, url="https://w"), ['"url": "https://w"']),
        (
            InlineButtonAction(type=A.Auth, url="u", forward_text="f", button_id=3),
            ['"url": "u"', '"forward_text": "f"', '"button_id": 3'],
        ),
        (InlineButtonAction(type=A.Auth, url="u"), ['"url": "u"', '"button_id": 0']),
        (
            InlineButtonAction(type=A.SwitchInline, query="q", same_peer=True),
            ['"query": "q"', '"same_peer": true'],
        ),
        (InlineButtonAction(type=A.UserProfile, user_id=42), ['"user_id": "42"']),
        (InlineButtonAction(type=A.CopyText, copy_text="c\n"), ['"copy_text": "c\\n"']),
    ],
)
def test_button_action_fields(action, fields):
    type_name = InlineButtonAction.type_to_string(action)
    body = ",\n    ".join([f'"type": "{type_name}"', *fields])
    expected = f'   "button": {{\n    {body}\n   }}\n  }}'
    assert expected.encode() in _row(action)


def test_button_row_with_style_and_peer_types():
    action = InlineButtonAction(
        type=A.SwitchInlineSame,
        query="q",
        peer_types=[InlineButtonPeerType.Chat, InlineButtonPeerType.Megagroup],
    )
    assert _row(action, RichButtonStyle.Danger) == lines(
        "{",
        ' "type": "button_row",',
        ' "alignment": "right",',
        ' "buttons": [',
        "  {",
        '   "text": {',
        '    "type": "plain",',
        '    "text": "B"',
        "   },",
        '   "button": {',
        '    "type": "switch_inline_same",',
        '    "query": "q",',
        '    "same_peer": false,',
        '    "peer_types": [',
        '     "chat",',
        '     "megagroup"',
        "    ]",
        "   },",
        '   "style": "danger"',
        "  }",
        " ]",
        "}",
    )
    game = _row(InlineButtonAction(type=A.Game))
    assert b'   "button": {\n    "type": "game"\n   }\n  }' in game
    assert block_json(block(K.ButtonRow)) == lines(
        "{", ' "type": "button_row",', ' "alignment": "stretch",', ' "buttons": []', "}"
    )


# Blocks.


def test_simple_blocks():
    assert block_json(block(K.Unsupported)) == lines(
        "{",
        ' "type": "unsupported",',
        ' "source_type": "page_block_unsupported",',
        ' "unsupported": true',
        "}",
    )
    assert block_json(block(K.Unknown)) == lines(
        "{", ' "type": "unsupported",', ' "source_type": "unknown",', ' "unsupported": true', "}"
    )
    assert block_json(block(K.Divider)) == lines("{", ' "type": "divider"', "}")
    assert block_json(block(K.Anchor, name="n")) == lines(
        "{", ' "type": "anchor",', ' "name": "n"', "}"
    )
    assert block_json(block(K.Math, formula="a<b")) == lines(
        "{", ' "type": "math",', ' "formula": "a<b"', "}"
    )
    assert block_json(block(K.Heading, heading_level=2, text=plain("x"))) == lines(
        "{", ' "type": "heading",', ' "level": 2,', ' "text": {', *PLAIN_X, " }", "}"
    )
    for kind, name in ((K.Footer, "footer"), (K.Thinking, "thinking")):
        assert block_json(block(kind, text=plain("x"))) == lines(
            "{", f' "type": "{name}",', ' "text": {', *PLAIN_X, " }", "}"
        )
    assert block_json(block(K.Code, text=plain("x"))) == lines(
        "{", ' "type": "code",', ' "text": {', *PLAIN_X, " },", ' "language": ""', "}"
    )
    assert block_json(block(K.AuthorDate, text=plain("x"), date=D)) == lines(
        "{",
        ' "type": "author_date",',
        ' "author": {',
        *PLAIN_X,
        " },",
        f' "date": {ISO},',
        f' "date_unixtime": {RAW}',
        "}",
    )


def test_ordered_list_items():
    items = [
        RichListItem(
            text=plain("x"), num="1.", value=3, type="t", task_state=RichTaskState.Checked
        ),
        RichListItem(content=RichListItemContent.Blocks, blocks=[block(K.Divider)]),
    ]
    listing = block(
        K.List,
        list_kind=RichListKind.Ordered,
        ordered_list=RichOrderedList(type="a", start=2, reversed=True),
        list_items=items,
    )
    assert block_json(listing) == lines(
        "{",
        ' "type": "list",',
        ' "kind": "ordered",',
        ' "reversed": true,',
        ' "start": 2,',
        ' "list_type": "a",',
        ' "items": [',
        "  {",
        '   "task_state": "checked",',
        '   "content": "text",',
        '   "text": {',
        '    "type": "plain",',
        '    "text": "x"',
        "   },",
        '   "num": "1.",',
        '   "value": 3,',
        '   "item_type": "t"',
        "  },",
        "  {",
        '   "task_state": "none",',
        '   "content": "blocks",',
        '   "blocks": [',
        "    {",
        '     "type": "divider"',
        "    }",
        "   ]",
        "  }",
        " ]",
        "}",
    )


def test_bullet_list_item_without_text_serializes_an_empty_text():
    listing = block(K.List, list_items=[RichListItem(task_state=RichTaskState.Unchecked)])
    assert block_json(listing) == lines(
        "{",
        ' "type": "list",',
        ' "kind": "bullet",',
        ' "items": [',
        "  {",
        '   "task_state": "unchecked",',
        '   "content": "text",',
        '   "text": {',
        '    "type": "empty"',
        "   }",
        "  }",
        " ]",
        "}",
    )


def test_quotes():
    quote = block(K.Quote, text=plain("x"), quote_caption=plain("x"))
    assert block_json(quote) == lines(
        "{",
        ' "type": "quote",',
        ' "content": "text",',
        ' "pullquote": false,',
        ' "text": {',
        *PLAIN_X,
        " },",
        ' "caption": {',
        *PLAIN_X,
        " }",
        "}",
    )
    pull = block(K.Quote, quote_content=RichQuoteContent.Blocks, pullquote=True)
    assert block_json(pull) == lines(
        "{",
        ' "type": "quote",',
        ' "content": "blocks",',
        ' "pullquote": true,',
        ' "blocks": [],',
        ' "caption": {',
        '  "type": "empty"',
        " }",
        "}",
    )


def test_table_rows_and_cells():
    cells = [
        RichTableCell(text=plain("c"), colspan=2, header=True),
        RichTableCell(
            rowspan=0,
            alignment=RichTableAlignment.Right,
            vertical_alignment=RichTableVerticalAlignment.Bottom,
        ),
    ]
    table = block(K.Table, text=plain("x"), bordered=True, table_rows=[RichTableRow(cells)])
    assert block_json(table) == lines(
        "{",
        ' "type": "table",',
        ' "title": {',
        *PLAIN_X,
        " },",
        ' "bordered": true,',
        ' "striped": false,',
        ' "compact": false,',
        ' "rows": [',
        "  {",
        '   "cells": [',
        "    {",
        '     "text": {',
        '      "type": "plain",',
        '      "text": "c"',
        "     },",
        '     "colspan": 2,',
        '     "header": true,',
        '     "align": "left",',
        '     "vertical_align": "top"',
        "    },",
        "    {",
        '     "rowspan": 0,',
        '     "header": false,',
        '     "align": "right",',
        '     "vertical_align": "bottom"',
        "    }",
        "   ]",
        "  }",
        " ]",
        "}",
    )


def test_details_cover_and_collage():
    details = block(K.Details, text=plain("x"), open=True)
    assert block_json(details) == lines(
        "{",
        ' "type": "details",',
        ' "title": {',
        *PLAIN_X,
        " },",
        ' "open": true,',
        ' "blocks": []',
        "}",
    )
    cover = block(K.Cover, blocks=[block(K.Anchor, name="n")])
    assert block_json(cover) == lines(
        "{", ' "type": "cover",', ' "block": {', '  "type": "anchor",', '  "name": "n"', " }", "}"
    )
    with pytest.raises(AssertionError):
        block_json(block(K.Cover))
    for kind, name in ((K.Collage, "collage"), (K.Slideshow, "slideshow")):
        assert block_json(block(kind)) == lines(
            "{", f' "type": "{name}",', ' "items": [],', *EMPTY_CAPTION, "}"
        )


def test_maps_write_coordinates_only_for_real_points():
    point = RichMapPoint(
        access_hash=-2,
        accuracy_radius=5,
        latitude=1.5,
        longitude=-2.25,
        source=RichMapPoint.Source.GeoPoint,
    )
    geo = block(K.Map, map_point=point, zoom=3, map_width=4, map_height=5)
    assert block_json(geo) == lines(
        "{",
        ' "type": "map",',
        ' "geo": {',
        '  "source_type": "geo_point",',
        '  "latitude": 1.500000,',
        '  "longitude": -2.250000,',
        '  "access_hash": "-2",',
        '  "accuracy_radius": 5',
        " },",
        ' "zoom": 3,',
        ' "width": 4,',
        ' "height": 5,',
        *EMPTY_CAPTION,
        "}",
    )
    empty = RichMapPoint(latitude=1.0, source=RichMapPoint.Source.InputGeoPointEmpty)
    assert b' "geo": {\n  "source_type": "input_geo_point_empty"\n },' in block_json(
        block(K.InputMap, map_point=empty)
    )
    assert b'"type": "input_map"' in block_json(block(K.InputMap))


def test_channels():
    channel = RichChannel(
        title="T",
        username="u",
        access_hash=7,
        id=-3,
        source=RichChannel.Source.ChannelForbidden,
        megagroup=True,
    )
    assert block_json(block(K.Channel, channel=channel)) == lines(
        "{",
        ' "type": "channel",',
        ' "channel": {',
        '  "source_type": "channel_forbidden",',
        '  "id": "18446744073709551613",',
        '  "access_hash": "7",',
        '  "title": "T",',
        '  "username": "u",',
        '  "broadcast": false,',
        '  "megagroup": true,',
        '  "monoforum": false',
        " }",
        "}",
    )
    chat = RichChannel(id=4, source=RichChannel.Source.Chat, broadcast=True)
    assert block_json(block(K.Channel, channel=chat)) == lines(
        "{",
        ' "type": "channel",',
        ' "channel": {',
        '  "source_type": "chat",',
        '  "id": "4"',
        " }",
        "}",
    )


POSTER = Photo(image=Image(3, 2, File(relative_path="photos/4.jpg", size=9)))


def test_embed_with_poster_photo():
    embed = block(
        K.Embed,
        optional_url="https://e",
        html="<i>",
        poster_photo_id=4,
        width=10,
        height=20,
        full_width=True,
    )
    assert block_json(embed, photos={4: POSTER}) == lines(
        "{",
        ' "type": "embed",',
        ' "url": "https://e",',
        ' "html": "<i>",',
        ' "poster_photo_id": "4",',
        ' "poster_photo": "photos/4.jpg",',
        ' "poster_photo_file_size": 9,',
        ' "poster_photo_width": 3,',
        ' "poster_photo_height": 2,',
        ' "width": 10,',
        ' "height": 20,',
        ' "full_width": true,',
        ' "allow_scrolling": false,',
        *EMPTY_CAPTION,
        "}",
    )
    assert block_json(block(K.Embed)) == lines(
        "{",
        ' "type": "embed",',
        ' "full_width": false,',
        ' "allow_scrolling": false,',
        *EMPTY_CAPTION,
        "}",
    )


def test_embed_post_with_missing_author_photo():
    post = block(K.EmbedPost, url="u", webpage_id=1, author_photo_id=2, author="A")
    assert block_json(post) == lines(
        "{",
        ' "type": "embed_post",',
        ' "url": "u",',
        ' "webpage_id": "1",',
        ' "author_photo_id": "2",',
        ' "author_photo_skip_reason": "unavailable",',
        ' "author": "A",',
        ' "date": "1970-01-01T00:00:00",',
        ' "date_unixtime": "0",',
        ' "blocks": [],',
        *EMPTY_CAPTION,
        "}",
    )


def test_related_articles():
    articles = [
        RichRelatedArticle(
            url="u",
            title="t",
            description="d",
            author="a",
            photo_id=4,
            published_date=D,
            webpage_id=8,
        ),
        RichRelatedArticle(),
    ]
    related = block(K.RelatedArticles, text=plain("x"), related_articles=articles)
    assert block_json(related, photos={4: POSTER}) == lines(
        "{",
        ' "type": "related_articles",',
        ' "title": {',
        *PLAIN_X,
        " },",
        ' "articles": [',
        "  {",
        '   "url": "u",',
        '   "webpage_id": "8",',
        '   "title": "t",',
        '   "description": "d",',
        '   "photo_id": "4",',
        '   "photo": "photos/4.jpg",',
        '   "photo_file_size": 9,',
        '   "photo_width": 3,',
        '   "photo_height": 2,',
        '   "author": "a",',
        f'   "published_date": {ISO},',
        f'   "published_date_unixtime": {RAW}',
        "  },",
        "  {",
        '   "url": "",',
        '   "webpage_id": "0"',
        "  }",
        " ]",
        "}",
    )


@pytest.mark.parametrize(
    ("reason", "name"),
    [
        (SkipReason.Unavailable, "unavailable"),
        (SkipReason.FileType, "file_type"),
        (SkipReason.FileSize, "file_size"),
        (SkipReason.DateLimits, "date_limits"),
        (SkipReason.None_, "unavailable"),
    ],
)
def test_photo_block_skip_reasons(reason, name):
    photo = Photo(image=Image(0, 5, File(skip_reason=reason, size=5)))
    photo_block = block(K.Photo, photo_id=1, optional_url="", optional_webpage_id=0)
    assert block_json(photo_block, photos={1: photo}) == lines(
        "{",
        ' "type": "photo",',
        ' "photo_id": "1",',
        f' "photo_skip_reason": "{name}",',
        ' "photo_file_size": 5,',
        ' "spoiler": false,',
        ' "url": "",',
        ' "webpage_id": "0",',
        *EMPTY_CAPTION,
        "}",
    )


def test_file_block_with_sticker_document_and_thumbnail():
    sticker = Document(
        name="s.webp",
        mime="image/webp",
        is_sticker=True,
        sticker_emoji=":)",
        file=File(relative_path="stickers/s.webp", size=3),
        thumb=Image(width=5, file=File(skip_reason=SkipReason.FileType, size=1)),
    )
    assert block_json(block(K.File, document_id=1), documents={1: sticker}) == lines(
        "{",
        ' "type": "file",',
        ' "document_id": "1",',
        ' "file": "stickers/s.webp",',
        ' "file_name": "s.webp",',
        ' "file_size": 3,',
        ' "thumbnail_skip_reason": "file_type",',
        ' "thumbnail_file_size": 1,',
        ' "media_type": "sticker",',
        ' "sticker_emoji": ":)",',
        ' "mime_type": "image/webp",',
        *EMPTY_CAPTION,
        "}",
    )


def test_audio_block_with_song_metadata_and_dimensions():
    song = Document(
        is_audio_file=True,
        song_performer="P",
        song_title="T",
        duration=61,
        width=2,
        height=3,
        file=File(relative_path="files/a.mp3", size=7),
        thumb=Image(width=4, file=File(relative_path="files/a.jpg", size=2)),
    )
    assert block_json(block(K.Audio, document_id=9), documents={9: song}) == lines(
        "{",
        ' "type": "audio",',
        ' "document_id": "9",',
        ' "file": "files/a.mp3",',
        ' "file_size": 7,',
        ' "thumbnail": "files/a.jpg",',
        ' "thumbnail_file_size": 2,',
        ' "media_type": "audio_file",',
        ' "performer": "P",',
        ' "title": "T",',
        ' "duration_seconds": 61,',
        ' "width": 2,',
        ' "height": 3,',
        *EMPTY_CAPTION,
        "}",
    )
    assert block_json(block(K.Audio, document_id=8)) == lines(
        "{",
        ' "type": "audio",',
        ' "document_id": "8",',
        ' "file_skip_reason": "unavailable",',
        *EMPTY_CAPTION,
        "}",
    )


@pytest.mark.parametrize(
    ("flag", "name"),
    [
        ("is_video_message", "video_message"),
        ("is_voice_message", "voice_message"),
        ("is_animated", "animation"),
        ("is_video_file", "video_file"),
    ],
)
def test_video_block_media_types(flag, name):
    document = Document(file=File(relative_path="v"), **{flag: True})
    body = block_json(block(K.Video, document_id=1, loop=True), documents={1: document})
    assert (
        f' "file_size": 0,\n "media_type": "{name}",\n "autoplay": false,\n "loop": true,'
    ).encode() in body
