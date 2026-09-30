"""JSON for one exported message: header fields, media, text, inline buttons and reactions.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_json.cpp
SerializeMessage), GPL-3.0.
"""

from __future__ import annotations

from typing import Any

from .json_actions import MessageValues, push_service_action, serialize_todo_items
from .json_rich import base64_url, serialize_rich_message
from .json_serialize import (
    K_ARRAY,
    K_OBJECT,
    JsonContext,
    format_username,
    number,
    serialize_array,
    serialize_date,
    serialize_date_raw,
    serialize_object,
    serialize_string,
    serialize_text,
    string_allow_null,
)
from .model import Document, Photo
from .model_format import format_phone_number
from .model_media import (
    Game,
    GeoPoint,
    GiveawayResults,
    GiveawayStart,
    Invoice,
    PaidMedia,
    Poll,
    SharedContact,
    TodoList,
    UnsupportedMedia,
    Venue,
)
from .model_message import HistoryMessageMarkupButton, Message, Reaction
from .model_peers import Peer


def _bool(value: bool) -> bytes:
    return b"true" if value else b"false"


def _location(context: JsonContext, point: GeoPoint) -> bytes:
    return serialize_object(
        context, [("latitude", number(point.latitude)), ("longitude", number(point.longitude))]
    )


def _push_document(v: MessageValues, data: Document) -> None:
    v.push_path(data.file, "file")
    v.string("file_name", data.name)
    v.number("file_size", data.file.size)
    if data.thumb.width > 0:
        v.push_path(data.thumb.file, "thumbnail")
        v.number("thumbnail_file_size", data.thumb.file.size)
    if data.is_sticker:
        v.string("media_type", "sticker")
        v.string("sticker_emoji", data.sticker_emoji)
    elif data.is_video_message:
        v.string("media_type", "video_message")
    elif data.is_voice_message:
        v.string("media_type", "voice_message")
    elif data.is_animated:
        v.string("media_type", "animation")
    elif data.is_video_file:
        v.string("media_type", "video_file")
    elif data.is_audio_file:
        v.string("media_type", "audio_file")
        v.string("performer", data.song_performer)
        v.string("title", data.song_title)
    v.string("mime_type", data.mime)
    if data.duration:
        v.number("duration_seconds", data.duration)
    if data.width and data.height:
        v.number("width", data.width)
        v.number("height", data.height)
    v.push_spoiler(data)
    v.push_ttl()


def _poll(context: JsonContext, data: Poll) -> bytes:
    context.nesting.append(K_OBJECT)
    answers = []
    for answer in data.answers:
        context.nesting.append(K_ARRAY)
        answers.append(
            serialize_object(
                context,
                [
                    ("text", serialize_text(context, answer.text)),
                    ("voters", number(answer.votes)),
                    ("chosen", _bool(answer.my)),
                ],
            )
        )
        context.nesting.pop()
    serialized = serialize_array(context, answers)
    context.nesting.pop()
    return serialize_object(
        context,
        [
            ("question", serialize_text(context, data.question)),
            ("closed", _bool(data.closed)),
            ("total_voters", number(data.total_votes)),
            ("answers", serialized),
        ],
    )


def _todo_list(context: JsonContext, data: TodoList) -> bytes:
    context.nesting.append(K_OBJECT)
    serialized = serialize_todo_items(context, data.items)
    context.nesting.pop()
    return serialize_object(
        context,
        [
            ("title", serialize_text(context, data.title)),
            ("others_can_append", _bool(data.others_can_append)),
            ("others_can_complete", _bool(data.others_can_complete)),
            ("answers", serialized),
        ],
    )


def _nested_array(context: JsonContext, values: list[bytes]) -> bytes:
    context.nesting.append(K_ARRAY)
    result = serialize_array(context, values)
    context.nesting.pop()
    return result


def _giveaway_start(context: JsonContext, data: GiveawayStart) -> bytes:
    channels = _nested_array(context, [number(channel) for channel in data.channels])
    countries = _nested_array(context, [serialize_string(code) for code in data.countries])
    return serialize_object(
        context,
        [
            ("quantity", number(data.quantity)),
            ("months", number(data.months)),
            ("until_date", serialize_date(data.until_date)),
            ("channels", channels),
            ("countries", countries),
            ("additional_prize", serialize_string(data.additional_prize)),
            ("stars", number(data.credits)),
            ("is_only_new_subscribers", _bool(not data.all)),
        ],
    )


