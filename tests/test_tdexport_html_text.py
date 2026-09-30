"""tdesktop HTML export: escaping, text entities, link sanitising, dates, tags and userpics
(spec 030, stream C). Expected strings are derived by hand from export_output_html.cpp and
Qt 5.15's QUrl. Synthetic data only.
"""

from datetime import timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.html_safe import (
    anchor_token,
    is_bot_command_action,
    is_cashtag_action,
    is_hashtag_action,
    safe_action_data,
    safe_email_href,
    safe_http_href,
    safe_mention_href,
    safe_message_href,
    safe_phone_href,
    safe_relative_emoji_href,
)
from telegram_mcp.tdexport.html_text import (
    HtmlContext,
    PeersMap,
    UserpicData,
    WrapBase,
    byte_mid,
    display_date,
    fill_userpic_names_from_full,
    format_date_text,
    format_text,
    format_time_text,
    iso_date_time,
    serialize_list,
    serialize_string,
)
from telegram_mcp.tdexport.model import TextPart, peer_from_user
from telegram_mcp.tdexport.model_peers import ContactInfo, Peer, User

T = TextPart.Type


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def test_serialize_string_escapes_like_tdesktop():
    source = "a\n\"&'<>\t\x01\x1f\u2028\u2029b\u00e9"
    assert serialize_string(source) == (
        "a<br>&quot;&amp;&apos;&lt;&gt;&#x09;&#x01;&#x1F;<br><br>b\u00e9"
    )


def test_serialize_list_joins_with_and():
    assert serialize_list([]) == ""
    assert serialize_list(["a"]) == "a"
    assert serialize_list(["a", "b"]) == "a and b"
    assert serialize_list(["a", "b", "c"]) == "a, b and c"


def test_byte_mid_cuts_utf8_bytes():
    assert byte_mid("@user", 1) == "user"
    cut = byte_mid("\u00e9x", 1)
    assert cut.encode("utf-8", "surrogateescape") == b"\xa9x"


def test_context_indents_block_tags_and_sorts_attributes():
    context = HtmlContext()
    assert context.push_tag("div", {"id": "m1", "class": "a"}) == '\n<div class="a" id="m1">\n'
    assert context.push_tag("div", {"class": "b"}) == '\n <div class="b">\n'
    assert context.push_tag("span", {"inline": "", "title": 'x"y'}) == '<span title="x&quot;y">'
    assert context.push_tag("img", {"src": "p.jpg", "class": "c", "empty": ""}) == (
        '\n   <img class="c" src="p.jpg"/>\n'
    )
    assert context.pop_tag() == "</span>"
    assert context.pop_tag() == "\n </div>\n"
    assert context.pop_tag() == "\n</div>\n"
    assert context.empty()


def test_format_text_entities():
    parts = [
        TextPart(T.Text, "hi & bye "),
        TextPart(T.Mention, "@user"),
        TextPart(T.Hashtag, "#tag"),
        TextPart(T.BotCommand, "/start"),
        TextPart(T.Cashtag, "$USD"),
        TextPart(T.Bold, "b"),
        TextPart(T.Italic, "i"),
        TextPart(T.Code, "c"),
        TextPart(T.Pre, "p", "python"),
        TextPart(T.Underline, "u"),
        TextPart(T.Strike, "s"),
        TextPart(T.Blockquote, "q"),
        TextPart(T.BankCard, "1234"),
        TextPart(T.Email, "a@b.c"),
        TextPart(T.Phone, "+123"),
        TextPart(T.MentionName, "Ann", "42"),
    ]
    assert format_text(parts, "https://t.me/", "") == (
        "hi &amp; bye "
        '<a href="https://t.me/user">@user</a>'
        '<a href="" onclick="return ShowHashtag(&quot;tag&quot;)">#tag</a>'
        '<a href="" onclick="return ShowBotCommand(&quot;start&quot;)">/start</a>'
        '<a href="" onclick="return ShowCashtag(&quot;USD&quot;)">$USD</a>'
        "<strong>b</strong><em>i</em><code>c</code><pre>p</pre><u>u</u><s>s</s>"
        "<blockquote>q</blockquote>1234"
        '<a href="mailto:a@b.c">a@b.c</a>'
        '<a href="tel:+123">+123</a>'
        '<a href="" onclick="return ShowMentionName()">Ann</a>'
    )


