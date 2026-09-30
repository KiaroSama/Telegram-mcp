"""Export a secret chat from Telethon-Secret-Chat's auto-saved records, Desktop-style.

Telegram Desktop cannot export secret chats; this renders the saved records (autosave.py of the
Telethon-Secret-Chat package, format 1) as the personal-chat export Desktop would write for the
same conversation, through the ExportWriter protocol of fetch.py. Built on the tdesktop v7.2.10
port in this package (Telegram/SourceFiles/export/, GPL-3.0).

Mapping choices where the secret layer has no Desktop counterpart:
- ids: Desktop needs increasing message ids, so messages get 1..N in saved order and reply_to
  random ids are mapped onto them (a reply to an unsaved message keeps no reply id).
- SetMessageTTL -> ActionSetMessagesTTL, ScreenshotMessages -> ActionScreenshotTaken,
  FlushHistory -> ActionHistoryClear; DeleteMessages has no Desktop service message: skipped.
- Self-destruct timers are not applied: the owner chose to keep those messages and files.
- Custom emoji cannot be resolved offline and read "(unavailable)", as an unresolved one does.
"""

from __future__ import annotations

import os
import shutil
from dataclasses import replace
from typing import Any

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .fetch import ExportResult, ExportWriter
from .fetch_files import message_file_work
from .files import SkipReason, file_name_from_user_string, normalize_path, prepare_relative_path
from .model import (
    Document,
    File,
    Image,
    ParseMediaContext,
    Photo,
    TextPart,
    compute_document_name,
    document_folder,
    parse_text,
    peer_from_user,
    prepare_photo_file_name,
)
from .model_actions import (
    ActionHistoryClear,
    ActionScreenshotTaken,
    ActionSetMessagesTTL,
    ServiceAction,
)
from .model_dialogs import DialogsInfo, dialog_info_from_user, finalize_dialogs_info
from .model_media import GeoPoint, Media, SharedContact, Venue
from .model_message import (
    Message,
    MessagesSlice,
    skip_message_by_date,
)
from .model_peers import parse_peers_lists, parse_user
from .settings import Environment, MediaSettings, Settings, normalize_settings

_MESSAGES_SLICE_LIMIT = 100
_DOCUMENT_MEDIA = (
    "decryptedMessageMediaDocument",
    "decryptedMessageMediaExternalDocument",
    "decryptedMessageMediaVideo",
    "decryptedMessageMediaAudio",
)


def _entity(data: dict[str, Any]) -> Any | None:
    """A saved entity dict ({"_": "messageEntityBold", ...}) as Telethon's MessageEntity*."""
    name = str(data.get("_", ""))
    cls = getattr(tl, name[:1].upper() + name[1:], None) if name else None
    if cls is None or not name.startswith("messageEntity"):
        return None
    fields = {k: v for k, v in data.items() if k != "_"}
    try:
        return cls(**fields)
    except TypeError:
        return None


def _apply_attributes(document: Document, attributes: list[dict[str, Any]]) -> None:
    """Data::ParseAttributes over the secret layer's documentAttribute* records."""
    for value in attributes:
        name = value.get("_")
        if name == "documentAttributeImageSize":
            document.width, document.height = value.get("w") or 0, value.get("h") or 0
        elif name == "documentAttributeAnimated":
            document.is_animated = True
        elif name == "documentAttributeSticker":
            document.is_sticker = True
            document.sticker_emoji = value.get("alt") or ""
        elif name == "documentAttributeVideo":
            if value.get("round_message"):
                document.is_video_message = True
            else:
                document.is_video_file = True
            document.width, document.height = value.get("w") or 0, value.get("h") or 0
            document.duration = int(value.get("duration") or 0)
        elif name == "documentAttributeAudio":
            if value.get("voice"):
                document.is_voice_message = True
            else:
                document.is_audio_file = True
            document.song_performer = value.get("performer") or ""
            document.song_title = value.get("title") or ""
            document.duration = int(value.get("duration") or 0)
        elif name == "documentAttributeFilename":
            document.name = value.get("file_name") or ""


