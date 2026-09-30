"""Single-chat export data flow: takeout, dialog list, history slices and media files.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/export_api_wrap.cpp and
export_controller.cpp, the single-peer path), GPL-3.0.

The client is injected: anything awaitable as `client(request)` with `takeout(...)` and
`download_file(...)` in Telethon's shape. Writer methods mirror Output::AbstractWriter.
"""

from __future__ import annotations

import logging
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from typing import Any, Protocol

from telethon import errors  # type: ignore[import-untyped]
from telethon.tl import functions  # type: ignore[import-untyped]
from telethon.tl import types as tl

from .fetch_files import LoadedFileCache, MessageFiles, message_file_work, rpc_message
from .files import normalize_path
from .model import Document, ParseMediaContext, peer_from_channel, peer_from_user
from .model_dialogs import (
    DialogInfo,
    DialogsInfo,
    add_migrate_from_slice,
    finalize_dialogs_info,
    parse_dialogs_info_chats,
    parse_dialogs_info_users,
    settings_from_dialogs_type,
)
from .model_format import countries_from_tl, set_countries_list
from .model_message import (
    FileOrigin,
    Message,
    MessagesSlice,
    adjust_migrate_message_ids,
    parse_messages_slice,
    single_message_after,
    single_message_before,
    skip_message_by_date,
)
from .model_rich_parse import extract_full_rich_message, parse_rich_message
from .settings import Environment, Settings, normalize_settings

log = logging.getLogger(__name__)

_MESSAGES_SLICE_LIMIT = 100
_INT_MAX = 2**31 - 1


class ExportWriter(Protocol):
    def start(self, settings: Settings, environment: Environment) -> None: ...
    def write_dialogs_start(self, data: DialogsInfo) -> None: ...
    def write_dialog_start(self, data: DialogInfo) -> None: ...
    def write_dialog_slice(self, data: MessagesSlice) -> None: ...
    def write_dialog_end(self) -> None: ...
    def write_dialogs_end(self) -> None: ...
    def finish(self) -> None: ...


@dataclass
class ExportResult:
    path: str
    takeout: bool
    takeout_error: str = ""
    messages: int = 0
    files: int = 0


@dataclass
class _ChatProcess:
    info: DialogInfo
    context: ParseMediaContext
    local_split_index: int = 0
    largest_id_plus_one: int = 1
    last_slice: bool = False


