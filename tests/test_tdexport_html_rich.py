"""tdesktop HTML export: rich messages, album geometry and image thumbnails (spec 030,
stream C). Expected HTML is derived by hand from export_output_html.cpp and grouped_layout.cpp.
Synthetic data only; images are generated in tmp_path.
"""

from datetime import timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.html_layout import layout_media_group_geometry, united
from telegram_mcp.tdexport.html_message import MessageMixin
from telegram_mcp.tdexport.html_rich import render_rich_message
from telegram_mcp.tdexport.model import Document, File, Image, Photo
from telegram_mcp.tdexport.model_rich import (
    InlineButtonAction,
    InlineButtonPeerType,
    RichBlock,
    RichButtonAlignment,
    RichButtonPayload,
    RichButtonStyle,
    RichCaption,
    RichListItem,
    RichListItemContent,
    RichListKind,
    RichMessage,
    RichOrderedList,
    RichQuoteContent,
    RichTableAlignment,
    RichTableCell,
    RichTableRow,
    RichTableVerticalAlignment,
    RichTaskState,
    RichText,
)

T = RichText.Type
K = RichBlock.Kind
OPEN = '<div class="text rich_message" data-rich-message="5" data-rich-part="false" dir="auto">'


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def plain(text):
    return RichText(type=T.Plain, text=text)


def wrapped(kind, *children, **fields):
    return RichText(type=kind, children=list(children), **fields)


def render(blocks, base_path="", **message):
    wrap = MessageMixin("")
    rich = RichMessage(blocks=blocks, **message)
    return render_rich_message(
        wrap._context, rich, 5, "https://t.me/", "", wrap._rich_callbacks(base_path)
    )


def block(kind, **fields):
    return RichBlock(kind=kind, **fields)


def test_paragraph_links_and_anchors():
    text = wrapped(
        T.Concat,
        plain("Hi "),
        wrapped(T.Bold, plain("b")),
        wrapped(T.Url, plain("x"), data="https://a.com"),
        wrapped(T.Url, plain("to"), data="#sec"),
        wrapped(T.Url, wrapped(T.Url, plain("in"), data="https://b.com"), data="https://a.com"),
        wrapped(T.Url, plain("js"), data="javascript:x"),
    )
    html = render([block(K.Paragraph, text=text), block(K.Anchor, name="sec")])
    assert html == (
        '<div class="text rich_message" data-rich-message="5" data-rich-part="false"'
        ' dir="auto">'
        '\n <p class="rich_block rich_paragraph" data-rich-kind="paragraph" dir="auto">\n'
        "Hi <strong>b</strong>"
        '<a href="https://a.com">x</a>'
        '<a href="#rich-message-5-anchor-c2Vj">to</a>'
        '<a href="https://a.com"><span class="rich_inert_link">in</span></a>'
        '<span class="rich_inert_link">js</span>'
        "\n </p>\n"
        '<span class="rich_block rich_anchor" data-rich-kind="anchor"'
        ' id="rich-message-5-anchor-c2Vj"></span>'
        "</div>"
    )


def test_inline_kinds():
    button = RichText(
        type=T.Button,
        children=[plain("Copy")],
        button=RichButtonPayload(
            InlineButtonAction(type=InlineButtonAction.Type.CopyText, copy_text="a b"),
            RichButtonStyle.Primary,
        ),
    )
    text = wrapped(
        T.Concat,
        wrapped(T.Hashtag, plain("#tag")),
        wrapped(T.Mention, plain("@user_1")),
        RichText(type=T.CustomEmoji, text="E", custom_emoji_data="123", id=77),
        wrapped(T.Spoiler, plain("s")),
        RichText(type=T.Math, data="x<y"),
        wrapped(T.Diff),
        button,
    )
    html = render([block(K.Paragraph, text=text)])
    assert html == (
        '<div class="text rich_message" data-rich-message="5" data-rich-part="false"'
        ' dir="auto">'
        '\n <p class="rich_block rich_paragraph" data-rich-kind="paragraph" dir="auto">\n'
        '<a data-tag="tag" href="" onclick="return ShowHashtag(this.dataset.tag)">#tag</a>'
        '<a href="https://t.me/user_1">@user_1</a>'
        '<span class="rich_custom_emoji" data-document-id="77">'
        '<a href="" onclick="return ShowNotLoadedEmoji()">E</a></span>'
        '<span class="spoiler hidden" onclick="ShowSpoiler(this)">'
        '<span aria-hidden="true">s</span></span>'
        '<span class="rich_math_inline" data-rich-kind="inline-math" dir="ltr">x&lt;y</span>'
        '<span class="rich_diff" data-rich-kind="diff">'
        '<span class="rich_inline_fallback" data-rich-kind="unsupported-text">'
        "Unsupported rich text</span>"
        '<del class="rich_diff_previous">Previous: '
        '<span class="rich_inline_fallback" data-rich-kind="unsupported-text">'
        "Unsupported rich text</span></del></span>"
        '<button class="rich_button rich_button_inline rich_button_style_primary'
        ' rich_button_action_copy_text" data-button-style="primary"'
        ' data-button-type="copy_text" data-copy-text="a%20b"'
        ' onclick="return ShowTextCopied(decodeURIComponent(this.dataset.copyText))"'
        ' type="button">Copy</button>'
        "\n </p>\n"
        "</div>"
    )


