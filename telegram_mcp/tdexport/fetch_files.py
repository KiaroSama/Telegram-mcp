"""Media files and custom emoji of exported messages: selection, download, reference refresh.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/export_api_wrap.cpp
LoadedFileCache, buildMessageFileWork, processFileLoad, writePreloadedFile, loadFile,
filePartDone, filePartRefreshReference, collectMessagesCustomEmoji, resolveCustomEmoji,
getCustomEmoji), GPL-3.0.
"""

from __future__ import annotations

import logging
import os
from collections import deque
from dataclasses import dataclass
from typing import Any, BinaryIO

from telethon import errors  # type: ignore[import-untyped]
from telethon.tl import functions  # type: ignore[import-untyped]
from telethon.tl import types as tl

from .files import SkipReason, apply_file_policy, document_media_type, prepare_relative_path
from .model import (
    Document,
    File,
    FileLocation,
    ParseMediaContext,
    Photo,
    TextPart,
    parse_document,
    peer_from_user,
    refresh_file_reference,
)
from .model_actions import ActionChatEditPhoto, ActionSuggestProfilePhoto
from .model_media import SharedContact
from .model_message import (
    FileOrigin,
    Message,
    MessagesSlice,
    Reaction,
    parse_messages_slice,
    skip_message_by_date,
)
from .model_rich import RichText
from .model_rich_parse import (
    RichVisitor,
    extract_full_rich_message,
    parse_rich_message,
    refresh_rich_message_file_reference,
    visit_rich_message,
)
from .settings import MediaSettings, Settings

log = logging.getLogger(__name__)

FILE_CHUNK_KB = 128
_MAX_EMOJI_PER_REQUEST = 100
_LOCATION_CACHE_SIZE = 100_000
_UNAVAILABLE_ERRORS = ("LOCATION_INVALID", "VERSION_INVALID", "LOCATION_NOT_AVAILABLE")
# ponytail: tdesktop retries a FILE_REFERENCE_* error for as long as the server returns it; a
# bounded count keeps a misbehaving server from looping forever.
_MAX_REFERENCE_REFRESHES = 3


# Telethon raises a class per known error and keeps only the HTTP-like status in `.message`;
# tdesktop matches on the MTProto error type, so recover it from the class.
_ERROR_NAMES = {cls: name for name, cls in errors.rpc_errors_dict.items()}


def rpc_message(error: BaseException) -> str:
    """The MTProto error type, e.g. "FILE_REFERENCE_EXPIRED"."""
    name = _ERROR_NAMES.get(type(error))
    return name if name is not None else str(getattr(error, "message", "") or "")


def compute_location_key(location: FileLocation) -> tuple[int, int]:
    """ApiWrap ComputeLocationKey."""
    key_type = location.dc_id
    data = location.data
    letter = ord(data.thumb_size[0]) if getattr(data, "thumb_size", "") else 0
    if isinstance(data, tl.InputDocumentFileLocation):
        return key_type | (2 << 24) | (letter << 16), data.id
    if isinstance(data, tl.InputPhotoFileLocation):
        return key_type | (6 << 24) | (letter << 16), data.id
    return key_type | (5 << 24), 0


class LoadedFileCache:
    """ApiWrap::LoadedFileCache: relative paths of files already written, by location."""

    def __init__(self, limit: int = _LOCATION_CACHE_SIZE) -> None:
        self._limit = limit
        self._map: dict[tuple[int, int], str] = {}
        self._list: deque[tuple[int, int]] = deque()

    def save(self, location: FileLocation, relative_path: str) -> None:
        if not location:
            return
        key = compute_location_key(location)
        self._map[key] = relative_path
        self._list.append(key)
        if len(self._list) > self._limit:
            self._map.pop(self._list.popleft(), None)

    def find(self, location: FileLocation) -> str | None:
        return self._map.get(compute_location_key(location)) if location else None


class LazyFile:
    """Output::File: nothing exists on disk until the first block arrives."""

    def __init__(self, path: str) -> None:
        self._path = path
        self._handle: BinaryIO | None = None

    def write(self, data: bytes) -> int:
        if self._handle is None:
            os.makedirs(os.path.dirname(self._path) or ".", exist_ok=True)
            self._handle = open(self._path, "wb")
        self._handle.write(data)
        return len(data)

    def finish(self) -> None:
        """A download that delivered no bytes still leaves an (empty) file behind."""
        if self._handle is None:
            self.write(b"")
        self.close()

    def close(self) -> None:
        if self._handle is not None:
            self._handle.close()
            self._handle = None


@dataclass
class FileWork:
    """ApiWrap::MessageFileWork."""

    file: File
    media_type: MediaSettings.Type
    controlling_size: int
    rich: bool = False