def _giveaway_results(context: JsonContext, data: GiveawayResults) -> bytes:
    winners = _nested_array(context, [number(winner) for winner in data.winners])
    return serialize_object(
        context,
        [
            ("channel", number(data.channel)),
            ("winners", winners),
            ("additional_prize", serialize_string(data.additional_prize)),
            ("until_date", serialize_date(data.until_date)),
            ("launch_message_id", number(data.launch_id)),
            ("additional_peers_count", number(data.additional_peers_count)),
            ("winners_count", number(data.winners_count)),
            ("unclaimed_count", number(data.unclaimed_count)),
            ("months", number(data.months)),
            ("stars", number(data.credits)),
            ("is_refunded", _bool(data.refunded)),
            ("is_only_new_subscribers", _bool(not data.all)),
        ],
    )


def _push_media(v: MessageValues, content: Any, internal_links_domain: str) -> None:
    context = v.context
    if isinstance(content, Photo):
        v.push_photo(content.image)
        v.push_spoiler(content)
        v.push_ttl()
    elif isinstance(content, Document):
        _push_document(v, content)
    elif isinstance(content, SharedContact):
        info = content.info
        contact = [
            ("first_name", serialize_string(info.first_name)),
            ("last_name", serialize_string(info.last_name)),
            ("phone_number", serialize_string(format_phone_number(info.phone_number))),
        ]
        v.bare("contact_information", serialize_object(context, contact))
        if content.vcard.content:
            v.push_path(content.vcard, "contact_vcard")
            v.number("contact_vcard_file_size", content.vcard.size)
    elif isinstance(content, GeoPoint):
        location = _location(context, content) if content.valid else b"null"
        v.bare("location_information", location)
        v.push_ttl("live_location_period_seconds")
    elif isinstance(content, Venue):
        v.string("place_name", content.title)
        v.string("address", content.address)
        if content.point.valid:
            v.bare("location_information", _location(context, content.point))
    elif isinstance(content, Game):
        v.string("game_title", content.title)
        v.string("game_description", content.description)
        if content.bot_id != 0 and content.short_name:
            bot = v.user_of(content.bot_id)
            if bot.is_bot and bot.username:
                link = internal_links_domain + bot.username + "?game=" + content.short_name
                v.string("game_link", link)
    elif isinstance(content, Invoice):
        receipt = number(content.receipt_msg_id) if content.receipt_msg_id else b""
        invoice = [
            ("title", serialize_string(content.title)),
            ("description", serialize_string(content.description)),
            ("amount", number(content.amount)),
            ("currency", serialize_string(content.currency)),
            ("receipt_message_id", receipt),
        ]
        # push() of a QByteArray: the serialized object lands in the file as a string.
        v.string("invoice_information", serialize_object(context, invoice))
    elif isinstance(content, Poll):
        v.bare("poll", _poll(context, content))
    elif isinstance(content, TodoList):
        v.bare("todo_list", _todo_list(context, content))
    elif isinstance(content, GiveawayStart):
        v.bare("giveaway_information", _giveaway_start(context, content))
    elif isinstance(content, GiveawayResults):
        v.bare("giveaway_results", _giveaway_results(context, content))
    elif isinstance(content, PaidMedia):
        v.number("paid_stars_amount", content.stars)


def _button(context: JsonContext, entry: HistoryMessageMarkupButton) -> bytes:
    pairs = [("type", serialize_string(HistoryMessageMarkupButton.type_to_string(entry)))]
    if entry.text:
        pairs.append(("text", serialize_string(entry.text)))
    if entry.data:
        kind = HistoryMessageMarkupButton.Type
        if entry.type in (kind.Callback, kind.CallbackWithPassword):
            pairs.append(("dataBase64", serialize_string(base64_url(entry.data))))
            pairs.append(("data", serialize_string(b"")))
        else:
            pairs.append(("data", serialize_string(entry.data)))
    if entry.forward_text:
        pairs.append(("forward_text", serialize_string(entry.forward_text)))
    if entry.button_id:
        pairs.append(("button_id", number(entry.button_id)))
    return serialize_object(context, pairs)