def test_structure_blocks():
    ordered = block(
        K.List,
        list_kind=RichListKind.Ordered,
        ordered_list=RichOrderedList(type="a", start=3),
        list_items=[
            RichListItem(text=plain("one")),
            RichListItem(text=plain("two"), task_state=RichTaskState.Checked),
        ],
    )
    table = block(
        K.Table,
        text=plain("cap"),
        bordered=True,
        table_rows=[RichTableRow([RichTableCell(text=plain("h"), header=True, colspan=5000)])],
    )
    html = render(
        [
            block(K.Code, text=plain("x = 1"), language="py"),
            block(K.Divider),
            ordered,
            table,
            block(K.Unknown),
        ]
    )
    assert html == (
        '<div class="text rich_message" data-rich-message="5" data-rich-part="false"'
        ' dir="auto">'
        '<pre class="rich_block rich_code" data-language="py" data-rich-kind="code"'
        ' dir="auto"><code class="rich_code_content">x = 1</code></pre>'
        '\n <hr class="rich_block rich_divider" data-rich-kind="divider"/>\n'
        '\n <ol class="rich_block rich_list" data-rich-kind="ordered-list" start="3"'
        ' type="a">\n'
        '\n  <li class="rich_list_item" data-rich-content="text">\n'
        '<span class="rich_list_content">one</span>'
        "\n  </li>\n"
        '\n  <li class="rich_list_item rich_task_item" data-rich-content="text">\n'
        '<span aria-checked="true" class="rich_task_marker" role="checkbox">\u2611</span>'
        '<span class="rich_list_content">two</span>'
        "\n  </li>\n"
        "\n </ol>\n"
        '\n <div class="rich_block rich_table_wrap" data-rich-kind="table" tabindex="0">\n'
        '\n  <table class="rich_table bordered">\n'
        "\n   <caption>\ncap\n   </caption>\n"
        "\n   <tbody>\n"
        "\n    <tr>\n"
        '\n     <th class="rich_align_left rich_valign_top" colspan="1000"'
        ' data-source-colspan="5000">\nh\n     </th>\n'
        "\n    </tr>\n"
        "\n   </tbody>\n"
        "\n  </table>\n"
        "\n </div>\n"
        '\n <div class="rich_block rich_fallback" data-rich-kind="unknown">\n'
        "Unknown rich content"
        "\n </div>\n"
        "</div>"
    )


def test_missing_photo_renders_unavailable_card():
    html = render(
        [block(K.Photo, photo_id=1, caption=RichCaption(text=plain("cap")))],
        rtl=True,
    )
    assert html == (
        '<div class="text rich_message" data-rich-message="5" data-rich-part="false"'
        ' dir="rtl">'
        '\n <figure class="rich_block rich_media_item" data-photo-id="1"'
        ' data-rich-kind="photo" data-spoiler="false">\n'
        '\n  <div class="media_wrap clearfix">\n'
        '\n   <div class="media clearfix pull_left media_photo">\n'
        '\n    <div class="fill pull_left">\n'
        "\n    </div>\n"
        '\n    <div class="body">\n'
        '\n     <div class="title bold">\nPhoto\n     </div>\n'
        '\n     <div class="description">\nUnavailable, please try again later.\n     </div>\n'
        '\n     <div class="status details">\n0\u00d70, 0 B\n     </div>\n'
        "\n    </div>\n"
        "\n   </div>\n"
        "\n  </div>\n"
        '\n  <figcaption class="rich_media_caption" dir="auto">\n'
        '\n   <div class="rich_caption_text">\ncap\n   </div>\n'
        "\n  </figcaption>\n"
        "\n </figure>\n"
        "</div>"
    )