def _document(context: ParseMediaContext, media: dict[str, Any], date: int) -> Document:
    """Data::ParseDocument for a secret document, video or audio (older layers included)."""
    kind = media["_"]
    result = Document(date=date, mime=media.get("mime_type") or "")
    result.file.size = int(media.get("size") or 0)
    if kind == "decryptedMessageMediaVideo":
        result.mime = result.mime or "video/mp4"
        result.is_video_file = True
        result.width, result.height = media.get("w") or 0, media.get("h") or 0
        result.duration = int(media.get("duration") or 0)
    elif kind == "decryptedMessageMediaAudio":
        # Layer-8/17 audio is the voice note of its time.
        result.mime = result.mime or "audio/ogg"
        result.is_voice_message = True
        result.duration = int(media.get("duration") or 0)
    else:
        if media.get("file_name"):
            result.name = media["file_name"]
        _apply_attributes(result, media.get("attributes") or [])
    result.file.suggested_path = (
        document_folder(result)
        + "/"
        + file_name_from_user_string(compute_document_name(context, result, date))
    )
    return result


def _media(context: ParseMediaContext, media: dict[str, Any] | None, date: int) -> Media:
    """Data::ParseMedia for a saved secret media record."""
    result = Media()
    kind = (media or {}).get("_")
    if media is None or kind is None:
        return result
    if kind == "decryptedMessageMediaPhoto":
        context.photos += 1
        photo = Photo(date=date)
        photo.image = Image(width=media.get("w") or 0, height=media.get("h") or 0)
        photo.image.file.size = int(media.get("size") or 0)
        photo.image.file.suggested_path = "photos/" + prepare_photo_file_name(context.photos, date)
        result.content = photo
    elif kind in _DOCUMENT_MEDIA:
        result.content = _document(context, media, date)
    elif kind == "decryptedMessageMediaGeoPoint":
        result.content = GeoPoint(latitude=media["lat"], longitude=media["long"], valid=True)
    elif kind == "decryptedMessageMediaVenue":
        point = GeoPoint(latitude=media["lat"], longitude=media["long"], valid=True)
        result.content = Venue(point=point, title=media["title"], address=media["address"])
    elif kind == "decryptedMessageMediaContact":
        contact = SharedContact()
        contact.info.user_id = int(media.get("user_id") or 0)
        contact.info.first_name = media.get("first_name") or ""
        contact.info.last_name = media.get("last_name") or ""
        contact.info.phone_number = media.get("phone_number") or ""
        result.content = contact
    # decryptedMessageMediaWebPage stays null: Desktop ignores web page media.
    return result


def _action(action: dict[str, Any]) -> Any | None:
    kind = action.get("_")
    if kind == "decryptedMessageActionSetMessageTTL":
        return ActionSetMessagesTTL(period=int(action.get("ttl_seconds") or 0))
    if kind == "decryptedMessageActionScreenshotMessages":
        return ActionScreenshotTaken()
    if kind == "decryptedMessageActionFlushHistory":
        return ActionHistoryClear()
    return None


def _unresolved_custom_emoji(message: Message) -> None:
    for part in message.text:
        if part.type == TextPart.Type.CustomEmoji:
            part.additional = TextPart.unavailable_emoji()


def _parse(
    records: list[dict[str, Any]], self_user: Any, peer_user: Any
) -> tuple[list[Message], dict[int, str]]:
    """The messages, and the saved file behind each message's main File (by id(File))."""
    self_peer, peer = peer_from_user(self_user.id), peer_from_user(peer_user.id)
    context = ParseMediaContext(self_peer_id=self_peer)
    kept: list[tuple[dict[str, Any], Message]] = []
    ids: dict[Any, int] = {}
    for record in records:
        kind = record.get("type")
        if kind == "service":
            content = _action(record.get("action") or {})
            if content is None:
                continue
            message = Message(action=ServiceAction(content))
        elif kind == "message":
            message = Message()
        else:
            continue
        message.id = len(kept) + 1
        message.date = int(record.get("date") or 0)
        message.out = bool(record.get("out"))
        message.peer_id = peer
        message.self_id = self_peer
        message.from_id = self_peer if message.out else peer
        ids[record.get("id")] = message.id
        kept.append((record, message))
    sources: dict[int, str] = {}
    for record, message in kept:
        if record.get("type") != "message":
            continue
        media = record.get("media")
        message.media = _media(context, media, message.date)
        caption = (media or {}).get("caption") or ""
        entities = [e for e in map(_entity, record.get("entities") or []) if e is not None]
        message.text = parse_text(record.get("text") or caption, entities)
        _unresolved_custom_emoji(message)
        reply = record.get("reply_to")
        message.reply_to_msg_id = ids.get(reply, 0) if reply is not None else 0
        saved = record.get("file")
        if saved and isinstance(message.media.content, (Photo, Document)):
            sources[id(message.media.file())] = str(saved)
    return [message for _, message in kept], sources