@dataclass
class _Exporter(MessageFiles):
    client: Any
    settings: Settings
    writer: ExportWriter
    environment: Environment
    api: Any = None
    splits: list[Any] = field(default_factory=list)
    use_splits: bool = False
    self_id: int = 0
    file_cache: LoadedFileCache = field(default_factory=LoadedFileCache)
    resolved_emoji: dict[int, Document] = field(default_factory=dict)
    unresolved_emoji: set[int] = field(default_factory=set)
    messages: int = 0
    files: int = 0

    async def main(self, request: Any) -> Any:
        """mainRequest: through the takeout session when there is one."""
        return await self.api(request)

    async def split(self, index: int, request: Any) -> Any:
        """splitRequest: invokeWithMessagesRange needs takeout; without it there is one split."""
        if self.use_splits:
            request = functions.InvokeWithMessagesRangeRequest(self.splits[index], request)
        return await self.api(request)

    async def run(self) -> None:
        self.writer.start(self.settings, self.environment)
        dialogs = await self.collect_dialogs_list()
        self.writer.write_dialogs_start(dialogs)
        index = 0
        while (info := dialogs.item(index)) is not None:
            await self.export_dialog(info)
            index += 1
        self.writer.write_dialogs_end()
        self.writer.finish()

    async def request_split_ranges(self) -> None:
        if self.use_splits:
            self.splits = list(await self.main(functions.messages.GetSplitRangesRequest()))
        if not self.splits:
            self.splits = [tl.MessageRange(min_id=1, max_id=_INT_MAX)]

    async def collect_dialogs_list(self) -> DialogsInfo:
        """requestDialogsList -> requestSinglePeerDialog -> appendSinglePeerDialogs."""
        result = DialogsInfo()
        index_by_peer: dict[int, int] = {}
        peer = self.settings.single_peer
        if isinstance(peer, (tl.InputPeerUser, tl.InputPeerSelf)):
            user = (
                tl.InputUser(user_id=peer.user_id, access_hash=peer.access_hash)
                if isinstance(peer, tl.InputPeerUser)
                else tl.InputUserSelf()
            )
            users = await self.main(functions.users.GetUsersRequest([user]))
            infos = parse_dialogs_info_users(peer, users)
        elif isinstance(peer, tl.InputPeerChat):
            chats = await self.main(functions.messages.GetChatsRequest([peer.chat_id]))
            infos = parse_dialogs_info_chats(peer, chats.chats)
        elif isinstance(peer, tl.InputPeerChannel):
            channel = tl.InputChannel(channel_id=peer.channel_id, access_hash=peer.access_hash)
            chats = await self.main(functions.channels.GetChannelsRequest([channel]))
            infos = parse_dialogs_info_chats(peer, chats.chats)
        else:
            raise ValueError("Unexpected single peer in ApiWrap::requestSinglePeerDialog.")
        migrated = await self.append_single_peer_dialogs(result, index_by_peer, infos)
        if migrated is not None:
            await self.append_single_peer_dialogs(result, index_by_peer, migrated)
        finalize_dialogs_info(result, self.settings)
        return result

    async def append_single_peer_dialogs(
        self, result: DialogsInfo, index_by_peer: dict[int, int], info: DialogsInfo
    ) -> DialogsInfo | None:
        """Returns the migrated-from chat to append next, as requestSinglePeerMigrated does."""
        supergroups = (DialogInfo.Type.PrivateSupergroup, DialogInfo.Type.PublicSupergroup)
        channels = (DialogInfo.Type.PrivateChannel, DialogInfo.Type.PublicChannel)
        last = len(self.splits) - 1
        migrated_source: DialogInfo | None = None
        for chat in info.chats:
            if chat.type in supergroups and migrated_source is None:
                migrated_source = chat
                continue
            if chat.type in channels or chat.is_monoforum:
                continue
            for i in range(last, 0, -1):
                chat.splits.append(i - 1)
                chat.messages_count_per_split.append(0)
        self.append_chats_slice(result.chats, index_by_peer, info.chats, last)
        if migrated_source is None:
            return None
        return await self.request_single_peer_migrated(migrated_source)

    async def request_single_peer_migrated(self, info: DialogInfo) -> DialogsInfo:
        channel = tl.InputChannel(
            channel_id=info.input.channel_id, access_hash=info.input.access_hash
        )
        full = await self.main(functions.channels.GetFullChannelRequest(channel))
        migrated_id = getattr(full.full_chat, "migrated_from_chat_id", None) or 0
        if not migrated_id:
            return DialogsInfo()
        return parse_dialogs_info_chats(tl.InputPeerChat(chat_id=migrated_id), full.chats)

    def append_chats_slice(
        self,
        to: list[DialogInfo],
        index_by_peer: dict[int, int],
        source: list[DialogInfo],
        split_index: int,
    ) -> None:
        types = self.settings.types
        groups = Settings.Type.PublicGroups | Settings.Type.PrivateGroups

        def good_by_types(info: DialogInfo) -> bool:
            return bool(types & settings_from_dialogs_type(info.type))

        for info in source:
            if not good_by_types(info) and not (info.migrated_to_channel_id and types & groups):
                continue
            if info.migrated_to_channel_id:
                target = index_by_peer.get(peer_from_channel(info.migrated_to_channel_id))
                if target is not None and add_migrate_from_slice(
                    to[target], info, split_index, len(self.splits)
                ):
                    continue
                if not good_by_types(info):
                    continue
            position = index_by_peer.setdefault(info.peer_id, len(to))
            if position == len(to):
                to.append(info)
            to[position].splits.append(split_index)
            to[position].messages_count_per_split.append(0)

    async def export_dialog(self, info: DialogInfo) -> None:
        """requestMessages for one dialog, driving the writer like exportNextDialog."""
        process = _ChatProcess(info, ParseMediaContext(self_peer_id=peer_from_user(self.self_id)))
        for local in range(len(info.splits)):
            info.messages_count_per_split[local] = await self.request_messages_count(
                process, local
            )
        self.writer.write_dialog_start(info)
        while True:
            count = info.messages_count_per_split[process.local_split_index]
            if not count:
                slice_ = MessagesSlice()
            else:
                slice_ = await self.request_messages_slice(process)
            if not slice_.list:
                process.last_slice = True
            await self.load_slice_files(process, slice_)
            if slice_.list:
                process.largest_id_plus_one = slice_.list[-1].id + 1
                if info.splits[process.local_split_index] < 0:
                    slice_ = adjust_migrate_message_ids(slice_)
                self.messages += len(slice_.list)
                self.writer.write_dialog_slice(slice_)
            if process.last_slice and process.local_split_index + 1 < len(info.splits):
                process.local_split_index += 1
                process.last_slice = False
                process.largest_id_plus_one = 1
            if process.last_slice:
                break
        self.writer.write_dialog_end()

    async def request_messages_count(self, process: _ChatProcess, local: int) -> int:
        split = process.info.splits[local]
        result = await self.request_chat_messages(process, split, 0, 0, 1)
        if isinstance(result, tl.messages.MessagesNotModified):
            raise RuntimeError("Unexpected messagesNotModified received.")
        if isinstance(result, tl.messages.Messages):
            count = len(result.messages)
        else:
            count = result.count
        if not single_message_after(result, self.settings.single_peer_from):
            return 0
        if self.settings.single_peer_till <= 0:
            return count
        first = await self.request_chat_messages(process, split, 1, -1, 1)
        return count if single_message_before(first, self.settings.single_peer_till) else 0

    async def request_messages_slice(self, process: _ChatProcess) -> MessagesSlice:
        split = process.info.splits[process.local_split_index]
        result = await self.request_chat_messages(
            process,
            split,
            process.largest_id_plus_one,
            -_MESSAGES_SLICE_LIMIT,
            _MESSAGES_SLICE_LIMIT,
        )
        if isinstance(result, tl.messages.MessagesNotModified):
            raise RuntimeError("Unexpected messagesNotModified received.")
        if isinstance(result, tl.messages.Messages):
            process.last_slice = True
        return parse_messages_slice(
            process.context,
            result.messages,
            result.users,
            result.chats,
            process.info.relative_path,
        )

    async def request_chat_messages(
        self, process: _ChatProcess, split_index: int, offset_id: int, add_offset: int, limit: int
    ) -> Any:
        info = process.info
        peer = info.input if split_index >= 0 else info.migrated_from_input
        outgoing = info.monoforum_broadcast_input if info.is_monoforum else tl.InputPeerSelf()
        real_split = split_index if split_index >= 0 else len(self.splits) + split_index
        if info.only_my_messages:
            request: Any = functions.messages.SearchRequest(
                peer=peer,
                q="",
                filter=tl.InputMessagesFilterEmpty(),
                min_date=None,
                max_date=None,
                offset_id=offset_id,
                add_offset=add_offset,
                limit=limit,
                max_id=0,
                min_id=0,
                hash=0,
                from_id=outgoing,
            )
            return await self.split(real_split, request)
        request = functions.messages.GetHistoryRequest(
            peer=peer,
            offset_id=offset_id,
            offset_date=None,
            add_offset=add_offset,
            limit=limit,
            max_id=0,
            min_id=0,
            hash=0,
        )
        try:
            return await self.split(real_split, request)
        except errors.RPCError as error:
            # Perhaps we just left / were kicked from the channel: only my messages then.
            if rpc_message(error) == "CHANNEL_PRIVATE" and isinstance(peer, tl.InputPeerChannel):
                info.only_my_messages = True
                return await self.request_chat_messages(
                    process, split_index, offset_id, add_offset, limit
                )
            raise

    def current_origin(self, process: _ChatProcess, message: Message) -> FileOrigin:
        split = process.info.splits[process.local_split_index]
        return FileOrigin(
            message_id=message.id,
            split=split if split >= 0 else len(self.splits) + split,
            peer=process.info.input if split >= 0 else process.info.migrated_from_input,
        )

    async def load_slice_files(self, process: _ChatProcess, slice_: MessagesSlice) -> None:
        """resumeMessagesSlice -> resolveCustomEmoji -> loadNextMessageFile."""
        for message in slice_.list:
            if skip_message_by_date(message, self.settings):
                continue
            rich = message.rich_message
            if rich is not None and rich.part:
                await self.hydrate_rich_message(process, message)
        self.collect_custom_emoji(slice_)
        folder = process.info.relative_path
        await self.resolve_custom_emoji(process.context, folder)
        for message in slice_.list:
            if skip_message_by_date(message, self.settings):
                continue
            await self.resolve_message_custom_emoji(folder, message)
            origin = self.current_origin(process, message)
            for work in message_file_work(message):
                file_origin = FileOrigin(
                    origin.split, origin.peer, origin.message_id, rich_message=work.rich
                )
                await self.process_file_load(
                    folder, work.file, file_origin, message, work.media_type, work.controlling_size
                )

    async def hydrate_rich_message(self, process: _ChatProcess, message: Message) -> None:
        origin = self.current_origin(process, message)
        result = await self.main(functions.messages.GetRichMessageRequest(origin.peer, message.id))
        rich = extract_full_rich_message(getattr(result, "messages", []), message.id)
        if rich is None:
            raise RuntimeError("Unexpected rich message hydration result.")
        parsed = parse_rich_message(
            process.context, rich, process.info.relative_path, message.date
        )
        if parsed.part:
            raise RuntimeError("Unexpected rich message hydration result.")
        message.rich_message = parsed


