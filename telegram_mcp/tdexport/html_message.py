"""One message of the chat history: service lines, the joined/default bubble and its parts.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
HtmlWriter::MessageInfo, HtmlWriter::Wrap::pushServiceMessage/pushMessage/messageNeedsWrap/
forwardedNeedsWrap/pushMedia, WriteUserpicThumb), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from enum import Enum
from typing import Callable

from .files import write_image_thumb_sized
from .html_media import MediaMixin, prepare_media_data
from .html_media_extra import ExtraMediaMixin
from .html_rich import RichMediaCallbacks, render_rich_message
from .html_service import service_text
from .html_text import (
    PeersMap,
    UserpicData,
    compose_name,
    fill_userpic_names_from_full,
    fill_userpic_names_from_peer,
    format_custom_emoji,
    format_text,
    format_time_text,
    local_datetime,
    serialize_string,
)
from .model import Document, Photo, peer_id_color_index, peer_color_index, peer_is_user
from .model_actions import ActionChatEditPhoto, ActionSuggestProfilePhoto
from .model_dialogs import DialogInfo
from .model_format import format_date_time, number_to_string
from .model_media import GiveawayResults, GiveawayStart, Poll, TodoList, UnsupportedMedia
from .model_message import HistoryMessageMarkupButton, Message, Reaction

SERVICE_MESSAGE_PHOTO_SIZE = 60
HISTORY_USERPIC_SIZE = 42
JOIN_WITHIN_SECONDS = 900
WAVING_HAND = chr(0x1F44B)
STAR = chr(0x2B50)

WrapMessageLink = Callable[[int, str], str]


@dataclass
class MessageInfo:
    class Type(Enum):
        Service = 0
        Default = 1

    id: int = 0
    type: MessageInfo.Type = Type.Service
    from_id: int = 0
    via_bot_id: int = 0
    date: int = 0
    forwarded_from_id: int = 0
    forwarded_from_name: str = ""
    forwarded: bool = False
    show_forwarded_as_original: bool = False
    forwarded_date: int = 0


def write_userpic_thumb(
    base_path: str, large_path: str, userpic: UserpicData, postfix: str = "_thumb"
) -> str:
    size = userpic.pixel_size * 2
    return write_image_thumb_sized(base_path, large_path, size, size, postfix)


def _button_onclick(button: HistoryMessageMarkupButton) -> str:
    endline = " | "
    data = button.data.decode("utf-8", "replace")
    content = (
        ("Data: " + data + endline if button.data else "")
        + ("Forward text: " + button.forward_text + endline if button.forward_text else "")
        + "Type: "
        + HistoryMessageMarkupButton.type_to_string(button)
    )
    escaped = content.replace("\\", "\\\\").replace("'", "\\'")
    return "return ShowTextCopied('" + escaped + "');"


class MessageMixin(ExtraMediaMixin, MediaMixin):
    def push_service_message(
        self,
        message_id: int,
        dialog: DialogInfo,
        base_path: str,
        serialized: str,
        photo: Photo | None = None,
    ) -> str:
        result = self.push_tag(
            "div", {"class": "message service", "id": "message" + number_to_string(message_id)}
        )
        result += self.push_div("body details") + serialized + self.pop_tag()
        if photo is not None:
            userpic = UserpicData(
                color_index=dialog.color_index,
                first_name=dialog.name,
                last_name=dialog.last_name,
                pixel_size=SERVICE_MESSAGE_PHOTO_SIZE,
                large_link=photo.image.file.relative_path,
            )
            userpic.image_link = write_userpic_thumb(base_path, userpic.large_link, userpic)
            result += self.push_div("userpic_wrap") + self.push_userpic(userpic) + self.pop_tag()
        return result + self.pop_tag()

    def push_message(
        self,
        message: Message,
        previous: MessageInfo | None,
        dialog: DialogInfo,
        base_path: str,
        peers: PeersMap,
        internal_links_domain: str,
        wrap_message_link: WrapMessageLink,
    ) -> tuple[MessageInfo, str]:
        info = MessageInfo(
            id=message.id,
            from_id=message.from_id,
            via_bot_id=message.via_bot_id,
            date=message.date,
            forwarded_from_id=message.forwarded_from_id,
            forwarded_from_name=message.forwarded_from_name,
            forwarded_date=message.forwarded_date,
            forwarded=message.forwarded,
            show_forwarded_as_original=message.show_forwarded_as_original,
        )
        if isinstance(message.media.content, UnsupportedMedia):
            text = (
                "This message is not supported by this version "
                "of Telegram Desktop. Please update the application."
            )
            return info, self.push_service_message(message.id, dialog, base_path, text)

        def wrap_reply_to_link(text: str) -> str:
            return wrap_message_link(message.reply_to_msg_id, text)

        service = service_text(
            message, dialog, peers, internal_links_domain, self._base, wrap_reply_to_link
        )
        if service:
            content = message.action.content
            photo = (
                content.photo
                if isinstance(content, (ActionChatEditPhoto, ActionSuggestProfilePhoto))
                else None
            )
            return info, self.push_service_message(message.id, dialog, base_path, service, photo)
        info.type = MessageInfo.Type.Default
        return info, self._push_default(
            message, previous, base_path, peers, internal_links_domain, wrap_message_link
        )

    def _userpics(self, message: Message, peers: PeersMap) -> tuple[UserpicData, UserpicData]:
        forwarded = UserpicData()
        if message.forwarded:
            forwarded.color_index = (
                peer_id_color_index(message.forwarded_from_id)
                if message.forwarded_from_id
                else peer_color_index(message.id & 0xFFFFFFFFFFFFFFFF)
            )
            forwarded.pixel_size = HISTORY_USERPIC_SIZE
            if message.forwarded_from_id:
                fill_userpic_names_from_peer(forwarded, peers.peer(message.forwarded_from_id))
            else:
                fill_userpic_names_from_full(forwarded, message.forwarded_from_name)
        if message.show_forwarded_as_original:
            return replace(forwarded), forwarded
        userpic = UserpicData(
            color_index=peer_id_color_index(message.from_id), pixel_size=HISTORY_USERPIC_SIZE
        )
        fill_userpic_names_from_peer(userpic, peers.peer(message.from_id))
        return userpic, forwarded

    def _push_default(
        self,
        message: Message,
        previous: MessageInfo | None,
        base_path: str,
        peers: PeersMap,
        internal_links_domain: str,
        wrap_message_link: WrapMessageLink,
    ) -> str:
        wrap = self.message_needs_wrap(message, previous)
        show_forwarded_info = message.forwarded and not message.show_forwarded_as_original
        userpic, forwarded_userpic = self._userpics(message, peers)
        via = ""
        if message.via_bot_id:
            bot = peers.user(message.via_bot_id)
            if bot.username:
                via = serialize_string(bot.username)
        class_name = "message default clearfix" + ("" if wrap else " joined")
        block = self.push_tag(
            "div", {"class": class_name, "id": "message" + number_to_string(message.id)}
        )
        if wrap:
            block += self.push_div("pull_left userpic_wrap")
            block += self.push_userpic(userpic) + self.pop_tag()
        block += self.push_div("body")
        block += self.push_tag(
            "div",
            {"class": "pull_right date details", "title": format_date_time(message.date, True)},
        )
        block += format_time_text(message.date) + self.pop_tag()
        if wrap:
            block += self.push_div("from_name")
            block += serialize_string(compose_name(userpic, "Deleted Account"))
            if via and (not message.forwarded or message.show_forwarded_as_original):
                block += " via @" + via
            block += self.pop_tag()
        if show_forwarded_info:
            forwarded_wrap = self.forwarded_needs_wrap(message, previous)
            if forwarded_wrap:
                block += self.push_div("pull_left forwarded userpic_wrap")
                block += self.push_userpic(forwarded_userpic) + self.pop_tag()
            block += self.push_div("forwarded body")
            if forwarded_wrap:
                block += self.push_div("from_name")
                block += serialize_string(compose_name(forwarded_userpic, "Deleted Account"))
                if via:
                    block += " via @" + via
                block += self.push_tag(
                    "span",
                    {
                        "class": "date details",
                        "title": format_date_time(message.forwarded_date, True),
                        "inline": "",
                    },
                )
                block += " " + format_date_time(message.forwarded_date)
                block += self.pop_tag() + self.pop_tag()
        if message.reply_to_msg_id:
            block += self.push_div("reply_to details")
            if message.reply_to_peer_id:
                block += "In reply to a message in another chat"
            else:
                block += "In reply to " + wrap_message_link(
                    message.reply_to_msg_id, "this message"
                )
            block += self.pop_tag()
        block += self.push_media(
            message, base_path, peers, internal_links_domain, wrap_message_link
        )
        if message.rich_message is not None:
            block += render_rich_message(
                self._context,
                message.rich_message,
                message.id,
                internal_links_domain,
                self._base,
                self._rich_callbacks(base_path),
            )
        else:
            text = format_text(message.text, internal_links_domain, self._base)
            if text:
                block += self.push_div("text") + text + self.pop_tag()
        if message.inline_button_rows:
            block += self._push_inline_buttons(message)
        if message.signature:
            block += self.push_div("signature details")
            block += serialize_string(message.signature) + self.pop_tag()
        if show_forwarded_info:
            block += self.pop_tag()
        if message.reactions:
            block += self._push_reactions(message.reactions, peers)
        return block + self.pop_tag() + self.pop_tag()

    def _rich_callbacks(self, base_path: str) -> RichMediaCallbacks:
        def photo(data: Photo | None) -> str:
            return self.push_rich_photo_media(data, base_path)

        def video(data: Document | None) -> str:
            return self.push_rich_video_media(data, base_path)

        return RichMediaCallbacks(
            photo=photo,
            video=video,
            audio=self.push_rich_audio_media,
            file=self.push_rich_file_media,
            generic=self.push_generic_media,
            photo_card=lambda data, card: self.push_rich_reference_media(data, card, base_path),
        )

    def _push_inline_buttons(self, message: Message) -> str:
        block = self.push_tag("table", {"class": "bot_buttons_table"})
        block += self.push_tag("tbody")
        for row in message.inline_button_rows:
            block += self.push_tag("tr") + self.push_tag("td", {"class": "bot_button_row"})
            for index, button in enumerate(row):
                url = button.type == HistoryMessageMarkupButton.Type.Url
                link = button.data.decode("utf-8", "surrogateescape") if url else ""
                onclick = "" if url else _button_onclick(button)
                # std::map{ Attribute(), ... }: an empty pair becomes a nameless attribute,
                # and of two empty pairs only the first is kept.
                attributes: dict[str, str] = {}
                for name, value in (("href", link), ("onclick", onclick)):
                    attributes.setdefault(name if value else "", value)
                block += self.push_tag("div", {"class": "bot_button"})
                block += self.push_tag("a", attributes) + self.push_tag("div")
                block += serialize_string(button.text)
                block += self.pop_tag() + self.pop_tag() + self.pop_tag()
                if index != len(row) - 1:
                    block += self.push_tag("div", {"class": "bot_button_column_separator"})
                    block += self.pop_tag()
            block += self.pop_tag() + self.pop_tag()
        return block + self.pop_tag() + self.pop_tag()

    def _push_reactions(self, reactions: list[Reaction], peers: PeersMap) -> str:
        block = self.push_tag("span", {"class": "reactions"})
        for reaction in reactions:
            reaction_class = "reaction"
            for recent in reaction.recent:
                user = peers.peer(recent.peer_id).user()
                if user is not None and user.is_self:
                    reaction_class += " active"
                    break
            if reaction.type == Reaction.Type.Paid:
                reaction_class += " paid"
            block += self.push_tag("span", {"class": reaction_class})
            block += self.push_tag("span", {"class": "emoji"})
            if reaction.type == Reaction.Type.Emoji:
                block += serialize_string(reaction.emoji)
            elif reaction.type == Reaction.Type.CustomEmoji:
                block += format_custom_emoji(reaction.document_id, WAVING_HAND, self._base)
            elif reaction.type == Reaction.Type.Paid:
                block += serialize_string(STAR)
            block += self.pop_tag()
            if reaction.recent:
                block += self.push_tag("span", {"class": "userpics"})
                for recent in reaction.recent:
                    peer = peers.peer(recent.peer_id)
                    user = peer.user()
                    block += self.push_userpic(
                        UserpicData(
                            color_index=peer.color_index(),
                            pixel_size=20,
                            first_name=user.info.first_name if user else peer.name(),
                            last_name=user.info.last_name if user else "",
                            tooltip=peer.name(),
                        )
                    )
                block += self.pop_tag()
            if not reaction.recent or reaction.count > len(reaction.recent):
                block += self.push_tag("span", {"class": "count"})
                block += number_to_string(reaction.count) + self.pop_tag()
            block += self.pop_tag()
        return block + self.pop_tag()

    def message_needs_wrap(self, message: Message, previous: MessageInfo | None) -> bool:
        if previous is None or previous.type != MessageInfo.Type.Default:
            return True
        if not message.from_id or previous.from_id != message.from_id:
            return True
        if message.via_bot_id != previous.via_bot_id:
            return True
        if local_datetime(previous.date).date() != local_datetime(message.date).date():
            return True
        if (
            message.forwarded != previous.forwarded
            or message.show_forwarded_as_original != previous.show_forwarded_as_original
            or message.forwarded_from_id != previous.forwarded_from_id
            or message.forwarded_from_name != previous.forwarded_from_name
        ):
            return True
        limit = (
            1 if message.forwarded_from_id or message.forwarded_from_name else JOIN_WITHIN_SECONDS
        )
        return abs(message.date - previous.date) > limit

    def forwarded_needs_wrap(self, message: Message, previous: MessageInfo | None) -> bool:
        if self.message_needs_wrap(message, previous) or previous is None:
            return True
        if (
            not message.forwarded_from_id
            or message.forwarded_from_id != previous.forwarded_from_id
        ):
            return True
        if not peer_is_user(message.forwarded_from_id):
            return True
        return abs(message.forwarded_date - previous.forwarded_date) > JOIN_WITHIN_SECONDS

    def push_media(
        self,
        message: Message,
        base_path: str,
        peers: PeersMap,
        internal_links_domain: str,
        wrap_message_link: WrapMessageLink,
    ) -> str:
        data = prepare_media_data(message, peers, internal_links_domain)
        if data.classes:
            return self.push_generic_media(data)
        content = message.media.content
        if isinstance(content, Document):
            if content.is_sticker:
                return self.push_sticker_media(content, base_path)
            if content.is_animated:
                return self.push_animated_media(content, base_path)
            if content.is_video_file:
                return self.push_video_file_media(content, base_path)
            raise ValueError("Non generic document in HtmlWriter::Wrap::pushMedia.")
        if isinstance(content, Photo):
            return self.push_photo_media(content, base_path)
        if isinstance(content, Poll):
            return self.push_poll(content, internal_links_domain, self._base)
        if isinstance(content, TodoList):
            return self.push_todo_list(content, internal_links_domain, self._base)
        if isinstance(content, GiveawayStart):
            return self.push_giveaway_start(peers, content)
        if isinstance(content, GiveawayResults):
            return self.push_giveaway_results(peers, content, wrap_message_link)
        return ""
