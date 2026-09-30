"""tdesktop HTML export: media cards (MediaData), thumbnails, rich media callbacks, giveaways.

Expected values are derived by hand from Telegram Desktop v7.2.10's export_output_html.cpp
(CalculateThumbSize, PrepareAudioMediaData, PrepareFileMediaData, Wrap::prepareMediaData,
pushGenericMedia, pushStickerMedia, pushAnimatedMedia, pushVideoFileMedia, pushPhotoMedia,
pushRichVideoMedia/AudioMedia/FileMedia/ReferenceMedia and pushGiveaway). Synthetic data only;
images are generated in tmp_path.
"""

from datetime import timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.html_media import calculate_thumb_size, prepare_media_data
from telegram_mcp.tdexport.html_media_extra import _participants
from telegram_mcp.tdexport.html_message import MessageMixin
from telegram_mcp.tdexport.html_rich_support import MediaData
from telegram_mcp.tdexport.html_text import PeersMap
from telegram_mcp.tdexport.model import (
    Document,
    File,
    Image,
    Photo,
    peer_from_channel,
    peer_from_user,
)
from telegram_mcp.tdexport.model_actions import ActionPhoneCall
from telegram_mcp.tdexport.model_media import (
    Game,
    GeoPoint,
    GiveawayResults,
    GiveawayStart,
    Invoice,
    Media,
    PaidMedia,
    SharedContact,
    Venue,
)
from telegram_mcp.tdexport.model_message import Message
from telegram_mcp.tdexport.model_peers import Chat, ContactInfo, Peer, User

ANN = peer_from_user(5)
SELF = peer_from_user(9)
UNAVAILABLE = "Unavailable, please try again later."
State = ActionPhoneCall.State


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def user(bare_id, first, last="", **extra):
    return Peer(User(bare_id=bare_id, info=ContactInfo(bare_id, first, last), **extra))


def peers():
    return PeersMap(
        {
            ANN: user(5, "Ann", "Lee"),
            SELF: user(9, "Me", is_self=True),
            peer_from_user(7): user(7, "Bot", username="robot", is_bot=True),
            peer_from_user(8): user(8, "Human", username="human"),
            peer_from_channel(3): Peer(Chat(bare_id=3, title="News", is_broadcast=True)),
            peer_from_channel(4): Peer(Chat(bare_id=4, title="Club", is_supergroup=True)),
            peer_from_channel(6): Peer(Chat(bare_id=6, title="Plain")),
        }
    )


def media_of(content, ttl=0):
    return prepare_media_data(Message(id=1, media=Media(content, ttl)), peers(), "https://t.me/")


def _image(path, size, fmt="JPEG"):
    from PIL import Image as PilImage

    path.parent.mkdir(parents=True, exist_ok=True)
    PilImage.new("RGB", size, "blue").save(path, fmt)


# CalculateThumbSize.


@pytest.mark.parametrize(
    ("size", "retina", "expected"),
    [
        ((1040, 780), False, (520, 390)),
        ((300, 200), True, (520, 346)),
        ((81, 81), False, (80, 80)),
        ((79, 200), False, (0, 0)),
        ((0, 100), False, (0, 0)),
        # QSize::scaled of a size with a zero side returns the target size itself.
        ((0, 1000), False, (520, 520)),
    ],
)
def test_calculate_thumb_size(size, retina, expected):
    assert calculate_thumb_size(520, 520, 80, 80, retina)(size) == expected


# prepareMediaData.


def _call(state, out=False, duration=0):
    message = Message(id=1, peer_id=ANN, self_id=SELF, out=out)
    message.action.content = ActionPhoneCall(state=state, duration=duration)
    return prepare_media_data(message, peers(), "")


@pytest.mark.parametrize(
    ("state", "out", "title", "status"),
    [
        (State.Invitation, False, "Me", "Invitation"),
        (State.Active, True, "Ann Lee", "Ongoing"),
        (State.Missed, True, "Ann Lee", "Cancelled"),
        (State.Hangup, True, "Ann Lee", "Outgoing"),
        (State.Missed, False, "Me", "Missed"),
        (State.Busy, False, "Me", "Declined"),
        (State.Hangup, False, "Me", "Incoming"),
    ],
)
def test_call_cards(state, out, title, status):
    assert _call(state, out) == MediaData(title=title, status=status, classes="media_call")


def test_call_with_duration_is_a_success():
    assert _call(State.Hangup, duration=12) == MediaData(
        title="Me", status="Incoming (12 seconds)", classes="media_call success"
    )


