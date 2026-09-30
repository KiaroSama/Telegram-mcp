"""tdesktop HTML export: rich-message media blocks, groups, embeds and references.

Expected HTML is derived by hand from Telegram Desktop v7.2.10's export_output_html.cpp
(RichHtmlRenderer::renderPhoto/renderVideo/renderAudio/renderFile/renderMediaGroup/
renderCollage/renderSlideshow/renderEmbed/renderEmbedPost/renderChannel/renderMap/
renderRelatedArticle(s)/renderSourceMeta/renderCaption and tailState). The media callbacks are
stubs that print what they were handed, so each test pins exactly the wrapper markup and the
MediaData the renderer builds. Synthetic data only.
"""

import math
from datetime import timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.html_rich import render_rich_message
from telegram_mcp.tdexport.html_rich_media import RichMediaCallbacks
from telegram_mcp.tdexport.html_text import HtmlContext
from telegram_mcp.tdexport.model import Document, File, Image, Photo
from telegram_mcp.tdexport.model_rich import (
    RichBlock,
    RichCaption,
    RichChannel,
    RichListItem,
    RichListItemContent,
    RichMapPoint,
    RichMessage,
    RichQuoteContent,
    RichRelatedArticle,
    RichText,
)

K = RichBlock.Kind
DATE = 86400 * 365  # 01.01.1971 00:00:00 UTC
OPEN = '<div class="text rich_message" data-rich-message="5" data-rich-part="false" dir="auto">'
UNAVAILABLE = "Unavailable, please try again later."


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def plain(text):
    return RichText(type=RichText.Type.Plain, text=text)


def block(kind, **fields):
    return RichBlock(kind=kind, **fields)


def _id(item):
    return item.id if item is not None else None


def _card(media):
    return f"{media.title}|{media.description}|{media.status}|{media.classes}|{media.link}"


def stubs():
    return RichMediaCallbacks(
        photo=lambda photo: f"[photo {_id(photo)}]",
        video=lambda document: f"[video {_id(document)}]",
        audio=lambda document: f"[audio {_id(document)}]",
        file=lambda document: f"[file {_id(document)}]",
        generic=lambda media: f"[generic {_card(media)}]",
        photo_card=lambda media, photo: f"[card {_card(media)}|{_id(photo)}]",
    )


def render(blocks, **message):
    rich = RichMessage(blocks=blocks, **message)
    return render_rich_message(HtmlContext(), rich, 5, "https://t.me/", "", stubs())


def body(html):
    assert html.startswith(OPEN) and html.endswith("</div>")
    return html[len(OPEN) : -len("</div>")]


def photo_with_file(photo_id, width=100, height=100):
    return Photo(
        id=photo_id, image=Image(width, height, File(relative_path=f"photos/{photo_id}.jpg"))
    )


def figure(photo_id, depth=1, spoiler="false"):
    pad = " " * depth
    return (
        f'\n{pad}<figure class="rich_block rich_media_item" data-photo-id="{photo_id}"'
        f' data-rich-kind="photo" data-spoiler="{spoiler}">\n'
        f"[photo {photo_id}]"
        f"\n{pad}</figure>\n"
    )


def test_photo_with_webpage_source_link_and_caption():
    photo = block(
        K.Photo,
        photo_id=7,
        spoiler=True,
        optional_webpage_id=3,
        optional_url="https://example.com/a",
        caption=RichCaption(text=plain("c"), credit=plain("by")),
    )
    assert body(render([photo], photos={7: Photo(id=7)})) == (
        '\n <figure class="rich_block rich_media_item" data-photo-id="7" data-rich-kind="photo"'
        ' data-spoiler="true" data-webpage-id="3">\n'
        "[photo 7]"
        '\n  <div class="rich_source_meta">\n'
        '<a class="rich_source_link" href="https://example.com/a">https://example.com/a</a>'
        "\n  </div>\n"
        '\n  <figcaption class="rich_media_caption" dir="auto">\n'
        '\n   <div class="rich_caption_text">\nc\n   </div>\n'
        '\n   <cite class="rich_caption_credit">\nby\n   </cite>\n'
        "\n  </figcaption>\n"
        "\n </figure>\n"
    )


def test_source_that_is_not_a_web_link_is_plain_text_and_empty_source_is_dropped():
    html = body(render([block(K.Photo, photo_id=1, optional_url="not <a> link")]))
    assert (
        '\n  <div class="rich_source_meta">\n'
        '<span class="rich_source_text">not &lt;a&gt; link</span>'
        "\n  </div>\n"
    ) in html
    # A present but empty URL writes no meta, and only the credit makes a caption.
    html = body(
        render(
            [block(K.Photo, photo_id=1, optional_url="", caption=RichCaption(credit=plain("x")))]
        )
    )
    assert "rich_source_meta" not in html and "rich_caption_text" not in html
    assert '\n   <cite class="rich_caption_credit">\nx\n   </cite>\n' in html


