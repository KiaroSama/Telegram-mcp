"""Message media blocks: the generic card, photos, stickers, GIFs, video files and rich media.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
CalculateThumbSize, PrepareAudioMediaData, PrepareFileMediaData, HtmlWriter::Wrap::
prepareMediaData/pushGenericMedia/pushStickerMedia/pushAnimatedMedia/pushVideoFileMedia/
pushPhotoMedia/pushRichPhotoMedia/pushRichVideoMedia/pushRichAudioMedia/pushRichFileMedia/
pushRichReferenceMedia), GPL-3.0.
"""

from __future__ import annotations

from typing import Callable

from .files import write_image_thumb, write_image_thumb_sized
from .html_rich_support import (
    MediaData,
    no_file_description,
    rich_document_presentation,
    rich_photo_presentation,
)
from .html_safe import is_global_link
from .html_text import PeersMap, WrapBase, serialize_string
from .model import Document, Photo
from .model_actions import ActionPhoneCall
from .model_format import (
    format_duration,
    format_file_size,
    format_image_size_text,
    format_money_amount,
    format_phone_number,
    number_to_string,
)
from .model_media import Game, GeoPoint, Invoice, PaidMedia, SharedContact, Venue
from .model_message import Message

PHOTO_MAX_WIDTH = PHOTO_MAX_HEIGHT = 520
PHOTO_MIN_WIDTH = PHOTO_MIN_HEIGHT = 80
STICKER_MAX_WIDTH = STICKER_MAX_HEIGHT = 384
STICKER_MIN_WIDTH = STICKER_MIN_HEIGHT = 80
ENTRY_USERPIC_SIZE = 48

Size = tuple[int, int]