def test_self_destructing_media():
    assert media_of(Photo(id=1), ttl=5) == MediaData(
        title="Self-destructing photo",
        status="Please view it on your mobile",
        classes="media_photo",
    )
    assert media_of(Photo(), ttl=5).status == "Expired"
    assert media_of(Photo()) == MediaData()
    assert media_of(Document(id=2), ttl=5) == MediaData(
        title="Self-destructing video",
        status="Please view it on your mobile",
        classes="media_video",
    )
    assert media_of(Document(), ttl=5).status == "Expired"


def test_document_cards():
    video_message = Document(
        is_video_message=True,
        duration=5,
        file=File(size=2048, skip_reason=SkipReason.FileSize),
        thumb=Image(file=File(relative_path="round/t.jpg")),
    )
    assert media_of(video_message) == MediaData(
        title="Video message",
        description="Exceeds maximum size, change data exporting settings to download.",
        status="00:05, 2.0 KB",
        classes="media_video",
        thumb="round/t.jpg",
    )
    voice = Document(is_voice_message=True, duration=65, file=File(relative_path="voice/a.ogg"))
    assert media_of(voice) == MediaData(
        title="Voice message", status="01:05", classes="media_voice_message", link="voice/a.ogg"
    )
    sticker = Document(is_sticker=True, file=File(relative_path="stickers/s.webp"))
    assert media_of(sticker) == MediaData(link="stickers/s.webp")
    gif = Document(is_animated=True, file=File(skip_reason=SkipReason.Unavailable))
    assert media_of(gif) == MediaData(description=UNAVAILABLE)
    song = Document(
        is_audio_file=True, song_performer="P", song_title="T", duration=3, file=File(size=10)
    )
    assert media_of(song) == MediaData(
        title="P \u2013 T", status="00:03, 10 B", classes="media_audio_file"
    )
    named = Document(
        is_audio_file=True, name="a.mp3", song_title="T", file=File(relative_path="a")
    )
    assert media_of(named) == MediaData(
        title="a.mp3", status="00:00", classes="media_audio_file", link="a"
    )
    other = Document(file=File(relative_path="files/x", size=1))
    assert media_of(other) == MediaData(
        title="File", status="1 B", classes="media_file", link="files/x"
    )


def test_contact_location_venue_game_invoice_and_paid_media():
    contact = SharedContact(
        info=ContactInfo(first_name="A", last_name="B", phone_number="123"),
        vcard=File(content=b"v", relative_path="contacts/c.vcf"),
    )
    assert media_of(contact) == MediaData(
        title="A B", status="123 - vCard", classes="media_contact", link="contacts/c.vcf"
    )
    assert media_of(SharedContact()) == MediaData(title=" ", classes="media_contact")
    assert media_of(GeoPoint(), ttl=60) == MediaData(
        title="Live location", classes="media_live_location"
    )
    link = "https://maps.google.com/maps?q=1.500000,2.500000&ll=1.500000,2.500000&z=16"
    venue = Venue(point=GeoPoint(1.5, 2.5, True), title="V", address="Addr")
    assert media_of(venue) == MediaData(
        title="V", description="Addr", classes="media_venue", link=link
    )
    assert media_of(Venue(title="V")) == MediaData(title="V", classes="media_venue")
    game = Game(short_name="g", title="G", description="d", bot_id=7)
    assert media_of(game) == MediaData(
        title="G",
        description="d",
        status="https://t.me/robot?game=g",
        classes="media_game",
        link="https://t.me/robot?game=g",
    )
    # No link without a bot, or when the "bot" is an ordinary user.
    for bot_id in (0, 8):
        plain_game = Game(short_name="g", title="G", bot_id=bot_id)
        assert media_of(plain_game) == MediaData(title="G", classes="media_game")
    invoice = Invoice(title="I", description="d", currency="USD", amount=150)
    assert media_of(invoice) == MediaData(
        title="I", description="d", status="$1.50", classes="media_invoice"
    )
    assert media_of(PaidMedia(stars=5)) == MediaData(status="\u2b505", classes="media_invoice")


# Generic cards and the no-thumbnail fallbacks.


def test_generic_card_with_thumbnail_and_global_link():
    wrap = MessageMixin("../")
    data = MediaData(title="T", thumb="thumbs/t.jpg", link="https://x", classes="media_file")
    assert wrap.push_generic_media(data) == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <a class="media clearfix pull_left block_link media_file" href="https://x">\n'
        '\n  <img class="thumb pull_left" src="../thumbs/t.jpg"/>\n'
        '\n  <div class="body">\n'
        '\n   <div class="title bold">\nT\n   </div>\n'
        "\n  </div>\n"
        "\n </a>\n"
        "\n</div>\n"
    )