def test_video_audio_and_file_figures():
    html = body(
        render(
            [
                block(K.Video, document_id=4, autoplay=True),
                block(K.Audio, document_id=0),
                block(K.File, document_id=4),
            ],
            documents={4: Document(id=4)},
        )
    )
    assert html == (
        '\n <figure class="rich_block rich_media_item" data-autoplay="true" data-document-id="4"'
        ' data-loop="false" data-rich-kind="video" data-spoiler="false">\n'
        "[video 4]"
        "\n </figure>\n"
        '\n <figure class="rich_block rich_media_item" data-document-id="0"'
        ' data-rich-kind="audio">\n'
        "[audio None]"
        "\n </figure>\n"
        '\n <figure class="rich_block rich_media_item" data-document-id="4"'
        ' data-rich-kind="file">\n'
        "[file 4]"
        "\n </figure>\n"
    )


def test_cover_group_never_writes_its_caption():
    cover = block(K.Cover, blocks=[block(K.Photo, photo_id=7)], caption=RichCaption(plain("c")))
    assert body(render([cover], photos={7: Photo(id=7)})) == (
        '\n <section class="rich_block rich_media_group" data-rich-has-items="true"'
        ' data-rich-items-end-media="true" data-rich-kind="cover">\n'
        '\n  <div class="rich_media_group_items">\n' + figure(7, 3) + "\n  </div>\n"
        "\n </section>\n"
    )


def test_collage_without_sizes_falls_back_to_a_group_with_caption():
    # The photo has no file, so mediaGroupSizes is empty.
    collage = block(
        K.Collage, blocks=[block(K.Photo, photo_id=7)], caption=RichCaption(plain("c"))
    )
    assert body(render([collage], photos={7: Photo(id=7, image=Image(10, 10))})) == (
        '\n <section class="rich_block rich_media_group" data-rich-has-items="true"'
        ' data-rich-items-end-media="true" data-rich-kind="collage">\n'
        '\n  <div class="rich_media_group_items">\n' + figure(7, 3) + "\n  </div>\n"
        '\n  <figcaption class="rich_media_caption" dir="auto">\n'
        '\n   <div class="rich_caption_text">\nc\n   </div>\n'
        "\n  </figcaption>\n"
        "\n </section>\n"
    )
    empty = body(render([block(K.Collage)]))
    assert 'data-rich-has-items="false" data-rich-items-end-media="false"' in empty


def test_collage_of_videos_needs_the_file_and_the_thumbnail():
    def video(document_id, thumb):
        return Document(
            id=document_id,
            width=100,
            height=100,
            file=File(relative_path="video_files/v.mp4"),
            thumb=Image(file=File(relative_path=thumb)),
        )

    items = [block(K.Video, document_id=1), block(K.Video, document_id=2)]
    ready = render(
        [block(K.Collage, blocks=items)], documents={1: video(1, "t"), 2: video(2, "t")}
    )
    assert '<div class="rich_collage_box" style="aspect-ratio: 430 / 213">' in ready
    assert "[video 1]" in ready and "[video 2]" in ready
    no_thumb = render(
        [block(K.Collage, blocks=items)], documents={1: video(1, "t"), 2: video(2, "")}
    )
    assert "rich_collage_box" not in no_thumb and "rich_media_group_items" in no_thumb


def test_slideshow_with_controls_uses_the_widest_aspect_ratio():
    slides = [block(K.Photo, photo_id=1), block(K.Photo, photo_id=2)]
    photos = {1: photo_with_file(1, 100, 50), 2: photo_with_file(2, 50, 100)}
    assert body(render([block(K.Slideshow, blocks=slides)], photos=photos)) == (
        '\n <section class="rich_block rich_media_group" data-rich-has-items="true"'
        ' data-rich-items-end-media="true" data-rich-kind="slideshow">\n'
        '\n  <div class="rich_slideshow_box" style="aspect-ratio: 100 / 50">\n'
        '\n   <div class="rich_slideshow_track">\n'
        '\n    <div class="rich_slide">\n' + figure(1, 5) + "\n    </div>\n"
        '\n    <div class="rich_slide">\n' + figure(2, 5) + "\n    </div>\n"
        "\n   </div>\n"
        '<button class="rich_slideshow_prev" type="button">&#8592;</button>'
        '<button class="rich_slideshow_next" type="button">&#8594;</button>'
        '\n   <div class="rich_slideshow_dots">\n'
        '<button class="rich_slideshow_dot" type="button"></button>'
        '<button class="rich_slideshow_dot" type="button"></button>'
        "\n   </div>\n"
        "\n  </div>\n"
        "\n </section>\n"
    )


