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
    RichBlock,
    RichButtonPayload,
    RichButtonStyle,
    RichCaption,
    RichListItem,
    RichListKind,
    RichMessage,
    RichOrderedList,
    RichTableCell,
    RichTableRow,
    RichTaskState,
    RichText,
)

T = RichText.Type
K = RichBlock.Kind


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