def parse_saved_messages(
    records: list[dict[str, Any]], self_user: Any, peer_user: Any
) -> list[Message]:
    """Saved secret-chat records -> Desktop export Messages (ids 1..N, oldest first)."""
    return _parse(records, self_user, peer_user)[0]


def _copy_files(messages: list[Message], sources: dict[int, str], settings: Settings) -> int:
    """ApiWrap::loadNextMessageFile + processFileLoad, with the saved file as the download."""
    copied = 0
    media = settings.media
    for message in messages:
        if skip_message_by_date(message, settings):
            continue
        for work in message_file_work(message):
            if _skipped(work.file, sources, work.media_type, work.controlling_size, media):
                continue
            file = work.file
            relative = prepare_relative_path(settings.path, file.suggested_path)
            target = settings.path + relative
            os.makedirs(os.path.dirname(target) or ".", exist_ok=True)
            shutil.copyfile(sources[id(file)], target)
            file.relative_path = relative
            copied += 1
    return copied


def _skipped(
    file: File,
    sources: dict[int, str],
    media_type: MediaSettings.Type,
    controlling_size: int,
    media: MediaSettings,
) -> bool:
    """processFileLoad's order: Unavailable, then FileType, then FileSize."""
    source = sources.get(id(file))
    if file.relative_path or file.skip_reason != SkipReason.None_:
        return True
    if not source or not os.path.isfile(source):
        file.skip_reason = SkipReason.Unavailable
    elif (media.types & media_type) != media_type:
        file.skip_reason = SkipReason.FileType
    elif (controlling_size or os.path.getsize(source)) > media.size_limit:
        file.skip_reason = SkipReason.FileSize
    else:
        return False
    return True


def _messages_count(messages: list[Message], settings: Settings) -> int:
    """requestMessagesCount + checkFirstMessageDate: 0 when the range misses the chat."""
    if not messages:
        return 0
    newest, oldest = messages[-1].date, messages[0].date
    if not (newest > 0 and newest > settings.single_peer_from):
        return 0
    if settings.single_peer_till > 0 and not (0 < oldest < settings.single_peer_till):
        return 0
    return len(messages)


def export_saved_chat(
    records: list[dict[str, Any]],
    settings: Settings,
    writer: ExportWriter,
    self_user: Any,
    peer_user: Any,
    environment: Environment | None = None,
) -> ExportResult:
    """Write the saved secret chat with `writer` into a folder under `settings.path`."""
    single_peer = tl.InputPeerUser(user_id=peer_user.id, access_hash=peer_user.access_hash or 0)
    settings = normalize_settings(replace(settings, single_peer=single_peer))
    settings.path = normalize_path(settings)
    messages, sources = _parse(records, self_user, peer_user)
    peers = parse_peers_lists([self_user, peer_user], [])

    dialogs = DialogsInfo(chats=[dialog_info_from_user(parse_user(peer_user))])
    info = dialogs.chats[0]
    info.splits, info.messages_count_per_split = [0], [_messages_count(messages, settings)]
    finalize_dialogs_info(dialogs, settings)

    writer.start(settings, environment or Environment())
    writer.write_dialogs_start(dialogs)
    writer.write_dialog_start(info)
    copied = 0
    if info.messages_count_per_split[0]:
        copied = _copy_files(messages, sources, settings)
        for start in range(0, len(messages), _MESSAGES_SLICE_LIMIT):
            chunk = messages[start : start + _MESSAGES_SLICE_LIMIT]
            writer.write_dialog_slice(MessagesSlice(list=chunk, peers=peers))
    writer.write_dialog_end()
    writer.write_dialogs_end()
    writer.finish()
    written = len(messages) if info.messages_count_per_split[0] else 0
    return ExportResult(settings.path, False, "", written, copied)