def test_single_slide_has_no_controls_and_a_linked_slide_falls_back():
    photos = {1: photo_with_file(1, 30, 60)}
    html = render([block(K.Slideshow, blocks=[block(K.Photo, photo_id=1)])], photos=photos)
    assert 'style="aspect-ratio: 30 / 60"' in html and "rich_slideshow_prev" not in html
    linked = block(K.Photo, photo_id=1, optional_url="https://example.com")
    html = render([block(K.Slideshow, blocks=[linked])], photos=photos)
    assert "rich_slideshow_box" not in html
    assert 'data-rich-items-end-media="false" data-rich-kind="slideshow"' in html


def test_embed_with_poster_and_source_meta():
    embed = block(
        K.Embed,
        poster_photo_id=7,
        optional_url="https://example.com/e",
        full_width=True,
        width=640,
    )
    assert body(render([embed])) == (
        '\n <figure class="rich_block rich_media_item rich_embed" data-allow-scrolling="false"'
        ' data-full-width="true" data-poster-photo-id="7" data-rich-has-meta="true"'
        ' data-rich-kind="embed" data-source-width="640">\n'
        "[photo None]"
        '\n  <div class="rich_source_meta">\n'
        '<a class="rich_source_link" href="https://example.com/e">https://example.com/e</a>'
        "\n  </div>\n"
        "\n </figure>\n"
    )


def test_embed_without_poster_is_a_generic_card():
    embed = block(K.Embed, optional_url="https://example.com/v", allow_scrolling=True, height=360)
    assert body(render([embed, block(K.Embed)])) == (
        '\n <figure class="rich_block rich_media_item rich_embed" data-allow-scrolling="true"'
        ' data-full-width="false" data-rich-has-meta="false" data-rich-kind="embed"'
        ' data-source-height="360">\n'
        "[generic Embed||https://example.com/v|media_file|https://example.com/v]"
        "\n </figure>\n"
        '\n <figure class="rich_block rich_media_item rich_embed" data-allow-scrolling="false"'
        ' data-full-width="false" data-rich-has-meta="false" data-rich-kind="embed">\n'
        "[generic Embed|||media_file|]"
        "\n </figure>\n"
    )
    # A non-web URL is shown as the status but not linked.
    assert "[generic Embed||ftp://x|media_file|]" in render(
        [block(K.Embed, optional_url="ftp://x")]
    )


def test_embed_post_author_card_meta_and_blocks():
    post = block(
        K.EmbedPost,
        author="Au",
        author_photo_id=7,
        date=DATE,
        url="https://example.com/p",
        webpage_id=9,
        blocks=[block(K.Paragraph, text=plain("t"))],
    )
    assert body(render([post], photos={7: Photo(id=7)})) == (
        '\n <section class="rich_block rich_embed_post" data-author-photo-id="7"'
        f' data-date="{DATE}" data-rich-has-items="true" data-rich-has-meta="true"'
        ' data-rich-items-end-media="false" data-rich-kind="embed-post" data-webpage-id="9">\n'
        '\n  <div class="rich_embed_post_author">\n'
        f"[card Au|{UNAVAILABLE}|01.01.1971 00:00:00|media_contact||7]"
        "\n  </div>\n"
        '\n  <div class="rich_embed_post_meta">\n'
        '<a class="rich_source_link" href="https://example.com/p">https://example.com/p</a>'
        "\n  </div>\n"
        '\n  <div class="rich_embed_post_blocks">\n'
        '\n   <p class="rich_block rich_paragraph" data-rich-kind="paragraph" dir="auto">\n'
        "t"
        "\n   </p>\n"
        "\n  </div>\n"
        "\n </section>\n"
    )


def test_embed_post_defaults():
    html = body(render([block(K.EmbedPost)]))
    assert f"[card Embed post|{UNAVAILABLE}||media_contact||None]" in html
    assert 'data-rich-has-meta="false" data-rich-items-end-media="false"' in html
    assert "rich_embed_post_meta" not in html