def _inline_buttons(context: JsonContext, rows: list[list[HistoryMessageMarkupButton]]) -> bytes:
    context.nesting.append(K_ARRAY)
    serialized = []
    for row in rows:
        context.nesting.append(K_ARRAY)
        buttons = [_button(context, entry) for entry in row]
        context.nesting.pop()
        serialized.append(serialize_array(context, buttons))
    context.nesting.pop()
    return serialize_array(context, serialized)


def _reaction(v: MessageValues, reaction: Reaction) -> bytes:
    context = v.context
    context.nesting.append(K_OBJECT)
    try:
        pairs = [
            ("type", serialize_string(Reaction.type_to_string(reaction))),
            ("count", number(reaction.count)),
        ]
        if reaction.type == Reaction.Type.Emoji:
            pairs.append(("emoji", serialize_string(reaction.emoji)))
        elif reaction.type == Reaction.Type.CustomEmoji:
            pairs.append(("document_id", serialize_string(reaction.document_id)))
        if reaction.recent:
            context.nesting.append(K_ARRAY)
            recents = []
            for recent in reaction.recent:
                context.nesting.append(K_ARRAY)
                recents.append(
                    serialize_object(
                        context,
                        [
                            ("from", v.wrap_peer_name(recent.peer_id)),
                            ("from_id", v.wrap_peer_id(recent.peer_id)),
                            ("date", serialize_date(recent.date)),
                        ],
                    )
                )
                context.nesting.pop()
            pairs.append(("recent", serialize_array(context, recents)))
            context.nesting.pop()
        # The guard pops only after this object is serialized, one level deeper than its key.
        return serialize_object(context, pairs)
    finally:
        context.nesting.pop()


def serialize_message(
    context: JsonContext,
    message: Message,
    peers: dict[int, Peer],
    internal_links_domain: str,
) -> bytes:
    """SerializeMessage."""
    if isinstance(message.media.content, UnsupportedMedia) and message.rich_message is None:
        return serialize_object(
            context, [("id", number(message.id)), ("type", serialize_string("unsupported"))]
        )
    v = MessageValues(context, message, peers)
    v.values = [
        ("id", number(message.id)),
        ("type", serialize_string("service" if message.action.content is not None else "message")),
        ("date", serialize_date(message.date)),
        ("date_unixtime", serialize_date_raw(message.date)),
    ]
    context.nesting.append(K_OBJECT)
    try:
        _fill_message(v, message, internal_links_domain)
    finally:
        context.nesting.pop()
    return serialize_object(context, v.values)


def _fill_message(v: MessageValues, message: Message, internal_links_domain: str) -> None:
    context = v.context
    if message.edited:
        v.bare("edited", serialize_date(message.edited))
        v.bare("edited_unixtime", serialize_date_raw(message.edited))

    push_service_action(v, message.action.content)

    if message.action.content is None:
        v.push_from()
        v.string("author", message.signature)
        if message.forwarded_from_id:
            v.bare("forwarded_from", v.wrap_peer_name(message.forwarded_from_id))
            v.peer("forwarded_from_id", message.forwarded_from_id)
        elif message.forwarded_from_name:
            v.bare("forwarded_from", string_allow_null(message.forwarded_from_name))
        if message.saved_from_chat_id:
            v.bare("saved_from", v.wrap_peer_name(message.saved_from_chat_id))
        v.push_reply_to_msg_id()
        if message.via_bot_id:
            username = format_username(v.user_of(message.via_bot_id).username)
            if username:
                v.string("via_bot", username)

    _push_media(v, message.media.content, internal_links_domain)

    if message.rich_message is not None:
        v.bare("rich_message", serialize_rich_message(context, message.rich_message))
    else:
        v.bare("text", serialize_text(context, message.text))
        v.bare("text_entities", serialize_text(context, message.text, True))

    if message.inline_button_rows:
        v.bare("inline_bot_buttons", _inline_buttons(context, message.inline_button_rows))

    if message.reactions:
        context.nesting.append(K_ARRAY)
        reactions = [_reaction(v, reaction) for reaction in message.reactions]
        v.bare("reactions", serialize_array(context, reactions))
        context.nesting.pop()