def test_sticker_without_thumbnail_is_a_generic_card(tmp_path):
    wrap = MessageMixin("")
    missing = Document(is_sticker=True, sticker_emoji="E", file=File(size=100))
    assert wrap.push_sticker_media(missing, str(tmp_path) + "/") == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <div class="media clearfix pull_left media_photo">\n'
        '\n  <div class="fill pull_left">\n'
        "\n  </div>\n"
        '\n  <div class="body">\n'
        '\n   <div class="title bold">\nSticker\n   </div>\n'
        '\n   <div class="status details">\nE, 100 B\n   </div>\n'
        "\n  </div>\n"
        "\n </div>\n"
        "\n</div>\n"
    )
    absent = Document(is_sticker=True, file=File(relative_path="stickers/gone.webp", size=100))
    html = wrap.push_sticker_media(absent, str(tmp_path) + "/")
    assert 'href="stickers/gone.webp"' in html and "\n100 B\n" not in html


def test_animation_with_a_thumbnail():
    gif = Document(
        is_animated=True,
        width=200,
        height=100,
        file=File(relative_path="video_files/v.mp4"),
        thumb=Image(file=File(relative_path="video_files/v_thumb.jpg")),
    )
    assert MessageMixin("").push_animated_media(gif, "") == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <a class="animated_wrap clearfix pull_left" href="video_files/v.mp4">\n'
        '\n  <div class="video_play_bg">\n'
        '\n   <div class="gif_play">\nGIF\n   </div>\n'
        "\n  </div>\n"
        '\n  <img class="animated" src="video_files/v_thumb.jpg"'
        ' style="width: 100px; height: 50px"/>\n'
        "\n </a>\n"
        "\n</div>\n"
    )


def test_video_and_photo_fallbacks():
    wrap = MessageMixin("")
    linked = Document(duration=5, file=File(relative_path="video_files/v.mp4"))
    html = wrap.push_video_file_media(linked, "")
    assert 'href="video_files/v.mp4"' in html and "\n00:05\n" in html and "Video file" in html
    unlinked = Document(duration=5, file=File(size=10, skip_reason=SkipReason.FileType))
    html = wrap.push_video_file_media(unlinked, "")
    assert "\n00:05, 10 B\n" in html
    assert "Not included, change data exporting settings to download." in html
    photo = Photo(image=Image(3, 4, File(relative_path="photos/missing.jpg")))
    html = wrap.push_photo_media(photo, "")
    assert 'href="photos/missing.jpg"' in html and "\n3\u00d74\n" in html


# Rich media callbacks.


def test_rich_audio_and_file_cards():
    wrap = MessageMixin("")
    assert wrap.push_rich_audio_media(None) == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <div class="media clearfix pull_left media_audio_file">\n'
        '\n  <div class="fill pull_left">\n'
        "\n  </div>\n"
        '\n  <div class="body">\n'
        '\n   <div class="title bold">\nAudio file\n   </div>\n'
        f'\n   <div class="description">\n{UNAVAILABLE}\n   </div>\n'
        '\n   <div class="status details">\n00:00, 0 B\n   </div>\n'
        "\n  </div>\n"
        "\n </div>\n"
        "\n</div>\n"
    )
    big = Document(name="n.pdf", file=File(skip_reason=SkipReason.FileSize, size=2048))
    html = wrap.push_rich_file_media(big)
    assert '\n   <div class="title bold">\nn.pdf\n   </div>\n' in html
    assert "Exceeds maximum size, change data exporting settings to download." in html
    assert '\n   <div class="status details">\n2.0 KB\n   </div>\n' in html


def test_rich_video_drops_an_unreadable_thumbnail(tmp_path):
    pytest.importorskip("PIL")
    base = str(tmp_path).replace("\\", "/") + "/"
    _image(tmp_path / "video_files" / "good.jpg", (64, 36))
    (tmp_path / "video_files" / "bad.jpg").write_bytes(b"not an image")
    wrap = MessageMixin("")

    def video(thumb):
        return Document(
            width=640,
            height=360,
            duration=5,
            file=File(relative_path="video_files/v.mp4"),
            thumb=Image(file=File(relative_path=thumb)),
        )

    html = wrap.push_rich_video_media(video("video_files/good.jpg"), base)
    assert (
        '<img class="video_file" src="video_files/good.jpg" style="width: 260px; height: 146px"/>'
    ) in html
    html = wrap.push_rich_video_media(video("video_files/bad.jpg"), base)
    assert "<img" not in html and "Video file" in html and 'href="video_files/v.mp4"' in html
    # A missing document is shown as an unavailable video.
    assert UNAVAILABLE in wrap.push_rich_video_media(None, base)


