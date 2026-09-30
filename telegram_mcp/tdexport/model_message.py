"""Messages: reactions, inline keyboards, Message, ParseMessage and message slices.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.{h,cpp}
Reaction, HistoryMessageMarkupButton, Message, FileOrigin, ParseMessage, ParseReactions,
ButtonRowsFromTL, MessagesSlice, ParseMessagesSlice, AdjustMigrateMessageIds,
SingleMessageBefore/After, SkipMessageByDate), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, List

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .model import (
    MIGRATED_MESSAGES_ID_SHIFT,
    Document,
    File,
    Image,
    ParseMediaContext,
    TextPart,
    parse_peer_id,
    parse_text,
    peer_is_user,
    peer_to_channel,
    peer_to_user,
    to_time,
)
from .model_actions import (
    ActionChatEditPhoto,
    ActionSuggestProfilePhoto,
    ServiceAction,
    parse_service_action,
)
from .model_format import number_to_string
from .model_media import Media, parse_media
from .model_peers import Peer, parse_peers_lists
from .model_rich import RichMessage
from .model_rich_parse import parse_rich_message
from .settings import Settings


@dataclass
class Reaction:
    class Type(Enum):
        Empty = 0
        Emoji = 1
        CustomEmoji = 2
        Paid = 3

    @dataclass
    class Recent:
        peer_id: int = 0
        date: int = 0

    type: Reaction.Type = Type.Empty
    emoji: str = ""
    document_id: str = ""
    count: int = 0
    recent: list[Reaction.Recent] = field(default_factory=list)

    @staticmethod
    def type_to_string(reaction: Reaction) -> str:
        return _REACTION_TYPE_NAMES[reaction.type]

    @staticmethod
    def id(reaction: Reaction) -> str:
        """Reaction::Id: the type name followed by the emoji or the document id."""
        suffix = ""
        if reaction.type == Reaction.Type.Emoji:
            suffix = reaction.emoji
        elif reaction.type == Reaction.Type.CustomEmoji:
            suffix = reaction.document_id
        return Reaction.type_to_string(reaction) + suffix


_REACTION_TYPE_NAMES = {
    Reaction.Type.Empty: "empty",
    Reaction.Type.Emoji: "emoji",
    Reaction.Type.CustomEmoji: "custom_emoji",
    Reaction.Type.Paid: "paid",
}


@dataclass(frozen=True, order=True)
class MessageId:
    channel_id: int = 0
    msg_id: int = 0


@dataclass
class HistoryMessageMarkupButton:
    class Type(Enum):
        Default = 0
        Url = 1
        Callback = 2
        CallbackWithPassword = 3
        RequestPhone = 4
        RequestLocation = 5
        RequestPoll = 6
        RequestPeer = 7
        SwitchInline = 8
        SwitchInlineSame = 9
        Game = 10
        Buy = 11
        Auth = 12
        UserProfile = 13
        WebView = 14
        SimpleWebView = 15
        CopyText = 16
        Disabled = 17

    type: HistoryMessageMarkupButton.Type = Type.Default
    text: str = ""
    # QByteArray: URL/query/copy text as UTF-8, or raw callback data.
    data: bytes = b""
    forward_text: str = ""
    button_id: int = 0

    @staticmethod
    def type_to_string(button: HistoryMessageMarkupButton) -> str:
        return _BUTTON_TYPE_NAMES[button.type]


_B = HistoryMessageMarkupButton.Type
_BUTTON_TYPE_NAMES = {
    _B.Default: "default",
    _B.Url: "url",
    _B.Callback: "callback",
    _B.CallbackWithPassword: "callback_with_password",
    _B.RequestPhone: "request_phone",
    _B.RequestLocation: "request_location",
    _B.RequestPoll: "request_poll",
    _B.RequestPeer: "request_peer",
    _B.SwitchInline: "switch_inline",
    _B.SwitchInlineSame: "switch_inline_same",
    _B.Game: "game",
    _B.Buy: "buy",
    _B.Auth: "auth",
    _B.UserProfile: "user_profile",
    _B.WebView: "web_view",
    _B.SimpleWebView: "simple_web_view",
    _B.CopyText: "copy_text",
    _B.Disabled: "disabled",
}


@dataclass
class Message:
    id: int = 0
    date: int = 0
    edited: int = 0
    from_id: int = 0
    peer_id: int = 0
    self_id: int = 0
    forwarded_from_id: int = 0
    forwarded_from_name: str = ""
    forwarded_date: int = 0
    forwarded: bool = False
    show_forwarded_as_original: bool = False
    saved_from_chat_id: int = 0
    signature: str = ""
    via_bot_id: int = 0
    reply_to_msg_id: int = 0
    reply_to_peer_id: int = 0
    text: list[TextPart] = field(default_factory=list)
    reactions: list[Reaction] = field(default_factory=list)
    media: Media = field(default_factory=Media)
    action: ServiceAction = field(default_factory=ServiceAction)
    out: bool = False
    inline_button_rows: list[list[HistoryMessageMarkupButton]] = field(default_factory=list)
    rich_message: RichMessage | None = None

    def file(self) -> File:
        content = self.action.content
        if isinstance(content, (ActionChatEditPhoto, ActionSuggestProfilePhoto)):
            return content.photo.image.file
        return self.media.file()

    def thumb(self) -> Image:
        return self.media.thumb()


@dataclass
class FileOrigin:
    split: int = 0
    peer: Any = None
    message_id: int = 0
    story_id: int = 0
    custom_emoji_id: int = 0
    rich_message: bool = False


def parse_reaction(reaction: Any) -> Reaction:
    if isinstance(reaction, tl.ReactionEmoji):
        return Reaction(type=Reaction.Type.Emoji, emoji=reaction.emoticon)
    if isinstance(reaction, tl.ReactionCustomEmoji):
        return Reaction(
            type=Reaction.Type.CustomEmoji, document_id=number_to_string(reaction.document_id)
        )
    if isinstance(reaction, tl.ReactionPaid):
        return Reaction(type=Reaction.Type.Paid)
    return Reaction(type=Reaction.Type.Empty)


def parse_reactions(data: tl.MessageReactions) -> list[Reaction]:
    # Insertion order of the dict is tdesktop's reactionsOrder.
    reactions: dict[str, Reaction] = {}
    for single in data.results:
        reaction = parse_reaction(single.reaction)
        reaction.count = single.count
        reactions.setdefault(Reaction.id(reaction), reaction)
    for recent in data.recent_reactions or []:
        reaction = parse_reaction(recent.reaction)
        stored = reactions.setdefault(Reaction.id(reaction), reaction)
        stored.recent.append(
            Reaction.Recent(peer_id=parse_peer_id(recent.peer_id), date=to_time(recent.date))
        )
    return list(reactions.values())


def _markup_button(button: Any) -> HistoryMessageMarkupButton | None:
    kind = button.type
    make = HistoryMessageMarkupButton
    text = button.text
    if isinstance(kind, tl.InlineButtonTypeUrl):
        return make(_B.Url, text, kind.url.encode("utf-8"))
    if isinstance(kind, tl.InlineButtonTypeUrlAuth):
        return make(_B.Auth, text, kind.url.encode("utf-8"), kind.fwd_text or "", kind.button_id)
    if isinstance(kind, tl.InlineButtonTypeWebView):
        return make(_B.WebView, text, kind.url.encode("utf-8"))
    if isinstance(kind, tl.InlineButtonTypeCallback):
        button_type = _B.CallbackWithPassword if kind.requires_password else _B.Callback
        return make(button_type, text, bytes(kind.data))
    if isinstance(kind, tl.InlineButtonTypeGame):
        return make(_B.Game, text)
    if isinstance(kind, tl.InlineButtonTypeBuy):
        return make(_B.Buy, text)
    if isinstance(kind, tl.InlineButtonTypeSwitchInline):
        button_type = _B.SwitchInlineSame if kind.same_peer else _B.SwitchInline
        return make(button_type, text, kind.query.encode("utf-8"))
    if isinstance(kind, tl.InlineButtonTypeUserProfile):
        return make(_B.UserProfile, text, str(kind.user_id).encode("utf-8"))
    if isinstance(kind, tl.InlineButtonTypeCopy):
        return make(_B.CopyText, text, kind.copy_text.encode("utf-8"))
    if isinstance(kind, tl.InlineButtonTypeDisabled):
        return make(_B.Disabled, text)
    # InputInlineButtonTypeUrlAuth / InputInlineButtonTypeUserProfile add nothing.
    return None


def button_rows_from_tl(data: tl.ReplyInlineMarkup) -> list[list[HistoryMessageMarkupButton]]:
    rows: list[list[HistoryMessageMarkupButton]] = []
    for tl_row in data.rows:
        row = [b for b in (_markup_button(x) for x in tl_row.buttons) if b is not None]
        if row:
            rows.append(row)
    return rows


def _reply_header(result: Message, reply: Any, drop_same_peer: bool) -> None:
    if not isinstance(reply, tl.MessageReplyHeader) or reply.reply_to_msg_id is None:
        return
    result.reply_to_msg_id = reply.reply_to_msg_id
    result.reply_to_peer_id = (
        parse_peer_id(reply.reply_to_peer_id) if reply.reply_to_peer_id is not None else 0
    )
    if drop_same_peer and result.reply_to_peer_id == result.peer_id:
        result.reply_to_peer_id = 0


def _parse_forward(result: Message, forward: tl.MessageFwdHeader) -> None:
    result.forwarded_from_id = parse_peer_id(forward.from_id) if forward.from_id else 0
    result.forwarded_from_name = forward.from_name or ""
    result.forwarded_date = to_time(forward.date)
    result.saved_from_chat_id = (
        parse_peer_id(forward.saved_from_peer) if forward.saved_from_peer else 0
    )
    result.forwarded = bool(result.forwarded_from_id or result.forwarded_from_name)
    result.show_forwarded_as_original = result.forwarded and bool(result.saved_from_chat_id)


def parse_message(context: ParseMediaContext, data: Any, media_folder: str) -> Message:
    result = Message(id=data.id)
    if isinstance(data, tl.MessageEmpty):
        return result
    result.date = to_time(data.date)
    result.out = bool(data.out)
    result.self_id = context.self_peer_id
    result.peer_id = parse_peer_id(data.peer_id)
    result.from_id = parse_peer_id(data.from_id) if data.from_id is not None else result.peer_id
    _reply_header(result, data.reply_to, True)
    if isinstance(data, tl.MessageService):
        result.action = parse_service_action(context, data.action, media_folder, result.date)
        return result

    if data.edit_date is not None:
        result.edited = to_time(data.edit_date)
    if isinstance(data.fwd_from, tl.MessageFwdHeader):
        _parse_forward(result, data.fwd_from)
    if data.post_author is not None:
        result.signature = data.post_author
    # tdesktop parses the reply header again here, without dropping a same-peer reply peer.
    _reply_header(result, data.reply_to, False)
    if data.via_bot_id is not None:
        result.via_bot_id = data.via_bot_id
    if data.media is not None:
        context.bot_id = (
            result.via_bot_id
            if result.via_bot_id
            else (
                peer_to_user(result.forwarded_from_id)
                if peer_is_user(result.forwarded_from_id)
                else peer_to_user(result.from_id)
            )
        )
        result.media = parse_media(context, data.media, media_folder, result.date)
        if result.media.ttl and not data.out:
            result.media.reset_file()
            if isinstance(result.media.content, Document):
                result.media.content.thumb.file = File()
        context.bot_id = 0
    if isinstance(data.reply_markup, tl.ReplyInlineMarkup):
        result.inline_button_rows = button_rows_from_tl(data.reply_markup)
    result.text = parse_text(data.message, data.entities)
    if data.reactions is not None:
        result.reactions = parse_reactions(data.reactions)
    if data.rich_message is not None:
        result.rich_message = parse_rich_message(
            context, data.rich_message, media_folder, result.date
        )
    return result


def parse_messages_list(
    self_id: int, data: list[Any], media_folder: str
) -> dict[MessageId, Message]:
    context = ParseMediaContext(self_peer_id=self_id)
    result: dict[MessageId, Message] = {}
    for message in data:
        parsed = parse_message(context, message, media_folder)
        result.setdefault(MessageId(peer_to_channel(parsed.peer_id), parsed.id), parsed)
    return dict(sorted(result.items()))


@dataclass
class MessagesSlice:
    list: List[Message] = field(default_factory=list)
    peers: dict[int, Peer] = field(default_factory=dict)


def parse_messages_slice(
    context: ParseMediaContext,
    data: list[Any],
    users: list[Any],
    chats: list[Any],
    media_folder: str,
) -> MessagesSlice:
    # The server sends newest first; the slice is oldest first.
    messages = [parse_message(context, message, media_folder) for message in reversed(data)]
    return MessagesSlice(list=messages, peers=parse_peers_lists(users, chats))


def adjust_migrate_message_ids(slice_: MessagesSlice) -> MessagesSlice:
    for message in slice_.list:
        message.id += MIGRATED_MESSAGES_ID_SHIFT
        if message.reply_to_msg_id and not message.reply_to_peer_id:
            message.reply_to_msg_id += MIGRATED_MESSAGES_ID_SHIFT
    return slice_


def single_message_date(data: Any) -> int:
    if isinstance(data, tl.messages.MessagesNotModified):
        return 0
    messages = getattr(data, "messages", None) or []
    if not messages or isinstance(messages[0], tl.MessageEmpty):
        return 0
    return to_time(messages[0].date)


def single_message_before(data: Any, date: int) -> bool:
    single = single_message_date(data)
    return 0 < single < date


def single_message_after(data: Any, date: int) -> bool:
    single = single_message_date(data)
    return single > 0 and single > date


def skip_message_by_date(message: Message, settings: Settings) -> bool:
    good_from = settings.single_peer_from <= 0 or settings.single_peer_from <= message.date
    good_till = settings.single_peer_till <= 0 or message.date < settings.single_peer_till
    return not good_from or not good_till