def test_collage_uses_album_geometry():
    photo = Photo(id=1, image=Image(100, 100, File(relative_path="photos/a.jpg")))
    blocks = [block(K.Photo, photo_id=1), block(K.Photo, photo_id=1)]
    html = render([block(K.Collage, blocks=blocks)], photos={1: photo})
    assert (
        '\n <section class="rich_block rich_media_group" data-rich-has-items="true"'
        ' data-rich-items-end-media="true" data-rich-kind="collage">\n'
        '\n  <div class="rich_collage_box" style="aspect-ratio: 430 / 213">\n'
        '\n   <div class="rich_collage_item" style="left: 0.000%; top: 0.000%;'
        ' width: 49.535%; height: 100.000%">\n'
    ) in html
    assert 'style="left: 50.465%; top: 0.000%; width: 49.535%; height: 100.000%"' in html


def test_layout_geometry():
    assert layout_media_group_geometry([(200, 100)], 430, 100, 4) == [(0, 0, 430, 215)]
    assert layout_media_group_geometry([(100, 100), (100, 100)], 430, 100, 4) == [
        (0, 0, 213, 213),
        (217, 0, 213, 213),
    ]
    three = layout_media_group_geometry([(50, 100), (100, 100), (100, 100)], 430, 100, 4)
    assert three == [(0, 0, 213, 430), (217, 0, 213, 213), (217, 217, 213, 213)]
    five = layout_media_group_geometry([(100, 100)] * 5, 430, 100, 4)
    assert len(five) == 5 and all(rect[1] >= 0 for rect in five)
    assert united(None, (1, 2, 3, 4)) == (1, 2, 3, 4)
    assert united((0, 0, 10, 10), (20, 5, 5, 20)) == (0, 0, 25, 25)


def _image(path, size, color="red", fmt="JPEG"):
    from PIL import Image as PilImage

    path.parent.mkdir(parents=True, exist_ok=True)
    PilImage.new("RGB", size, color).save(path, fmt)


def test_photo_sticker_and_video_thumbnails(tmp_path):
    pytest.importorskip("PIL")
    base = str(tmp_path).replace("\\", "/") + "/"
    _image(tmp_path / "photos" / "p.jpg", (200, 100))
    _image(tmp_path / "photos" / "small.jpg", (50, 50))
    _image(tmp_path / "stickers" / "s.webp", (512, 512), fmt="WEBP")
    wrap = MessageMixin("")
    photo = Photo(image=Image(200, 100, File(relative_path="photos/p.jpg")))
    assert wrap.push_photo_media(photo, base) == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <a class="photo_wrap clearfix pull_left" href="photos/p.jpg">\n'
        '\n  <img class="photo" src="photos/p_thumb.jpg" style="width: 100px; height: 50px"/>\n'
        "\n </a>\n"
        "\n</div>\n"
    )
    assert (tmp_path / "photos" / "p_thumb.jpg").exists()
    small = Photo(image=Image(50, 50, File(relative_path="photos/small.jpg", size=900)))
    html = wrap.push_photo_media(small, base)
    assert 'href="photos/small.jpg"' in html and "\n50\u00d750\n" in html
    sticker = Document(is_sticker=True, file=File(relative_path="stickers/s.webp"))
    assert wrap.push_sticker_media(sticker, base) == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <a class="sticker_wrap clearfix pull_left" href="stickers/s.webp">\n'
        '\n  <img class="sticker" src="stickers/s_thumb.webp" style="width: 192px;'
        ' height: 192px"/>\n'
        "\n </a>\n"
        "\n</div>\n"
    )
    video = Document(
        is_video_file=True,
        width=1280,
        height=720,
        duration=75,
        file=File(relative_path="video_files/v.mp4"),
        thumb=Image(file=File(relative_path="video_files/v.mp4_thumb.jpg")),
    )
    assert wrap.push_video_file_media(video, base) == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <a class="video_file_wrap clearfix pull_left" href="video_files/v.mp4">\n'
        '\n  <div class="video_play_bg">\n'
        '\n   <div class="video_play">\n'
        "\n   </div>\n"
        "\n  </div>\n"
        '\n  <div class="video_duration">\n01:15\n  </div>\n'
        '\n  <img class="video_file" src="video_files/v.mp4_thumb.jpg"'
        ' style="width: 260px; height: 146px"/>\n'
        "\n </a>\n"
        "\n</div>\n"
    )
    gif = Document(is_animated=True, width=100, height=100, file=File(size=5000))
    html = wrap.push_animated_media(gif, base)
    assert "Animation" in html and "4.8 KB" in html and "media_video" in html