def test_rich_reference_card_writes_its_own_thumbnail(tmp_path):
    pytest.importorskip("PIL")
    base = str(tmp_path).replace("\\", "/") + "/"
    _image(tmp_path / "photos" / "p.jpg", (200, 100))
    photo = Photo(image=Image(200, 100, File(relative_path="photos/p.jpg")))
    wrap = MessageMixin("")
    html = wrap.push_rich_reference_media(MediaData(title="R", classes="media_photo"), photo, base)
    assert (tmp_path / "photos" / "p_rich_card_thumb.jpg").exists()
    assert (
        '\n <a class="media clearfix pull_left block_link media_photo" href="photos/p.jpg">\n'
        '\n  <img class="thumb pull_left" src="photos/p_rich_card_thumb.jpg"/>\n'
    ) in html
    html = wrap.push_rich_reference_media(MediaData(title="R", classes="media_photo"), None, base)
    assert '<div class="media clearfix pull_left media_photo">' in html and "<img" not in html


# Giveaways.


@pytest.mark.parametrize(
    ("everyone", "channel", "group", "count", "text"),
    [
        (True, True, False, 2, "All subscribers of the channels:"),
        (True, False, True, 1, "All members of the group:"),
        (True, False, True, 2, "All members of the groups:"),
        (True, True, True, 1, "All members of the group:"),
        (True, True, True, 2, "All members of the groups and channels:"),
        (True, False, False, 1, ""),
        (False, True, False, 1, "All users who joined the channel below after this date:"),
        (False, True, False, 2, "All users who joined the channels below after this date:"),
        (False, False, True, 1, "All users who joined the group below after this date:"),
        (False, False, True, 2, "All users who joined the groups below after this date:"),
        (False, True, True, 1, "All users who joined the group below after this date:"),
        (
            False,
            True,
            True,
            2,
            "All users who joined the groups and channels below after this date:",
        ),
        (False, True, False, 0, ""),
    ],
)
def test_giveaway_participants_sentence(everyone, channel, group, count, text):
    assert _participants(GiveawayStart(all=everyone), channel, group, count) == text


def test_giveaway_start_of_premium_months_to_groups():
    start = GiveawayStart(channels=[4, 6], quantity=3, months=6, additional_prize="Mug")
    html = MessageMixin("").push_giveaway_start(peers(), start)
    assert "\nGiveaway Prizes\n" in html and "\n<b>3</b> Mug\n" in html
    assert "\n<b>3</b> Telegram Premium Subscriptions for <b>6</b> months.\n" in html
    assert "\nAll users who joined the groups below after this date:\n" in html
    assert "from " not in html
    one = GiveawayStart(channels=[3], quantity=1, months=1, credits=0)
    html = MessageMixin("").push_giveaway_start(peers(), one)
    assert "\n<b>1</b> Telegram Premium Subscription for <b>1</b> month.\n" in html
    stars = GiveawayStart(channels=[3], quantity=2, credits=500, countries=["DE"])
    html = MessageMixin("").push_giveaway_start(peers(), stars)
    assert "<b>500 Stars</b> will be distributed among <b>2</b> winners." in html
    assert "\nfrom \U0001f1e9\U0001f1ea\u00a0Germany\n" in html


def _results(**fields):
    def link(message_id, text):
        return f"[{message_id}:{text}]"

    return MessageMixin("").push_giveaway_results(peers(), GiveawayResults(**fields), link)


def test_giveaway_results_prize_lines():
    html = _results(winners=[ANN], winners_count=1, launch_id=4, credits=1)
    assert "\nWinner Selected!\n" in html and "\nWinner\n" in html
    assert "\n<b>1</b> winner of the [4:Giveaway] was randomly selected by Telegram.\n" in html
    assert "\n<b>Ann Lee</b>\n" in html
    assert "\nThe winner received <b>1</b> Star.\n" in html
    html = _results(winners=[ANN, SELF], winners_count=2, credits=100)
    assert "\n<b>Ann Lee</b>, <b>Me</b>\n" in html
    assert "\nAll winners received <b>100</b> Stars in total.\n" in html
    assert "\nSome winners couldn&apos;t be selected.\n" in _results(
        winners_count=2, unclaimed_count=1
    )
    assert "\nThe winner received their gift link in a private message.\n" in _results(
        winners=[ANN], winners_count=1
    )
    empty = _results()
    assert empty.endswith('\n  <div class="section_body">\n\n  </div>\n\n </div>\n\n</div>\n')