async def _load_countries(client: Any) -> None:
    """countries_manager.cpp: the server's country list, which FormatPhoneNumber uses."""
    try:
        result = await client(functions.help.GetCountriesListRequest(lang_code="", hash=0))
    except errors.RPCError:
        log.warning("API Error: getting countries failed.")
        return
    if isinstance(result, tl.help.CountriesList):
        set_countries_list(countries_from_tl(result))


async def _self_id(client: Any) -> int:
    users = await client(functions.users.GetUsersRequest([tl.InputUserSelf()]))
    for user in users:
        if isinstance(user, tl.User) and user.is_self:
            return int(user.id)
    raise RuntimeError("Could not retrieve selfId.")


def _takeout_kwargs(settings: Settings) -> dict[str, Any]:
    """ApiWrap::startMainSession flags for NormalizeSettings'ed single-peer settings."""
    kind = Settings.Type
    size_limit = settings.media.size_limit
    has_files = (
        (bool(settings.media.types) and size_limit > 0)
        or bool(settings.types & kind.Userpics)
        or bool(settings.types & kind.Stories)
    )
    return dict(
        contacts=bool(settings.types & kind.Contacts) or None,
        users=bool(settings.types & (kind.PersonalChats | kind.BotChats)) or None,
        chats=bool(settings.types & kind.PrivateGroups) or None,
        megagroups=bool(settings.types & (kind.PrivateGroups | kind.PublicGroups)) or None,
        channels=bool(settings.types & (kind.PrivateChannels | kind.PublicChannels)) or None,
        files=has_files or None,
        # files and file_max_size are one flag (flags.5); Desktop always serializes
        # MTP_long(sizeLimit), the 4000 MB ceiling included.
        max_file_size=size_limit if has_files else None,
    )


