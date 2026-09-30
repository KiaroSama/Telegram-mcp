"""Recent actions (admin log) written the way Telegram Desktop exports a chat (spec 033 R10).

Telegram Desktop has no export of Recent actions; the owner chose (2026-10-01) its chat export
instead: the same folder, the same `messages.html` pages and `result.json`, written by the
same `tdexport` writers, with every event drawn as Desktop's Recent actions view draws it
(`admin_log_events`): a service line with Desktop's sentence, then the deleted, edited,
pinned or sent message as an ordinary bubble with its own text, entities and media, which
download under the export's media kinds and size limit. Oldest first, as an export is.

Telegram keeps about 48 hours of admin log; the whole of it is read, page by page.
"""

from __future__ import annotations

import itertools
from dataclasses import dataclass, replace
from typing import Any, Callable

from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.admin_log_events import event_parts
from telegram_mcp.admin_log_text import Line, LogChat, Logged, Note
from telegram_mcp.tdexport import model_actions as act
from telegram_mcp.tdexport.fetch import _load_countries, _self_id
from telegram_mcp.tdexport.fetch_files import LoadedFileCache, MessageFiles, message_file_work
from telegram_mcp.tdexport.files import normalize_path
from telegram_mcp.tdexport.html_and_json import HtmlAndJsonWriter
from telegram_mcp.tdexport.html_text import serialize_string
from telegram_mcp.tdexport.html_writer import HtmlWriter
from telegram_mcp.tdexport.json_writer import JsonWriter
from telegram_mcp.tdexport.model import (
    ParseMediaContext,
    TextPart,
    peer_from_channel,
    peer_from_user,
    to_time,
)
from telegram_mcp.tdexport.model_dialogs import finalize_dialogs_info, parse_dialogs_info_chats
from telegram_mcp.tdexport.model_message import FileOrigin, Message, MessagesSlice, parse_message
from telegram_mcp.tdexport.model_peers import parse_peers_lists
from telegram_mcp.tdexport.settings import Environment, Format, Settings, normalize_settings

#: The dialog's name: the chat's title and Desktop's lng_manage_peer_recent_actions.
TITLE_SUFFIX = " - Recent actions"
_PAGE = 100  # channels.getAdminLog's own maximum
_SLICE = 100


@dataclass
class ActionCustomAction(act.ActionCustomAction, act.ActionChatEditPhoto):
    """A sentence with the new chat photo under it, as Desktop draws a photo change.

    tdexport picks a service line's HTML text by the action's class NAME (so this one
    writes the sentence) and shows and downloads the photo of an ActionChatEditPhoto (so
    this one has a thumbnail); result.json writes it as the photo change it is.
    """


@dataclass
class AdminLogResult:
    path: str
    events: int = 0
    messages: int = 0
    files: int = 0


class _HtmlWriter(HtmlWriter):
    """The chat export's HTML writer. Its service lines are markup already, while a
    sentence here is plain text, so the sentence is escaped on its way in; result.json
    serializes the same sentence as the JSON string it is."""

    def write_dialog_slice(self, data: MessagesSlice) -> None:
        super().write_dialog_slice(MessagesSlice([_escaped(m) for m in data.list], data.peers))


def _escaped(message: Message) -> Message:
    content = message.action.content
    if not isinstance(content, act.ActionCustomAction):
        return message
    escaped = replace(content, message=serialize_string(content.message))
    return replace(message, action=act.ServiceAction(escaped))


def writer_for(fmt: Format) -> Any:
    """export_dialog.writer_for, with the HTML writer that escapes sentences."""
    if fmt is Format.Json:
        return JsonWriter()
    if fmt is Format.Html:
        return _HtmlWriter()
    return HtmlAndJsonWriter(_HtmlWriter(), JsonWriter())


class _Files(MessageFiles):
    """tdexport's file half: downloads, size and type rules, custom emoji."""

    def __init__(self, client: Any, settings: Settings, self_id: int) -> None:
        self.client = client
        self.settings = settings
        self.self_id = self_id
        self.file_cache = LoadedFileCache()
        self.resolved_emoji = {}
        self.unresolved_emoji = set()
        self.files = 0

    async def main(self, request: Any) -> Any:
        return await self.client(request)

    async def split(self, index: int, request: Any) -> Any:
        return await self.client(request)

    async def refresh_reference(self, folder: str, origin: FileOrigin, location: Any) -> Any:
        # A chat photo of the log has no message to read it again from.
        if not origin.message_id:
            return None
        return await super().refresh_reference(folder, origin, location)

    async def load(
        self, context: ParseMediaContext, slice_: MessagesSlice, origins: dict[int, FileOrigin]
    ) -> None:
        """Export's loadSliceFiles without rich-message hydration."""
        folder = ""
        self.collect_custom_emoji(slice_)
        await self.resolve_custom_emoji(context, folder)
        for message in slice_.list:
            await self.resolve_message_custom_emoji(folder, message)
            origin = origins.get(message.id, FileOrigin())
            for work in message_file_work(message):
                await self.process_file_load(
                    folder, work.file, origin, message, work.media_type, work.controlling_size
                )


def _logged(context: ParseMediaContext, data: Any) -> Message:
    """PrepareLogMessage: the carried message without its edit date, reactions, reply
    header (its target is not in this export) and outgoing flag."""
    message = parse_message(context, data, "")
    message.edited = 0
    message.out = False
    message.reactions = []
    message.reply_to_msg_id = message.reply_to_peer_id = 0
    return message