DATE = 86400 * 365  # 01.01.1971 00:00:00 UTC
OPEN_PART = (
    '<div class="text rich_message" data-rich-message="5" data-rich-part="true" dir="auto">'
)


def test_text_blocks_math_author_date_and_fallbacks():
    html = render(
        [
            block(K.Heading, heading_level=2, text=plain("H")),
            block(K.Heading, heading_level=9, text=plain("h")),
            block(K.Footer, text=plain("f")),
            block(K.Thinking, text=plain("t")),
            block(K.Math, formula="a<b"),
            block(K.AuthorDate, text=plain("Au"), date=DATE),
            block(K.AuthorDate),
            block(K.Unsupported),
            block(K.Code, text=plain("x")),
        ],
        part=True,
    )
    assert html == (
        OPEN_PART
        + '\n <h2 class="rich_block rich_heading" data-level="2" data-rich-kind="heading">\n'
        "H\n </h2>\n"
        '\n <h6 class="rich_block rich_heading" data-level="9" data-rich-kind="heading">\n'
        "h\n </h6>\n"
        '\n <footer class="rich_block rich_footer" data-rich-kind="footer" dir="auto">\n'
        "f\n </footer>\n"
        '\n <p class="rich_block rich_thinking" data-rich-kind="thinking" dir="auto">\n'
        "t\n </p>\n"
        '<div class="rich_block rich_math_display" data-rich-kind="math" dir="ltr">a&lt;b</div>'
        '\n <address class="rich_block rich_author_date" data-rich-kind="author-date"'
        ' dir="auto">\n'
        'Au \u2022 <time data-date="31536000" datetime="1971-01-01T00:00:00" dir="ltr">'
        "01.01.1971 00:00:00</time>"
        "\n </address>\n"
        '\n <address class="rich_block rich_author_date" data-rich-kind="author-date"'
        ' dir="auto">\n'
        "\n </address>\n"
        '\n <div class="rich_block rich_fallback" data-rich-kind="unsupported">\n'
        "Unsupported rich content"
        "\n </div>\n"
        '<pre class="rich_block rich_code" data-rich-kind="code" dir="auto">'
        '<code class="rich_code_content">x</code></pre>'
        "</div>"
    )


def test_task_list_and_block_items():
    items = [
        RichListItem(text=plain("a"), task_state=RichTaskState.Unchecked),
        RichListItem(
            blocks=[block(K.Paragraph, text=plain("p"))], content=RichListItemContent.Blocks
        ),
    ]
    html = render([block(K.List, list_items=items)])
    assert html == (
        OPEN + '\n <ul class="rich_block rich_list" data-rich-kind="list">\n'
        '\n  <li class="rich_list_item rich_task_item" data-rich-content="text">\n'
        '<span aria-checked="false" class="rich_task_marker" role="checkbox">\u2610</span>'
        '<span class="rich_list_content">a</span>'
        "\n  </li>\n"
        '\n  <li class="rich_list_item" data-rich-content="blocks">\n'
        '<div class="rich_list_content">'
        '\n    <p class="rich_block rich_paragraph" data-rich-kind="paragraph" dir="auto">\n'
        "p\n    </p>\n"
        "</div>"
        "\n  </li>\n"
        "\n </ul>\n"
        "</div>"
    )


def test_ordered_list_custom_markers_and_non_native_type():
    items = [
        RichListItem(text=plain("one"), num="iv", type="t", value=4),
        RichListItem(num=""),
    ]
    listing = block(
        K.List,
        list_kind=RichListKind.Ordered,
        ordered_list=RichOrderedList(type="x", reversed=True),
        list_items=items,
    )
    assert render([listing]) == (
        OPEN + '\n <ol class="rich_block rich_list" data-list-type="x"'
        ' data-rich-kind="ordered-list" reversed="">\n'
        '\n  <li class="rich_list_item rich_order_item" data-item-type="t" data-num="iv"'
        ' data-rich-content="text" value="4">\n'
        '<span class="rich_order_marker">iv</span><span class="rich_list_content">one</span>'
        "\n  </li>\n"
        '\n  <li class="rich_list_item" data-num="" data-rich-content="text">\n'
        '<span class="rich_list_content"></span>'
        "\n  </li>\n"
        "\n </ol>\n"
        "</div>"
    )