def message_file_work(message: Message) -> list[FileWork]:
    """ApiWrap::buildMessageFileWork: the files of one message, in loading order."""
    work: list[FileWork] = []
    main: File | None = None
    document: Document | None = None
    main_type = MediaSettings.Type.Photo
    action = message.action.content
    if isinstance(action, (ActionChatEditPhoto, ActionSuggestProfilePhoto)):
        main = action.photo.image.file
    content = message.media.content
    if isinstance(content, Photo):
        main = main or content.image.file
    elif isinstance(content, Document):
        document = content
        if main is None:
            main = content.file
            main_type = document_media_type(content)
    elif isinstance(content, SharedContact):
        main = main or content.vcard
    main_size = main.size if main is not None else 0
    if main is not None:
        work.append(FileWork(main, main_type, main_size))
    if document is not None and document.thumb.width > 0:
        work.append(FileWork(document.thumb.file, document_media_type(document), main_size))
    if message.rich_message is None:
        return work
    seen: set[int] = set()

    def add(file: File, media_type: MediaSettings.Type, size: int) -> None:
        if id(file) not in seen:
            seen.add(id(file))
            work.append(FileWork(file, media_type, size, rich=True))

    def on_photo(photo: Photo) -> None:
        add(photo.image.file, MediaSettings.Type.Photo, photo.image.file.size)

    def on_document(rich_document: Document) -> None:
        kind = document_media_type(rich_document)
        add(rich_document.file, kind, rich_document.file.size)
        if rich_document.thumb.width > 0:
            add(rich_document.thumb.file, kind, rich_document.file.size)

    visit_rich_message(message.rich_message, RichVisitor(photo=on_photo, document=on_document))
    return work


def to_ulonglong(value: str) -> int:
    """QByteArray::toULongLong: 0 for anything that is not a decimal number."""
    stripped = value.strip()
    return int(stripped) if stripped.isascii() and stripped.isdigit() else 0