def event_messages(
    event: Any, chat: LogChat, context: ParseMediaContext, next_id: Callable[[], int]
) -> "tuple[list[Message], dict[int, FileOrigin]]":
    """One event as export messages, and where each one's files can be read again."""
    actor = peer_from_user(event.user_id)
    when = to_time(event.date)
    channel = tl.InputPeerChannel(chat.channel_id, chat.access_hash)
    messages: list[Message] = []
    origins: dict[int, FileOrigin] = {}
    for part in event_parts(event, chat):
        real_id = 0
        if isinstance(part, Line):
            content: Any = act.ActionCustomAction(part.text)
            if part.photo is not None:
                photo_action = tl.MessageActionChatEditPhoto(part.photo)
                parsed = act.parse_service_action(context, photo_action, "", when).content
                content = ActionCustomAction(message=part.text, photo=parsed.photo)
            message = Message(action=act.ServiceAction(content), from_id=actor)
        elif isinstance(part, Note):
            message = Message(text=list(part.parts), from_id=actor)
        elif isinstance(part, Logged):
            message = _logged(context, part.message)
            real_id = message.id
        else:  # Original: the block inside the bubble above it, from the same sender
            source = _logged(context, part.message) if part.message is not None else Message()
            message = Message(media=source.media, from_id=messages[-1].from_id if messages else 0)
            message.text = [
                TextPart(type=TextPart.Type.Bold, text=part.title),
                TextPart(text="\n"),
                *part.parts,
            ]
            real_id = source.id
        message.id = next_id()
        message.date = when
        message.peer_id = peer_from_channel(chat.channel_id)
        message.from_id = message.from_id or actor
        messages.append(message)
        origins[message.id] = FileOrigin(peer=channel, message_id=real_id)
    return messages, origins


async def fetch_events(
    client: Any,
    channel: Any,
    events_filter: Any = None,
    admins: Any = None,
    query: str = "",
    since: int = 0,
) -> "tuple[list[Any], dict[int, Any], dict[int, Any]]":
    """channels.getAdminLog paged back through max_id until empty; oldest first."""
    events: dict[int, Any] = {}
    users: dict[int, Any] = {}
    chats: dict[int, Any] = {}
    max_id = 0
    while True:
        page = await client(
            functions.channels.GetAdminLogRequest(
                channel=channel,
                q=query or "",
                events_filter=events_filter,
                admins=admins or None,
                max_id=max_id,
                min_id=0,
                limit=_PAGE,
            )
        )
        users.update((u.id, u) for u in page.users or [])
        chats.update((c.id, c) for c in page.chats or [])
        fresh = [e for e in page.events or [] if e.id not in events]
        if not fresh:
            break
        events.update((e.id, e) for e in fresh)
        max_id = min(e.id for e in fresh)
        if since and min(to_time(e.date) for e in fresh) < since:
            break
    ordered = sorted(events.values(), key=lambda e: (to_time(e.date), e.id))
    return ordered, users, chats


async def export_admin_log(
    client: Any,
    settings: Settings,
    writer: Any,
    environment: Environment | None = None,
    *,
    events_filter: Any = None,
    admins: Any = None,
    query: str = "",
) -> AdminLogResult:
    """Export the Recent actions of `settings.single_peer` (a supergroup or channel)."""
    environment = environment or Environment()
    settings = normalize_settings(settings)
    settings.path = normalize_path(settings)
    peer = settings.single_peer
    if not isinstance(peer, tl.InputPeerChannel):
        raise ValueError("Recent actions exist only in supergroups and channels.")
    channel = tl.InputChannel(peer.channel_id, peer.access_hash)
    self_id = await _self_id(client)
    await _load_countries(client)
    found = await client(functions.channels.GetChannelsRequest([channel]))
    info = parse_dialogs_info_chats(peer, found.chats)
    finalize_dialogs_info(info, settings)
    dialog = info.chats[0]
    dialog.name += TITLE_SUFFIX
    entity = next(c for c in found.chats if c.id == peer.channel_id)

    since, till = settings.single_peer_from, settings.single_peer_till
    events, users, chats = await fetch_events(client, channel, events_filter, admins, query, since)
    events = [
        e
        for e in events
        if (not since or to_time(e.date) >= since) and (till <= 0 or to_time(e.date) < till)
    ]
    chats.setdefault(entity.id, entity)
    defaults = getattr(entity, "default_banned_rights", None)
    chat = LogChat(
        channel_id=peer.channel_id,
        access_hash=peer.access_hash,
        megagroup=bool(getattr(entity, "megagroup", False)),
        broadcast=bool(getattr(entity, "broadcast", False)),
        anyone_can_add=not getattr(defaults, "invite_users", False),
        self_id=self_id,
        link_prefix=environment.internal_links_domain or "https://t.me/",
        users=users,
        chats=chats,
    )
    peers = parse_peers_lists(list(users.values()), list(chats.values()))
    context = ParseMediaContext(self_peer_id=peer_from_user(self_id))
    files = _Files(client, settings, self_id)
    ids = itertools.count(1)
    written = 0

    writer.start(settings, environment)
    writer.write_dialogs_start(info)
    writer.write_dialog_start(dialog)
    batch: list[Message] = []
    origins: dict[int, FileOrigin] = {}

    async def flush() -> None:
        nonlocal written
        if not batch:
            return
        slice_ = MessagesSlice(list(batch), peers)
        await files.load(context, slice_, origins)
        writer.write_dialog_slice(slice_)
        written += len(batch)
        batch.clear()
        origins.clear()

    for event in events:
        messages, event_origins = event_messages(event, chat, context, lambda: next(ids))
        batch.extend(messages)
        origins.update(event_origins)
        if len(batch) >= _SLICE:
            await flush()
    await flush()
    writer.write_dialog_end()
    writer.write_dialogs_end()
    writer.finish()
    return AdminLogResult(settings.path, len(events), written, files.files)