def test_quotes_and_details():
    html = render(
        [
            block(K.Quote, text=plain("q"), quote_caption=plain("c")),
            block(
                K.Quote,
                pullquote=True,
                quote_content=RichQuoteContent.Blocks,
                blocks=[block(K.Divider)],
            ),
            block(K.Quote, quote_content=RichQuoteContent.Blocks),
            block(
                K.Details, open=True, text=plain("S"), blocks=[block(K.Paragraph, text=plain("b"))]
            ),
            block(K.Details),
        ]
    )
    assert html == (
        OPEN + '\n <blockquote class="rich_block rich_quote" data-rich-kind="quote">\n'
        '\n  <div class="rich_quote_body">\nq\n  </div>\n'
        '\n  <cite class="rich_quote_caption">\nc\n  </cite>\n'
        "\n </blockquote>\n"
        '\n <aside class="rich_block rich_quote rich_pullquote" data-rich-kind="pullquote">\n'
        '\n  <div class="rich_quote_body">\n'
        '\n   <hr class="rich_block rich_divider" data-rich-kind="divider"/>\n'
        "\n  </div>\n"
        "\n </aside>\n"
        '\n <blockquote class="rich_block rich_quote" data-rich-kind="quote-blocks">\n'
        '\n  <div class="rich_quote_body">\n'
        "\n  </div>\n"
        "\n </blockquote>\n"
        '\n <details class="rich_block rich_details" data-rich-kind="details" open="">\n'
        "\n  <summary>\nS\n  </summary>\n"
        '\n  <div class="rich_details_body">\n'
        '\n   <p class="rich_block rich_paragraph" data-rich-kind="paragraph" dir="auto">\n'
        "b\n   </p>\n"
        "\n  </div>\n"
        "\n </details>\n"
        '\n <details class="rich_block rich_details" data-rich-kind="details">\n'
        "\n  <summary>\n\n  </summary>\n"
        '\n  <div class="rich_details_body">\n'
        "\n  </div>\n"
        "\n </details>\n"
        "</div>"
    )


def test_table_flags_alignments_and_spans():
    cells = [
        RichTableCell(
            text=plain("a"),
            alignment=RichTableAlignment.Center,
            vertical_alignment=RichTableVerticalAlignment.Middle,
            rowspan=2,
        ),
        RichTableCell(
            alignment=RichTableAlignment.Right,
            vertical_alignment=RichTableVerticalAlignment.Bottom,
            colspan=0,
        ),
    ]
    table = block(K.Table, striped=True, compact=True, table_rows=[RichTableRow(cells)])
    assert render([table]) == (
        OPEN + '\n <div class="rich_block rich_table_wrap" data-rich-kind="table" tabindex="0">\n'
        '\n  <table class="rich_table striped compact">\n'
        "\n   <tbody>\n"
        "\n    <tr>\n"
        '\n     <td class="rich_align_center rich_valign_middle" rowspan="2">\na\n     </td>\n'
        '\n     <td class="rich_align_right rich_valign_bottom">\n\n     </td>\n'
        "\n    </tr>\n"
        "\n   </tbody>\n"
        "\n  </table>\n"
        "\n </div>\n"
        "</div>"
    )


def _button(label, action, style=None):
    return RichText(
        type=T.Button, children=[plain(label)], button=RichButtonPayload(action, style)
    )