def test_channel_reference_links_a_valid_username():
    channel = RichChannel(
        username="durov_x",
        access_hash=-5,
        id=12,
        source=RichChannel.Source.Channel,
        broadcast=True,
    )
    assert body(render([block(K.Channel, channel=channel)])) == (
        '\n <section class="rich_block rich_reference_block" data-access-hash="-5"'
        ' data-broadcast="true" data-channel-id="12" data-channel-source="channel"'
        ' data-megagroup="false" data-monoforum="false" data-rich-kind="channel">\n'
        "[generic Channel||@durov_x, Channel|media_contact|https://t.me/durov_x]"
        "\n </section>\n"
    )


@pytest.mark.parametrize(
    ("channel", "card"),
    [
        (
            RichChannel(title="T", username="ab", source=RichChannel.Source.ChatForbidden),
            "T||@ab, Chat unavailable|media_contact|",
        ),
        (
            RichChannel(title="", source=RichChannel.Source.ChatEmpty),
            "Channel||Chat|media_contact|",
        ),
        (RichChannel(source=RichChannel.Source.Chat), "Channel||Chat|media_contact|"),
        (
            RichChannel(username="", source=RichChannel.Source.ChannelForbidden),
            "Channel||Channel unavailable|media_contact|",
        ),
        (
            RichChannel(username="bad-name", source=RichChannel.Source.Community),
            "Channel||@bad-name, Community|media_contact|",
        ),
        (
            RichChannel(source=RichChannel.Source.CommunityForbidden, megagroup=True),
            "Channel||Community unavailable|media_contact|",
        ),
    ],
)
def test_channel_status_labels(channel, card):
    assert f"[generic {card}]" in render([block(K.Channel, channel=channel)])


def test_map_with_coordinates():
    point = RichMapPoint(
        access_hash=3,
        accuracy_radius=10,
        latitude=55.75,
        longitude=37.62,
        source=RichMapPoint.Source.GeoPoint,
    )
    map_block = block(K.Map, map_point=point, zoom=15, map_width=600, map_height=300)
    link = "https://maps.google.com/maps?q=55.750000,37.620000&ll=55.750000,37.620000&z=16"
    assert body(render([map_block])) == (
        '\n <figure class="rich_block rich_reference_block" data-access-hash="3"'
        ' data-accuracy-radius="10" data-point-source="geo_point" data-rich-kind="map"'
        ' data-source-height="300" data-source-width="600" data-zoom="15">\n'
        f"[generic Location||55.750000, 37.620000|media_location|{link}]"
        "\n </figure>\n"
    )


@pytest.mark.parametrize(
    ("source", "latitude", "longitude"),
    [
        (RichMapPoint.Source.InputGeoPointEmpty, 1.0, 1.0),
        (RichMapPoint.Source.GeoPointEmpty, 1.0, 1.0),
        (RichMapPoint.Source.InputGeoPoint, 91.0, 0.0),
        (RichMapPoint.Source.InputGeoPoint, 0.0, -180.5),
        (RichMapPoint.Source.GeoPoint, math.nan, 0.0),
        (RichMapPoint.Source.GeoPoint, 0.0, math.inf),
    ],
)
def test_map_without_valid_coordinates_has_no_status_or_link(source, latitude, longitude):
    point = RichMapPoint(latitude=latitude, longitude=longitude, source=source)
    html = render([block(K.InputMap, map_point=point, caption=RichCaption(plain("here")))])
    assert 'data-rich-kind="input-map"' in html
    assert "[generic Location|||media_location|]" in html
    assert '<div class="rich_caption_text">\nhere\n' in html


def test_input_map_point_source_name():
    point = RichMapPoint(latitude=-90.0, longitude=180.0, source=RichMapPoint.Source.InputGeoPoint)
    html = render([block(K.InputMap, map_point=point)])
    assert 'data-point-source="input_geo_point"' in html
    assert "|-90.000000, 180.000000|media_location|https://maps.google.com/maps?q=" in html


def test_related_articles():
    articles = [
        RichRelatedArticle(
            url="https://example.com/1",
            title="One",
            description="d",
            author="Au",
            photo_id=7,
            published_date=DATE,
            webpage_id=11,
        ),
        RichRelatedArticle(webpage_id=12),
    ]
    related = block(K.RelatedArticles, text=plain("More"), related_articles=articles)
    assert body(render([related], photos={7: Photo(id=7)})) == (
        '\n <section class="rich_block rich_related_articles" data-rich-kind="related-articles">\n'
        '\n  <div class="rich_related_title" dir="auto">\nMore\n  </div>\n'
        '\n  <div class="rich_related_items">\n'
        '\n   <article class="rich_related_article" data-photo-id="7"'
        f' data-published-date="{DATE}" data-rich-article-index="0" data-webpage-id="11">\n'
        f"[card One|{UNAVAILABLE}|Au, 01.01.1971 00:00:00|media_photo||7]"
        '\n    <div class="rich_article_description">\nd\n    </div>\n'
        '\n    <div class="rich_source_meta">\n'
        '<a class="rich_source_link" href="https://example.com/1">https://example.com/1</a>'
        "\n    </div>\n"
        "\n   </article>\n"
        '\n   <article class="rich_related_article" data-rich-article-index="1"'
        ' data-webpage-id="12">\n'
        "[generic Related article|||media_file|]"
        "\n   </article>\n"
        "\n  </div>\n"
        "\n </section>\n"
    )