def _scaled_keep_aspect(size: Size, width: int, height: int) -> Size:
    """QSize::scaled(width, height, Qt::KeepAspectRatio)."""
    wd, ht = size
    if wd == 0 or ht == 0:
        return (width, height)
    rw = height * wd // ht
    if rw <= width:
        return (rw, height)
    return (width, width * ht // wd)


def calculate_thumb_size(
    max_width: int,
    max_height: int,
    min_width: int,
    min_height: int,
    expand_for_retina: bool = False,
) -> Callable[[Size], Size]:
    def convert(large: Size) -> Size:
        multiplier = 2 if expand_for_retina else 1
        check_width, check_height = large[0] * multiplier, large[1] * multiplier
        small = (
            _scaled_keep_aspect(large, max_width, max_height)
            if check_width > max_width or check_height > max_height
            else large
        )
        retina = (small[0] & ~0x01, small[1] & ~0x01)
        if retina[0] < PHOTO_MIN_WIDTH or retina[1] < PHOTO_MIN_HEIGHT:
            return (0, 0)
        return retina

    return convert


def _image_readable(path: str) -> bool:
    """QImageReader(path).canRead() && !read().isNull(), Pillow standing in for Qt."""
    try:
        from PIL import Image as PilImage
    except ImportError:
        return False
    try:
        with PilImage.open(path) as reader:
            reader.load()
    except (OSError, ValueError):
        return False
    return True


def prepare_audio_media_data(data: Document) -> MediaData:
    has_file = bool(data.file.relative_path)
    result = MediaData()
    if data.song_performer and data.song_title:
        result.title = data.song_performer + " \u2013 " + data.song_title
    else:
        result.title = data.name or "Audio file"
    result.status = format_duration(data.duration)
    if not has_file:
        result.status += ", " + format_file_size(data.file.size)
    result.classes = "media_audio_file"
    result.link = data.file.relative_path
    result.description = no_file_description(data.file.skip_reason)
    return result


def prepare_file_media_data(data: Document) -> MediaData:
    return MediaData(
        title=data.name or "File",
        status=format_file_size(data.file.size),
        classes="media_file",
        link=data.file.relative_path,
        description=no_file_description(data.file.skip_reason),
    )


def _call_status(message: Message, call: ActionPhoneCall) -> str:
    state = ActionPhoneCall.State
    if call.state == state.Invitation:
        return "Invitation"
    if call.state == state.Active:
        return "Ongoing"
    if message.out:
        return "Cancelled" if call.state == state.Missed else "Outgoing"
    if call.state == state.Missed:
        return "Missed"
    if call.state == state.Busy:
        return "Declined"
    return "Incoming"


def _maps_link(latitude: str, longitude: str) -> str:
    coords = latitude + "," + longitude
    return "https://maps.google.com/maps?q=" + coords + "&ll=" + coords + "&z=16"


def _prepare_document(message: Message, data: Document, result: MediaData) -> MediaData:
    if message.media.ttl:
        result.title = "Self-destructing video"
        result.status = "Please view it on your mobile" if data.id else "Expired"
        result.classes = "media_video"
        return result
    has_file = bool(data.file.relative_path)
    result.link = data.file.relative_path
    result.description = no_file_description(data.file.skip_reason)
    if data.is_sticker:
        pass
    elif data.is_video_message or data.is_voice_message:
        result.title = "Video message" if data.is_video_message else "Voice message"
        result.status = format_duration(data.duration)
        if not has_file:
            result.status += ", " + format_file_size(data.file.size)
        if data.is_video_message:
            result.thumb = data.thumb.file.relative_path
            result.classes = "media_video"
        else:
            result.classes = "media_voice_message"
    elif data.is_animated or data.is_video_file:
        pass
    elif data.is_audio_file:
        return prepare_audio_media_data(data)
    else:
        return prepare_file_media_data(data)
    return result


def prepare_media_data(message: Message, peers: PeersMap, internal_links_domain: str) -> MediaData:
    result = MediaData()
    call = message.action.content
    if isinstance(call, ActionPhoneCall):
        result.classes = "media_call"
        result.title = peers.peer(message.peer_id if message.out else message.self_id).name()
        result.status = _call_status(message, call)
        if call.duration > 0:
            result.classes += " success"
            result.status += " (" + number_to_string(call.duration) + " seconds)"
        return result
    content = message.media.content
    if isinstance(content, Photo):
        if message.media.ttl:
            result.title = "Self-destructing photo"
            result.status = "Please view it on your mobile" if content.id else "Expired"
            result.classes = "media_photo"
    elif isinstance(content, Document):
        return _prepare_document(message, content, result)
    elif isinstance(content, SharedContact):
        result.title = content.info.first_name + " " + content.info.last_name
        result.classes = "media_contact"
        result.status = format_phone_number(content.info.phone_number)
        if content.vcard.content:
            result.status += " - vCard"
            result.link = content.vcard.relative_path
    elif isinstance(content, GeoPoint):
        if message.media.ttl:
            result.classes, result.title = "media_live_location", "Live location"
        else:
            result.classes, result.title = "media_location", "Location"
        if content.valid:
            latitude = number_to_string(float(content.latitude))
            longitude = number_to_string(float(content.longitude))
            result.status = latitude + ", " + longitude
            result.link = _maps_link(latitude, longitude)
    elif isinstance(content, Venue):
        result.classes, result.title, result.description = (
            "media_venue",
            content.title,
            content.address,
        )
        if content.point.valid:
            result.link = _maps_link(
                number_to_string(float(content.point.latitude)),
                number_to_string(float(content.point.longitude)),
            )
    elif isinstance(content, Game):
        result.classes, result.title = "media_game", content.title
        result.description = content.description
        if content.bot_id != 0 and content.short_name:
            bot = peers.user(content.bot_id)
            if bot.is_bot and bot.username:
                link = internal_links_domain + bot.username + "?game=" + content.short_name
                result.link = result.status = link
    elif isinstance(content, Invoice):
        result.classes, result.title = "media_invoice", content.title
        result.description = content.description
        result.status = format_money_amount(content.amount, content.currency)
    elif isinstance(content, PaidMedia):
        result.classes = "media_invoice"
        result.status = format_money_amount(content.stars, "XTR")
    return result


class MediaMixin(WrapBase):
    def push_generic_media(self, data: MediaData) -> str:
        result = self.push_div("media_wrap clearfix")
        if not data.link:
            result += self.push_div("media clearfix pull_left " + data.classes)
        else:
            href = data.link if is_global_link(data.link) else self.relative_path(data.link)
            result += self.push_tag(
                "a", {"class": "media clearfix pull_left block_link " + data.classes, "href": href}
            )
        if not data.thumb:
            result += self.push_div("fill pull_left") + self.pop_tag()
        else:
            result += self.push_tag(
                "img",
                {"class": "thumb pull_left", "src": self.relative_path(data.thumb), "empty": ""},
            )
        result += self.push_div("body")
        for value, class_name in (
            (data.title, "title bold"),
            (data.description, "description"),
            (data.status, "status details"),
        ):
            if value:
                result += self.push_div(class_name) + serialize_string(value) + self.pop_tag()
        return result + self.pop_tag() + self.pop_tag() + self.pop_tag()

    def _open_sized(self, wrap_class: str, href: str) -> str:
        result = self.push_div("media_wrap clearfix")
        return result + self.push_tag("a", {"class": wrap_class, "href": self.relative_path(href)})

    def _image(self, class_name: str, size: Size, source: str) -> str:
        style = (
            "width: "
            + number_to_string(size[0] // 2)
            + "px; height: "
            + number_to_string(size[1] // 2)
            + "px"
        )
        return self.push_tag(
            "img",
            {"class": class_name, "style": style, "src": self.relative_path(source), "empty": ""},
        )

    def push_sticker_media(self, data: Document, base_path: str) -> str:
        thumb, size = write_image_thumb(
            base_path,
            data.file.relative_path,
            calculate_thumb_size(
                STICKER_MAX_WIDTH, STICKER_MAX_HEIGHT, STICKER_MIN_WIDTH, STICKER_MIN_HEIGHT
            ),
            "PNG",
            -1,
        )
        if not thumb:
            generic = MediaData(title="Sticker", status=data.sticker_emoji)
            if not data.file.relative_path:
                if generic.status:
                    generic.status += ", "
                generic.status += format_file_size(data.file.size)
            else:
                generic.link = data.file.relative_path
            generic.description = no_file_description(data.file.skip_reason)
            generic.classes = "media_photo"
            return self.push_generic_media(generic)
        result = self._open_sized("sticker_wrap clearfix pull_left", data.file.relative_path)
        result += self._image("sticker", size, thumb)
        return result + self.pop_tag() + self.pop_tag()

    def _video_thumb_size(self, data: Document) -> Size:
        return calculate_thumb_size(
            PHOTO_MAX_WIDTH, PHOTO_MAX_HEIGHT, PHOTO_MIN_WIDTH, PHOTO_MIN_HEIGHT, True
        )((data.width, data.height))

    def push_animated_media(self, data: Document, base_path: str) -> str:
        thumb_size = self._video_thumb_size(data)
        if (
            not data.thumb.file.relative_path
            or not data.file.relative_path
            or not thumb_size[0]
            or not thumb_size[1]
        ):
            return self.push_generic_media(
                MediaData(
                    title="Animation",
                    status=format_file_size(data.file.size),
                    link=data.file.relative_path,
                    description=no_file_description(data.file.skip_reason),
                    classes="media_video",
                )
            )
        result = self._open_sized("animated_wrap clearfix pull_left", data.file.relative_path)
        result += self.push_div("video_play_bg") + self.push_div("gif_play") + "GIF"
        result += self.pop_tag() + self.pop_tag()
        result += self._image("animated", thumb_size, data.thumb.file.relative_path)
        return result + self.pop_tag() + self.pop_tag()

    def push_video_file_media(self, data: Document, base_path: str) -> str:
        thumb_size = self._video_thumb_size(data)
        if (
            not data.thumb.file.relative_path
            or not data.file.relative_path
            or not thumb_size[0]
            or not thumb_size[1]
        ):
            generic = MediaData(title="Video file", status=format_duration(data.duration))
            if not data.file.relative_path:
                generic.status += ", " + format_file_size(data.file.size)
            else:
                generic.link = data.file.relative_path
            generic.description = no_file_description(data.file.skip_reason)
            generic.classes = "media_video"
            return self.push_generic_media(generic)
        result = self._open_sized("video_file_wrap clearfix pull_left", data.file.relative_path)
        result += self.push_div("video_play_bg") + self.push_div("video_play")
        result += self.pop_tag() + self.pop_tag()
        result += self.push_div("video_duration") + format_duration(data.duration) + self.pop_tag()
        result += self._image("video_file", thumb_size, data.thumb.file.relative_path)
        return result + self.pop_tag() + self.pop_tag()

    def push_photo_media(self, data: Photo, base_path: str) -> str:
        thumb, size = write_image_thumb(
            base_path,
            data.image.file.relative_path,
            calculate_thumb_size(
                PHOTO_MAX_WIDTH, PHOTO_MAX_HEIGHT, PHOTO_MIN_WIDTH, PHOTO_MIN_HEIGHT
            ),
        )
        if not thumb:
            image = data.image
            generic = MediaData(
                title="Photo", status=format_image_size_text(image.width, image.height)
            )
            if not image.file.relative_path:
                generic.status += ", " + format_file_size(image.file.size)
            else:
                generic.link = image.file.relative_path
            generic.description = no_file_description(image.file.skip_reason)
            generic.classes = "media_photo"
            return self.push_generic_media(generic)
        result = self._open_sized("photo_wrap clearfix pull_left", data.image.file.relative_path)
        result += self._image("photo", size, thumb)
        return result + self.pop_tag() + self.pop_tag()

    def push_rich_photo_media(self, data: Photo | None, base_path: str) -> str:
        return self.push_photo_media(rich_photo_presentation(data), base_path)

    def push_rich_video_media(self, data: Document | None, base_path: str) -> str:
        presentation = rich_document_presentation(data)
        thumb_path = presentation.thumb.file.relative_path
        if thumb_path and not _image_readable(base_path + thumb_path):
            presentation.thumb.file.relative_path = ""
        return self.push_video_file_media(presentation, base_path)

    def push_rich_audio_media(self, data: Document | None) -> str:
        return self.push_generic_media(prepare_audio_media_data(rich_document_presentation(data)))

    def push_rich_file_media(self, data: Document | None) -> str:
        return self.push_generic_media(prepare_file_media_data(rich_document_presentation(data)))

    def push_rich_reference_media(
        self, data: MediaData, photo: Photo | None, base_path: str
    ) -> str:
        path = rich_photo_presentation(photo).image.file.relative_path
        data.link = path
        data.thumb = (
            write_image_thumb_sized(
                base_path,
                path,
                ENTRY_USERPIC_SIZE * 2,
                ENTRY_USERPIC_SIZE * 2,
                "_rich_card_thumb",
            )
            if path
            else ""
        )
        return self.push_generic_media(data)
