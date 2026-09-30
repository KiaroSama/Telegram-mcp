"""Core export data types: peer ids, text, files, photos, documents, users and chats.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.{h,cpp},
data/data_peer_id.{h,cpp}, core/credits_amount.h, data/data_birthday.cpp), GPL-3.0.

Naming: C++ types keep their names, members and functions become snake_case
(`forwardedFromId` -> `forwarded_from_id`, `ParseDocument` -> `parse_document`). A PeerId is a
plain int encoded exactly like tdesktop's (bare id | type << 48). Utf8String and QString become
`str`; QByteArray stays `bytes` only where the content is binary.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .files import SkipReason, file_name_from_user_string, mime_first_glob
from .model_format import format_date_time, number_to_string

MIGRATED_MESSAGES_ID_SHIFT = -1_000_000_000
CHAT_TYPE_MASK = 0xFFFFFFFFFFFF
_USER_SHIFT, _CHAT_SHIFT, _CHANNEL_SHIFT = 0, 1, 2
REPLIES_USER_ID = 1271266957
VERIFY_CODES_USER_ID = 489000


def peer_from_user(user_id: int) -> int:
    return user_id | (_USER_SHIFT << 48)


def peer_from_chat(chat_id: int) -> int:
    return chat_id | (_CHAT_SHIFT << 48)


def peer_from_channel(channel_id: int) -> int:
    return channel_id | (_CHANNEL_SHIFT << 48)


def _peer_is(peer_id: int, shift: int) -> bool:
    return ((peer_id >> 48) & 0xFF) == shift


def peer_is_user(peer_id: int) -> bool:
    return _peer_is(peer_id, _USER_SHIFT)


def peer_is_chat(peer_id: int) -> bool:
    return _peer_is(peer_id, _CHAT_SHIFT)


def peer_is_channel(peer_id: int) -> bool:
    return _peer_is(peer_id, _CHANNEL_SHIFT)


def peer_to_user(peer_id: int) -> int:
    return peer_id & CHAT_TYPE_MASK if peer_is_user(peer_id) else 0


def peer_to_chat(peer_id: int) -> int:
    return peer_id & CHAT_TYPE_MASK if peer_is_chat(peer_id) else 0


def peer_to_channel(peer_id: int) -> int:
    return peer_id & CHAT_TYPE_MASK if peer_is_channel(peer_id) else 0


def peer_to_bare_id(peer_id: int) -> int:
    """Data::PeerToBareId."""
    return peer_id & CHAT_TYPE_MASK


def parse_peer_id(peer: Any) -> int:
    """peerFromMTP."""
    if isinstance(peer, tl.PeerUser):
        return peer_from_user(peer.user_id)
    if isinstance(peer, tl.PeerChat):
        return peer_from_chat(peer.chat_id)
    if isinstance(peer, tl.PeerChannel):
        return peer_from_channel(peer.channel_id)
    return 0


def peer_color_index(bare_id: int) -> int:
    """Data::PeerColorIndex(BareId)."""
    return (0, 7, 4, 1, 6, 3, 5)[bare_id % 7]


def peer_id_color_index(peer_id: int) -> int:
    """Data::PeerColorIndex(PeerId)."""
    return peer_color_index(peer_to_bare_id(peer_id))


def string_bare_peer_id(data: str) -> int:
    result = 0xFF
    for byte in data.encode("utf-8"):
        # `char` is signed in tdesktop's loop.
        result = (result * 239 + (byte - 256 if byte > 127 else byte)) & 0xFF
    return result


_OFFICIAL_APPLICATION_COLORS = {1: 0, 7: 0, 6: 1, 21724: 1, 2834: 2, 2496: 3, 2040: 4, 1429: 5}


def application_color_index(application_id: int) -> int:
    if application_id in _OFFICIAL_APPLICATION_COLORS:
        return _OFFICIAL_APPLICATION_COLORS[application_id]
    return peer_color_index(application_id)


def domain_application_id(data: str) -> int:
    return 0x1000 + string_bare_peer_id(data)


def to_time(value: Any) -> int:
    """Telethon turns TL `date` fields into datetime; tdesktop keeps the unix TimeId."""
    if value is None:
        return 0
    if isinstance(value, datetime):
        return int(value.timestamp())
    return int(value)


class CreditsType(Enum):
    Stars = 0
    Ton = 1


_ONE_STAR_IN_NANO = 1_000_000_000


@dataclass
class CreditsAmount:
    """core/credits_amount.h CreditsAmount."""

    _whole: int = 0
    _nano: int = 0
    _ton: bool = False

    @staticmethod
    def make(whole: int, nano: int = 0, kind: CreditsType = CreditsType.Stars) -> CreditsAmount:
        result = CreditsAmount(whole, nano, kind == CreditsType.Ton)
        result._normalize()
        return result

    def _normalize(self) -> None:
        if self._nano < 0:
            shifts = (-self._nano + _ONE_STAR_IN_NANO - 1) // _ONE_STAR_IN_NANO
            self._nano += shifts * _ONE_STAR_IN_NANO
            self._whole -= shifts
        elif self._nano >= _ONE_STAR_IN_NANO:
            shifts = self._nano // _ONE_STAR_IN_NANO
            self._nano -= shifts * _ONE_STAR_IN_NANO
            self._whole += shifts

    def whole(self) -> int:
        return self._whole

    def nano(self) -> int:
        return self._nano

    def value(self) -> float:
        return float(self._whole) + float(self._nano) / _ONE_STAR_IN_NANO

    def ton(self) -> bool:
        return self._ton

    def stars(self) -> bool:
        return not self._ton

    def type(self) -> CreditsType:
        return CreditsType.Ton if self._ton else CreditsType.Stars

    def empty(self) -> bool:
        return not self._whole and not self._nano

    def __bool__(self) -> bool:
        return not self.empty()

    def __sub__(self, other: CreditsAmount) -> CreditsAmount:
        result = CreditsAmount(self._whole - other._whole, self._nano - other._nano, self._ton)
        result._normalize()
        return result


def credits_amount_from_tl(amount: Any) -> CreditsAmount:
    """CreditsAmountFromTL (a missing amount gives an empty one)."""
    if isinstance(amount, tl.StarsAmount):
        return CreditsAmount.make(amount.amount, amount.nanos, CreditsType.Stars)
    if isinstance(amount, tl.StarsTonAmount):
        negative = amount.amount < 0
        absolute = -amount.amount if negative else amount.amount
        result = CreditsAmount.make(
            absolute // _ONE_STAR_IN_NANO, absolute % _ONE_STAR_IN_NANO, CreditsType.Ton
        )
        return CreditsAmount.make(0, 0, CreditsType.Ton) - result if negative else result
    return CreditsAmount()


@dataclass
class Birthday:
    """data/data_birthday Birthday: day + month * 100 + year * 10000, 0 when invalid."""

    _value: int = 0

    YEAR_MIN = 1875
    YEAR_MAX = 2100

    @staticmethod
    def make(day: int, month: int, year: int = 0) -> Birthday:
        valid = Birthday._validate(day, month, year)
        return Birthday(day + month * 100 + year * 10000 if valid else 0)

    @staticmethod
    def _validate(day: int, month: int, year: int) -> bool:
        if year != 0 and not (Birthday.YEAR_MIN <= year <= Birthday.YEAR_MAX):
            return False
        if day < 1:
            return False
        if month == 2:
            if day == 29:
                return not year or (year % 4 == 0 and (year % 100 != 0 or year % 400 == 0))
            return day <= 28
        if month in (4, 6, 9, 11):
            return day <= 30
        return 0 < month <= 12 and day <= 31

    def serialize(self) -> int:
        return self._value

    def valid(self) -> bool:
        return self._value != 0

    def day(self) -> int:
        return self._value % 100

    def month(self) -> int:
        return (self._value // 100) % 100

    def year(self) -> int:
        return self._value // 10000


@dataclass
class TextPart:
    class Type(Enum):
        Text = 0
        Unknown = 1
        Mention = 2
        Hashtag = 3
        BotCommand = 4
        Url = 5
        Email = 6
        Bold = 7
        Italic = 8
        Code = 9
        Pre = 10
        TextUrl = 11
        MentionName = 12
        Phone = 13
        Cashtag = 14
        Underline = 15
        Strike = 16
        Blockquote = 17
        BankCard = 18
        Spoiler = 19
        CustomEmoji = 20

    type: TextPart.Type = Type.Text
    text: str = ""
    additional: str = ""

    @staticmethod
    def unavailable_emoji() -> str:
        return "(unavailable)"


_T = TextPart.Type
_ENTITY_TYPES: dict[type, TextPart.Type] = {
    tl.MessageEntityUnknown: _T.Unknown,
    tl.MessageEntityMention: _T.Mention,
    tl.MessageEntityHashtag: _T.Hashtag,
    tl.MessageEntityBotCommand: _T.BotCommand,
    tl.MessageEntityUrl: _T.Url,
    tl.MessageEntityEmail: _T.Email,
    tl.MessageEntityBold: _T.Bold,
    tl.MessageEntityItalic: _T.Italic,
    tl.MessageEntityCode: _T.Code,
    tl.MessageEntityPre: _T.Pre,
    tl.MessageEntityTextUrl: _T.TextUrl,
    tl.MessageEntityMentionName: _T.MentionName,
    tl.InputMessageEntityMentionName: _T.MentionName,
    tl.MessageEntityPhone: _T.Phone,
    tl.MessageEntityCashtag: _T.Cashtag,
    tl.MessageEntityUnderline: _T.Underline,
    tl.MessageEntityStrike: _T.Strike,
    tl.MessageEntityBlockquote: _T.Blockquote,
    tl.MessageEntityBankCard: _T.BankCard,
    tl.MessageEntitySpoiler: _T.Spoiler,
    tl.MessageEntityCustomEmoji: _T.CustomEmoji,
    tl.MessageEntityFormattedDate: _T.Unknown,
    tl.MessageEntityDiffInsert: _T.Unknown,
    tl.MessageEntityDiffReplace: _T.Unknown,
    tl.MessageEntityDiffDelete: _T.Unknown,
}


def _entity_additional(entity: Any) -> str:
    if isinstance(entity, tl.MessageEntityPre):
        return entity.language or ""
    if isinstance(entity, tl.MessageEntityTextUrl):
        return entity.url or ""
    if isinstance(entity, tl.MessageEntityMentionName):
        return number_to_string(entity.user_id)
    if isinstance(entity, tl.MessageEntityCustomEmoji):
        return number_to_string(entity.document_id)
    if isinstance(entity, tl.MessageEntityBlockquote):
        return "1" if entity.collapsed else ""
    return ""


def parse_text(text: str | None, entities: list[Any] | None) -> list[TextPart]:
    """Data::ParseText: entity offsets are UTF-16 units, as in QString."""
    text = text or ""
    utf16 = text.encode("utf-16-le", "surrogatepass")
    # QByteArray::size of the UTF-8 text, which tdesktop compares UTF-16 positions against.
    size = len(text.encode("utf-8", "surrogatepass"))

    def mid(offset: int, length: int) -> str:
        return utf16[offset * 2 : (offset + length) * 2].decode("utf-16-le", "replace")

    result: list[TextPart] = []
    offset = 0

    def add_text_part(till: int) -> None:
        nonlocal offset
        if till > offset:
            result.append(TextPart(text=mid(offset, till - offset)))
            offset = till

    for entity in entities or []:
        start, length = entity.offset, entity.length
        if start < offset or length <= 0 or start + length > size:
            continue
        add_text_part(start)
        result.append(
            TextPart(
                type=_ENTITY_TYPES.get(type(entity), _T.Unknown),
                text=mid(start, length),
                additional=_entity_additional(entity),
            )
        )
        offset = start + length
    add_text_part(size)
    return result


def parse_text_with_entities(value: Any) -> list[TextPart]:
    """Data::ParseText(MTPTextWithEntities)."""
    if value is None:
        return []
    return parse_text(value.text, value.entities)


@dataclass
class FileLocation:
    dc_id: int = 0
    # InputPhotoFileLocation / InputDocumentFileLocation (Telethon TL object).
    data: Any = None

    def __bool__(self) -> bool:
        return self.dc_id != 0


def refresh_file_reference(to: FileLocation, source: FileLocation) -> bool:
    """Data::RefreshFileReference: copy `source` into `to` when it names the same file."""
    if to.dc_id != source.dc_id or type(to.data) is not type(source.data):
        return False
    if isinstance(to.data, (tl.InputPhotoFileLocation, tl.InputDocumentFileLocation)):
        if to.data.id != source.data.id or to.data.thumb_size != source.data.thumb_size:
            return False
        to.dc_id, to.data = source.dc_id, source.data
        return True
    return False


@dataclass
class File:
    location: FileLocation = field(default_factory=FileLocation)
    size: int = 0
    content: bytes = b""
    suggested_path: str = ""
    relative_path: str = ""
    skip_reason: SkipReason = SkipReason.None_


@dataclass
class Image:
    width: int = 0
    height: int = 0
    file: File = field(default_factory=File)


@dataclass
class Photo:
    id: int = 0
    date: int = 0
    spoilered: bool = False
    image: Image = field(default_factory=Image)


@dataclass
class Document:
    id: int = 0
    date: int = 0
    file: File = field(default_factory=File)
    thumb: Image = field(default_factory=Image)
    name: str = ""
    mime: str = ""
    width: int = 0
    height: int = 0
    sticker_emoji: str = ""
    song_performer: str = ""
    song_title: str = ""
    duration: int = 0
    is_sticker: bool = False
    is_animated: bool = False
    is_video_message: bool = False
    is_voice_message: bool = False
    is_video_file: bool = False
    is_audio_file: bool = False
    spoilered: bool = False


@dataclass
class ParseMediaContext:
    self_peer_id: int = 0
    photos: int = 0
    audios: int = 0
    videos: int = 0
    files: int = 0
    contacts: int = 0
    bot_id: int = 0


def prepare_file_name_date_part(date: int) -> str:
    return "@" + format_date_time(date, False, "-", "-", "_") if date else ""


def prepare_photo_file_name(index: int, date: int) -> str:
    return f"photo_{index}{prepare_file_name_date_part(date)}.jpg"


def prepare_story_file_name(index: int, date: int, extension: str) -> str:
    return f"story_{index}{prepare_file_name_date_part(date)}{extension}"


_THUMB_SKIPPED = (tl.PhotoSizeEmpty, tl.PhotoStrippedSize, tl.PhotoPathSize)


def _size_area(size: Any) -> int:
    return 0 if isinstance(size, _THUMB_SKIPPED) else size.w * size.h


def parse_max_image(photo: tl.Photo, suggested_path: str) -> Image:
    result = Image()
    result.file.suggested_path = suggested_path
    max_area = 0
    for size in photo.sizes:
        if isinstance(size, _THUMB_SKIPPED):
            continue
        area = size.w * size.h
        if area <= max_area:
            continue
        result.width, result.height = size.w, size.h
        result.file.location = FileLocation(
            photo.dc_id,
            tl.InputPhotoFileLocation(
                id=photo.id,
                access_hash=photo.access_hash,
                file_reference=photo.file_reference,
                thumb_size=size.type,
            ),
        )
        if isinstance(size, tl.PhotoCachedSize):
            result.file.content = bytes(size.bytes)
            result.file.size = len(result.file.content)
        elif isinstance(size, tl.PhotoSizeProgressive):
            if not size.sizes:
                continue
            result.file.content = b""
            result.file.size = size.sizes[-1]
        else:
            result.file.content = b""
            result.file.size = size.size
        max_area = area
    return result


def parse_photo(data: Any, suggested_path: str) -> Photo:
    result = Photo()
    if isinstance(data, tl.Photo):
        result.id = data.id
        result.date = to_time(data.date)
        result.image = parse_max_image(data, suggested_path)
    elif isinstance(data, tl.PhotoEmpty):
        result.id = data.id
    return result


def parse_attributes(result: Document, attributes: list[Any]) -> None:
    for value in attributes:
        if isinstance(value, tl.DocumentAttributeImageSize):
            result.width, result.height = value.w, value.h
        elif isinstance(value, tl.DocumentAttributeAnimated):
            result.is_animated = True
        elif isinstance(value, (tl.DocumentAttributeSticker, tl.DocumentAttributeCustomEmoji)):
            result.is_sticker = True
            result.sticker_emoji = value.alt or ""
        elif isinstance(value, tl.DocumentAttributeVideo):
            if value.round_message:
                result.is_video_message = True
            else:
                result.is_video_file = True
            result.width, result.height = value.w, value.h
            result.duration = int(value.duration)
        elif isinstance(value, tl.DocumentAttributeAudio):
            if value.voice:
                result.is_voice_message = True
            else:
                result.is_audio_file = True
            if value.performer is not None:
                result.song_performer = value.performer
            if value.title is not None:
                result.song_title = value.title
            result.duration = int(value.duration)
        elif isinstance(value, tl.DocumentAttributeFilename):
            result.name = value.file_name


def compute_document_name(context: ParseMediaContext, data: Document, date: int) -> str:
    if data.name:
        return data.name
    pattern = mime_first_glob(data.mime)
    extension = pattern.replace("*", "")
    date_part = prepare_file_name_date_part(date)
    if data.is_voice_message:
        is_mp3 = data.mime.lower() == "audio/mp3"
        context.audios += 1
        return f"audio_{context.audios}{date_part}" + (".mp3" if is_mp3 else ".ogg")
    if data.is_video_file:
        context.videos += 1
        return f"video_{context.videos}{date_part}" + (extension if pattern else ".mov")
    context.files += 1
    return f"file_{context.files}{date_part}" + (extension if pattern else ".unknown")


def document_folder(data: Document) -> str:
    if data.is_video_file:
        return "video_files"
    if data.is_animated:
        return "animations"
    if data.is_sticker:
        return "stickers"
    if data.is_voice_message:
        return "voice_messages"
    if data.is_video_message:
        return "round_video_messages"
    return "files"


def parse_document_thumb(document: tl.Document, document_path: str) -> Image:
    if not document.thumbs:
        return Image()
    best = max(document.thumbs, key=_size_area)
    if isinstance(best, _THUMB_SKIPPED):
        return Image()
    result = Image(width=best.w, height=best.h)
    result.file.location = FileLocation(
        document.dc_id,
        tl.InputDocumentFileLocation(
            id=document.id,
            access_hash=document.access_hash,
            file_reference=document.file_reference,
            thumb_size=best.type,
        ),
    )
    if isinstance(best, tl.PhotoCachedSize):
        result.file.content = bytes(best.bytes)
        result.file.size = len(result.file.content)
    elif isinstance(best, tl.PhotoSizeProgressive):
        if not best.sizes:
            return Image()
        result.file.size = best.sizes[-1]
    else:
        result.file.size = best.size
    result.file.suggested_path = document_path + "_thumb.jpg"
    return result


def parse_document(
    context: ParseMediaContext, data: Any, suggested_folder: str, date: int
) -> Document:
    result = Document()
    if isinstance(data, tl.Document):
        result.id = data.id
        result.date = to_time(data.date)
        result.mime = data.mime_type or ""
        parse_attributes(result, data.attributes or [])
        result.file.size = data.size
        result.file.location = FileLocation(
            data.dc_id,
            tl.InputDocumentFileLocation(
                id=data.id,
                access_hash=data.access_hash,
                file_reference=data.file_reference,
                thumb_size="",
            ),
        )
        result.file.suggested_path = (
            suggested_folder
            + document_folder(result)
            + "/"
            + file_name_from_user_string(compute_document_name(context, result, date))
        )
        result.thumb = parse_document_thumb(data, result.file.suggested_path)
    elif isinstance(data, tl.DocumentEmpty):
        result.id = data.id
    return result