class MessageFiles:
    """The file half of ApiWrap; the exporter supplies the requests and the state below."""

    client: Any
    settings: Settings
    self_id: int
    file_cache: LoadedFileCache
    resolved_emoji: dict[int, Document]
    unresolved_emoji: set[int]
    files: int

    async def main(self, request: Any) -> Any:
        raise NotImplementedError

    async def split(self, index: int, request: Any) -> Any:
        raise NotImplementedError

    def collect_custom_emoji(self, slice_: MessagesSlice) -> None:
        def collect(value: int) -> None:
            if value and value not in self.resolved_emoji:
                self.unresolved_emoji.add(value)

        def on_text(text: RichText) -> None:
            if text.type == RichText.Type.CustomEmoji:
                collect(text.id)

        for message in slice_.list:
            if skip_message_by_date(message, self.settings):
                continue
            for part in message.text:
                if part.type == TextPart.Type.CustomEmoji:
                    collect(to_ulonglong(part.additional))
            for reaction in message.reactions:
                if reaction.type == Reaction.Type.CustomEmoji:
                    collect(to_ulonglong(reaction.document_id))
            if message.rich_message is not None:
                visit_rich_message(message.rich_message, RichVisitor(text=on_text))

    async def resolve_custom_emoji(self, context: ParseMediaContext, folder: str) -> None:
        """Up to 100 ids a request, the largest first, like tdesktop's flat_set tail."""
        while self.unresolved_emoji:
            ordered = sorted(self.unresolved_emoji)
            chunk = ordered[-min(len(ordered), _MAX_EMOJI_PER_REQUEST) :]
            self.unresolved_emoji.difference_update(chunk)
            try:
                documents = await self.main(
                    functions.messages.GetCustomEmojiDocumentsRequest(document_id=chunk)
                )
            except errors.RPCError:
                log.warning("Export Error: Failed to get documents for emoji.")
                documents = []
            for entry in documents:
                document = parse_document(context, entry, folder, 0)
                self.resolved_emoji.setdefault(document.id, document)
            for emoji_id in chunk:
                if emoji_id not in self.resolved_emoji:
                    self.resolved_emoji[emoji_id] = Document(
                        file=File(skip_reason=SkipReason.Unavailable)
                    )

    async def custom_emoji_value(self, folder: str, message: Message, data: str) -> str:
        """ApiWrap::getCustomEmoji: an emoji id becomes its file path or a placeholder."""
        emoji_id = to_ulonglong(data)
        if not emoji_id:
            return data
        document = self.resolved_emoji.get(emoji_id)
        if document is None:
            return TextPart.unavailable_emoji()
        await self.process_file_load(
            folder,
            document.file,
            FileOrigin(custom_emoji_id=emoji_id),
            message,
            document_media_type(document),
            document.file.size,
        )
        if document.file.skip_reason == SkipReason.Unavailable:
            return TextPart.unavailable_emoji()
        if document.file.skip_reason in (SkipReason.FileType, SkipReason.FileSize):
            return ""
        return document.file.relative_path

    async def resolve_message_custom_emoji(self, folder: str, message: Message) -> None:
        """ApiWrap::messageCustomEmojiReady."""
        for part in message.text:
            if part.type == TextPart.Type.CustomEmoji:
                part.additional = await self.custom_emoji_value(folder, message, part.additional)
        for reaction in message.reactions:
            if reaction.type == Reaction.Type.CustomEmoji:
                reaction.document_id = await self.custom_emoji_value(
                    folder, message, reaction.document_id
                )
        if message.rich_message is None:
            return
        texts: list[RichText] = []
        visit_rich_message(message.rich_message, RichVisitor(text=texts.append))
        for text in texts:
            if text.type == RichText.Type.CustomEmoji:
                text.custom_emoji_data = await self.custom_emoji_value(
                    folder, message, text.custom_emoji_data
                )

    async def process_file_load(
        self,
        folder: str,
        file: File,
        origin: FileOrigin,
        message: Message,
        media_type: MediaSettings.Type,
        controlling_size: int,
    ) -> None:
        """ApiWrap::processFileLoad (FilePolicy overload) + writePreloadedFile + loadFile."""
        skip_by_date = skip_message_by_date(message, self.settings)
        media = self.settings.media
        if apply_file_policy(file, skip_by_date, media_type, controlling_size, media):
            return
        cached = self.file_cache.find(file.location)
        if cached is not None:
            file.relative_path = cached
            return
        relative = prepare_relative_path(self.settings.path, file.suggested_path)
        if file.content:
            target = LazyFile(self.settings.path + relative)
            target.write(file.content)
            target.close()
            file.relative_path = relative
            self.file_cache.save(file.location, relative)
            return
        file.relative_path = await self.load_file(folder, file, origin, relative)
        if not file.relative_path:
            file.skip_reason = SkipReason.Unavailable

    async def load_file(self, folder: str, file: File, origin: FileOrigin, relative: str) -> str:
        """loadFile/filePartDone: the relative path, or "" when the file is unavailable."""
        location = FileLocation(file.location.dc_id, file.location.data)
        path = self.settings.path + relative
        target = LazyFile(path)
        refreshes = 0
        try:
            while True:
                try:
                    await self.client.download_file(
                        location.data,
                        target,
                        part_size_kb=FILE_CHUNK_KB,
                        file_size=file.size or None,
                        dc_id=location.dc_id,
                    )
                    break
                except errors.RPCError as error:
                    message = rpc_message(error)
                    if message in _UNAVAILABLE_ERRORS:
                        log.warning("Export Error: File unavailable.")
                        return ""
                    if not (error.code == 400 and message.startswith("FILE_REFERENCE_")):
                        raise
                    refreshes += 1
                    refreshed = None
                    if refreshes <= _MAX_REFERENCE_REFRESHES:
                        refreshed = await self.refresh_reference(folder, origin, location)
                    if refreshed is None:
                        log.warning("Export Error: File unavailable.")
                        return ""
                    location = refreshed
                    target.close()
                    target = LazyFile(path)
            target.finish()
        finally:
            target.close()
        self.files += 1
        self.file_cache.save(location, relative)
        return relative

    async def refresh_reference(
        self, folder: str, origin: FileOrigin, location: FileLocation
    ) -> FileLocation | None:
        """filePartRefreshReference and the filePartExtract*Reference handlers."""
        context = ParseMediaContext(self_peer_id=peer_from_user(self.self_id))
        refreshed = FileLocation(location.dc_id, location.data)
        try:
            if origin.custom_emoji_id:
                documents = await self.main(
                    functions.messages.GetCustomEmojiDocumentsRequest([origin.custom_emoji_id])
                )
                parsed_documents = [parse_document(context, d, folder, 0) for d in documents]
                matches = [d for d in parsed_documents if d.id == origin.custom_emoji_id]
                if len(matches) != 1:
                    return None
                document = matches[0]
                if refresh_file_reference(refreshed, document.file.location):
                    return refreshed
                if document.thumb.width > 0 and refresh_file_reference(
                    refreshed, document.thumb.file.location
                ):
                    return refreshed
                return None
            if origin.rich_message:
                if not origin.message_id:
                    return None
                result = await self.main(
                    functions.messages.GetRichMessageRequest(origin.peer, origin.message_id)
                )
                messages = getattr(result, "messages", [])
                rich = extract_full_rich_message(messages, origin.message_id)
                if rich is None:
                    return None
                parsed = parse_rich_message(context, rich, folder, 0)
                if parsed.part:
                    return None
                return refresh_rich_message_file_reference(location, parsed)
            if not origin.message_id:
                raise RuntimeError("FILE_REFERENCE error for non-message file.")
            ids = [tl.InputMessageID(origin.message_id)]
            peer = origin.peer
            if isinstance(peer, (tl.InputPeerChannel, tl.InputPeerChannelFromMessage)):
                if isinstance(peer, tl.InputPeerChannel):
                    channel = tl.InputChannel(peer.channel_id, peer.access_hash)
                else:
                    channel = tl.InputChannelFromMessage(peer.peer, peer.msg_id, peer.channel_id)
                result = await self.main(functions.channels.GetMessagesRequest(channel, ids))
            else:
                request = functions.messages.GetMessagesRequest(ids)
                result = await self.split(origin.split, request)
        except errors.RPCError:
            return None
        found = parse_messages_slice(context, result.messages, result.users, result.chats, folder)
        for message in found.list:
            if message.id != origin.message_id:
                continue
            if refresh_file_reference(refreshed, message.file().location):
                return refreshed
            if refresh_file_reference(refreshed, message.thumb().file.location):
                return refreshed
        return None