def test_related_article_with_a_downloaded_photo_keeps_its_description():
    article = RichRelatedArticle(description="d", photo_id=7, published_date=DATE)
    html = render(
        [block(K.RelatedArticles, related_articles=[article])], photos={7: photo_with_file(7)}
    )
    assert "[card Related article|d|01.01.1971 00:00:00|media_photo||7]" in html
    assert "rich_article_description" not in html and "rich_related_title" not in html


# tailState: a cover ends in media when its last non-empty block does. Each case puts the
# block after a plain photo, so an Empty tail falls through to the photo (Media).

MEDIA_PHOTO = block(K.Photo, photo_id=1)
CAPTION = RichCaption(plain("c"))


def _list(*items):
    return block(K.List, list_items=list(items))


TAIL_CASES = [
    (block(K.Anchor, name="a"), True),
    (block(K.Paragraph, text=plain("p")), False),
    (block(K.Divider), False),
    (_list(), True),
    (_list(RichListItem(text=plain("x"))), False),
    (_list(RichListItem(blocks=[MEDIA_PHOTO], content=RichListItemContent.Blocks)), True),
    (_list(RichListItem(blocks=[], content=RichListItemContent.Blocks)), False),
    (block(K.Quote, text=plain("q")), False),
    (block(K.Quote, quote_caption=plain("c"), quote_content=RichQuoteContent.Blocks), False),
    (block(K.Quote, blocks=[MEDIA_PHOTO], quote_content=RichQuoteContent.Blocks), True),
    (block(K.Photo, photo_id=1, caption=CAPTION), False),
    (block(K.Photo, photo_id=1, optional_url="https://example.com"), False),
    (block(K.Video, caption=CAPTION), False),
    (block(K.Audio), True),
    (block(K.File), True),
    (block(K.Map), True),
    (block(K.Cover, blocks=[block(K.Paragraph)]), False),
    (block(K.Embed), True),
    (block(K.Embed, poster_photo_id=1, optional_url="https://example.com"), False),
    (block(K.Embed, caption=CAPTION), False),
    (block(K.EmbedPost), True),
    (block(K.EmbedPost, url="https://example.com"), False),
    (block(K.EmbedPost, caption=CAPTION), False),
    (block(K.EmbedPost, blocks=[MEDIA_PHOTO], url="https://example.com"), True),
    (block(K.Collage, blocks=[MEDIA_PHOTO], caption=CAPTION), False),
    (block(K.Slideshow, blocks=[block(K.Paragraph)]), False),
    (block(K.Channel), True),
    (block(K.Details, blocks=[MEDIA_PHOTO], open=True), True),
    (block(K.Details, blocks=[MEDIA_PHOTO]), False),
    (block(K.RelatedArticles), True),
    (block(K.RelatedArticles, text=plain("t")), False),
    (block(K.RelatedArticles, related_articles=[RichRelatedArticle(url="https://e.com")]), False),
    (block(K.RelatedArticles, related_articles=[RichRelatedArticle(photo_id=9)]), True),
    (
        block(
            K.RelatedArticles, related_articles=[RichRelatedArticle(photo_id=9, description="d")]
        ),
        False,
    ),
    (
        block(
            K.RelatedArticles, related_articles=[RichRelatedArticle(photo_id=1, description="d")]
        ),
        True,
    ),
]


@pytest.mark.parametrize(("last", "ends_in_media"), TAIL_CASES)
def test_cover_tail_state(last, ends_in_media):
    cover = block(K.Cover, blocks=[MEDIA_PHOTO, last])
    html = render([cover], photos={1: photo_with_file(1)})
    flag = "true" if ends_in_media else "false"
    assert html.startswith(
        OPEN + '\n <section class="rich_block rich_media_group" data-rich-has-items="true"'
        f' data-rich-items-end-media="{flag}" data-rich-kind="cover">\n'
    )