async def export_single_chat(
    client: Any,
    settings: Settings,
    writer: ExportWriter,
    environment: Environment | None = None,
) -> ExportResult:
    """Export `settings.single_peer` into a folder under `settings.path`, like Telegram Desktop.

    Tries a takeout session first (account.initTakeoutSession with tdesktop's flags) and falls
    back to ordinary requests when Telegram refuses or asks to wait; the result says which.
    """
    settings = normalize_settings(settings)
    if not settings.only_single_peer():
        raise ValueError("export_single_chat needs settings.single_peer.")
    settings.path = normalize_path(settings)
    exporter = _Exporter(client, settings, writer, environment or Environment())
    exporter.self_id = await _self_id(client)
    await _load_countries(client)
    reason = ""
    async with AsyncExitStack() as stack:
        try:
            takeout = await stack.enter_async_context(
                client.takeout(finalize=True, **_takeout_kwargs(settings))
            )
        except (errors.RPCError, ValueError) as error:
            # TakeoutInitDelayError (asked to wait), another refusal, or a takeout already
            # open on this session: read through the ordinary client instead.
            reason = type(error).__name__
            log.info("Export takeout refused (%s); using ordinary requests.", reason)
            takeout = None
        exporter.api = takeout if takeout is not None else client
        exporter.use_splits = takeout is not None
        await exporter.request_split_ranges()
        await exporter.run()
    return ExportResult(
        settings.path, takeout is not None, reason, exporter.messages, exporter.files
    )