def test_button_row_writes_every_action_kind():
    A = InlineButtonAction.Type
    buttons = [
        _button("Go", InlineButtonAction(type=A.Url, url="https://example.com/x")),
        _button("No", InlineButtonAction(type=A.Url, url="javascript:x")),
        _button(
            "Au",
            InlineButtonAction(type=A.Auth, url="u", forward_text="f t", button_id=3),
            RichButtonStyle.Success,
        ),
        _button(
            "Sw",
            InlineButtonAction(
                type=A.SwitchInline,
                query="q q",
                same_peer=True,
                peer_types=[InlineButtonPeerType.PM, InlineButtonPeerType.Broadcast],
            ),
        ),
        _button("Me", InlineButtonAction(type=A.UserProfile, user_id=42)),
        _button("Off", InlineButtonAction(type=A.Disabled)),
        _button(
            "Pw",
            InlineButtonAction(
                type=A.CallbackWithPassword, callback_data=b"\xff\xfe", requires_password=True
            ),
        ),
        _button("G", InlineButtonAction(type=A.Game)),
    ]
    row = block(K.ButtonRow, buttons=buttons, button_alignment=RichButtonAlignment.Center)
    style = "rich_button rich_button_block rich_button_style_"
    assert render([row]) == (
        OPEN + '\n <div class="rich_block rich_button_row rich_button_align_center"'
        ' data-button-alignment="center" data-rich-kind="button-row">\n'
        '\n  <div class="rich_button_group">\n'
        f'<a class="{style}default rich_button_action_url" data-button-type="url"'
        ' data-button-url="https%3A%2F%2Fexample.com%2Fx" href="https://example.com/x">Go</a>'
        f'<span class="{style}default rich_button_action_url" data-button-type="url"'
        ' data-button-url="javascript%3Ax">No</span>'
        f'<span class="{style}success rich_button_action_auth"'
        ' data-button-forward-text="f%20t" data-button-id="3" data-button-style="success"'
        ' data-button-type="auth" data-button-url="u">Au</span>'
        f'<span class="{style}default rich_button_action_switch_inline"'
        ' data-button-peer-types="%5B%22pm%22%2C%22broadcast%22%5D" data-button-query="q%20q"'
        ' data-button-same-peer="true" data-button-type="switch_inline">Sw</span>'
        f'<span class="{style}default rich_button_action_user_profile"'
        ' data-button-type="user_profile" data-button-user-id="42">Me</span>'
        f'<span aria-disabled="true" class="{style}default rich_button_action_disabled"'
        ' data-button-type="disabled">Off</span>'
        f'<span class="{style}default rich_button_action_callback_with_password"'
        ' data-button-requires-password="true" data-button-type="callback_with_password"'
        ' data-callback-data-base64="__4">Pw</span>'
        f'<span class="{style}default rich_button_action_game" data-button-type="game">G</span>'
        "\n  </div>\n"
        "\n </div>\n"
        "</div>"
    )


def test_malformed_button_and_unknown_block_kind_raise():
    broken = RichText(type=T.Button, children=[], button=RichButtonPayload())
    with pytest.raises(ValueError, match="Malformed rich button"):
        render([block(K.ButtonRow, buttons=[broken])])
    with pytest.raises(ValueError, match="Kind in RichHtmlRenderer::renderBlock."):
        render([block("not a kind")])


@pytest.mark.parametrize(
    ("sizes", "expected"),
    [
        ([], []),
        # Two wide items of nearly equal ratio stack top to bottom.
        ([(180, 100), (190, 100)], [(0, 0, 430, 213), (0, 217, 430, 213)]),
        ([(150, 100), (190, 100)], [(0, 0, 213, 112), (217, 0, 213, 112)]),
        ([(50, 100), (100, 100)], [(0, 0, 150, 276), (154, 0, 276, 276)]),
        (
            [(200, 100), (100, 100), (100, 100)],
            [(0, 0, 430, 215), (0, 219, 213, 211), (217, 219, 213, 211)],
        ),
        (
            [(200, 100), (100, 100), (100, 100), (100, 100)],
            [(0, 0, 430, 215), (0, 219, 141, 141), (145, 219, 140, 141), (289, 219, 141, 141)],
        ),
        (
            [(100, 100)] * 4,
            [(0, 0, 256, 430), (260, 0, 141, 141), (260, 145, 141, 141), (260, 290, 141, 140)],
        ),
        # A ratio above 2 sends even two items to the complex layouter (ratios cropped to 2.75).
        ([(300, 100), (100, 100)], [(0, 0, 430, 156), (0, 160, 430, 430)]),
    ],
)
def test_layout_geometry_variants(sizes, expected):
    assert layout_media_group_geometry(sizes, 430, 100, 4) == expected


def test_united_with_empty_and_negative_rects():
    assert united((0, 0, 5, 5), (3, 3, 0, 0)) == (0, 0, 5, 5)
    assert united((10, 0, -5, 2), (0, 0, 1, 1)) == (0, 0, 11, 2)