def test_format_text_links_spoilers_and_custom_emoji():
    parts = [
        TextPart(T.Url, "example.com/a?b=1&c=2"),
        TextPart(T.TextUrl, "bad", "javascript:alert(1)"),
        TextPart(T.TextUrl, "ok", "HTTPS://Example.COM/Path"),
        TextPart(T.Spoiler, "hidden"),
        TextPart(T.CustomEmoji, "\U0001f600", ""),
        TextPart(T.CustomEmoji, "\U0001f600", "(unavailable)"),
        TextPart(T.CustomEmoji, "\U0001f600", "stickers/e.webp"),
    ]
    assert format_text(parts, "https://t.me/", "../") == (
        '<a href="https://example.com/a?b=1&amp;c=2">example.com/a?b=1&amp;c=2</a>'
        "bad"
        '<a href="https://example.com/Path">ok</a>'
        '<span class="spoiler hidden" onclick="ShowSpoiler(this)">'
        '<span aria-hidden="true">hidden</span></span>'
        '<a href="" onclick="return ShowNotLoadedEmoji();">\U0001f600</a>'
        '<a href="" onclick="return ShowNotAvailableEmoji();">\U0001f600</a>'
        '<a href = "../stickers/e.webp">\U0001f600</a>'
    )


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("HTTPS://Example.COM/Path", "https://example.com/Path"),
        ("t.me/abc", "https://t.me/abc"),
        ("https://b\u00fccher.de/\u00e4?x=\u00fc#f", "https://xn--bcher-kva.de/%C3%A4?x=%C3%BC#f"),
        ("https://a.com/%7e%3f%d8%a7", "https://a.com/~%3F%D8%A7"),
        ("https://a.com/a%3F?q=%23#%23", "https://a.com/a%3F?q=%23#%23"),
        ("https://user:pw@a.com:8080/", "https://user:pw@a.com:8080/"),
        ("http://127.1/", "http://127.0.0.1/"),
        ("http://[::FFFF:1.2.3.4]/", "http://[::ffff:1.2.3.4]/"),
        ("http://[2001:DB8:0:0:0:0:0:1]/", "http://[2001:db8::1]/"),
        ("mailto:a@b.c", "mailto:a@b.c"),
        ("tg://resolve?domain=x", "tg://resolve?domain=x"),
        ("https://a.com/{x}", None),
        ("https://a b.com", None),
        ("https://a.com/x y", None),
        ("https://a.com:99999/", None),
        ("https://%41.com", None),
        ("https://a.com/%0a", None),
        ("https://-a.com", None),
        ("//evil.com", None),
        ("ftp://a.com", None),
        ("javascript:alert(1)", None),
        ("https://", None),
        (" https://a.com", None),
    ],
)
def test_safe_message_href_follows_qurl(source, expected):
    assert safe_message_href(source) == expected


def test_safe_http_email_phone_and_mention_hrefs():
    assert safe_http_href("a.com") is None
    assert safe_http_href("http://a.com") == "http://a.com"
    assert safe_http_href("mailto:a@b.c") is None
    assert safe_email_href("a@b.c") == "mailto:a@b.c"
    assert safe_email_href("a@@b.c") is None
    assert safe_email_href("a b@c.d") is None
    assert safe_email_href("a@b.c?x") is None
    assert safe_phone_href("+1 (234) 5-6.7") == "tel:+1 (234) 5-6.7"
    assert safe_phone_href("+()") is None
    assert safe_phone_href("12a") is None
    assert safe_mention_href("@user_1", "https://t.me/") == "https://t.me/user_1"
    assert safe_mention_href("@us-er", "https://t.me/") is None


def test_rich_action_values():
    assert is_hashtag_action("tag_1\u0301") and is_hashtag_action("tag@bot")
    assert not is_hashtag_action("123") and not is_hashtag_action("a")
    assert not is_hashtag_action("tag@a@b")
    assert is_bot_command_action("start@botname") and not is_bot_command_action("start@bot")
    assert is_cashtag_action("USD") and not is_cashtag_action("usd")
    assert safe_action_data("#tag", "#") == "tag"
    assert safe_action_data("#", "#") is None
    assert safe_action_data("/x", "#") is None


def test_relative_emoji_href_and_anchor_token():
    assert safe_relative_emoji_href("stickers/a.webp", "../") == "../stickers/a.webp"
    for bad in ("/a", "a/../b", "a\\b", "a:b", "(unavailable)", "123", "a//b"):
        assert safe_relative_emoji_href(bad, "") is None
    assert anchor_token("") == "empty"
    assert anchor_token("a") == "YQ"
    long_name = "x" * 80
    token = anchor_token(long_name)
    assert token.startswith("eHh4") and token.count("-") == 1 and len(token.split("-")[1]) == 16


def test_dates_use_the_local_zone():
    assert format_date_text(0) == "1 January 1970"
    assert format_time_text(13 * 3600 + 5 * 60) == "13:05"
    assert iso_date_time(86400 + 1) == "1970-01-02T00:00:01"
    assert display_date(100, 0)
    assert not display_date(100, 200)
    assert display_date(86400, 200)


def test_push_userpic_initials_and_images():
    wrap = WrapBase("../")
    userpic = UserpicData(color_index=2, pixel_size=42, first_name=" Ann", last_name="Lee")
    assert wrap.push_userpic(userpic) == (
        '\n<div class="userpic userpic3" style="width: 42px; height: 42px">\n'
        '\n <div class="initials" style="line-height: 42px">\n'
        "AL"
        "\n </div>\n"
        "\n</div>\n"
    )
    emoji = UserpicData(pixel_size=20, first_name="\U0001f600x", tooltip="T")
    assert wrap.push_userpic(emoji) == (
        '\n<div class="userpic userpic1" style="width: 20px; height: 20px">\n'
        '\n <div class="initials" style="line-height: 20px" title="T">\n'
        "?"
        "\n </div>\n"
        "\n</div>\n"
    )
    photo = UserpicData(pixel_size=60, image_link="p_thumb.jpg", large_link="p.jpg")
    assert wrap.push_userpic(photo) == (
        '\n<a class="userpic_link" href="../p.jpg">\n'
        '\n <img class="userpic" src="../p_thumb.jpg" style="width: 60px; height: 60px"/>\n'
        "\n</a>\n"
    )


def test_peers_map_names():
    ann = User(bare_id=5, info=ContactInfo(user_id=5, first_name="A<nn"))
    peers = PeersMap({peer_from_user(5): Peer(ann)})
    assert peers.wrap_peer_name(peer_from_user(5)) == "A&lt;nn"
    assert peers.wrap_peer_name(peer_from_user(6)) == "Deleted"
    assert peers.wrap_user_names([5, 6, 7]) == "A&lt;nn, Deleted Account and Deleted Account"
    names = UserpicData()
    fill_userpic_names_from_full(names, "First  Middle Last")
    assert (names.first_name, names.last_name) == ("First", "Middle Last")
