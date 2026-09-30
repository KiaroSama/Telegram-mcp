"""Message media: the Media variant and every kind it can hold.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.{h,cpp}
Media, ParseMedia, ParseSharedContact/ParseGeoPoint/ParseVenue/ParseGame/ParseInvoice/
ParsePaidMedia/ParsePoll/ParseTodoList/ParseGiveaway), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Union

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .model import (
    Document,
    File,
    Image,
    ParseMediaContext,
    Photo,
    TextPart,
    parse_document,
    parse_photo,
    parse_text_with_entities,
    prepare_photo_file_name,
    to_time,
)
from .model_peers import ContactInfo


@dataclass
class SharedContact:
    info: ContactInfo = field(default_factory=ContactInfo)
    vcard: File = field(default_factory=File)


@dataclass
class GeoPoint:
    latitude: float = 0.0
    longitude: float = 0.0
    valid: bool = False


@dataclass
class Venue:
    point: GeoPoint = field(default_factory=GeoPoint)
    title: str = ""
    address: str = ""


@dataclass
class Game:
    id: int = 0
    short_name: str = ""
    title: str = ""
    description: str = ""
    bot_id: int = 0


@dataclass
class Invoice:
    title: str = ""
    description: str = ""
    currency: str = ""
    amount: int = 0
    receipt_msg_id: int = 0


@dataclass
class PaidMedia:
    stars: int = 0
    # A preview (not purchased) entry is None, like tdesktop's empty unique_ptr.
    extended: list[Media | None] = field(default_factory=list)


@dataclass
class Poll:
    @dataclass
    class Answer:
        text: list[TextPart] = field(default_factory=list)
        option: bytes = b""
        votes: int = 0
        my: bool = False

    id: int = 0
    question: list[TextPart] = field(default_factory=list)
    answers: list[Poll.Answer] = field(default_factory=list)
    total_votes: int = 0
    closed: bool = False


@dataclass
class TodoListItem:
    text: list[TextPart] = field(default_factory=list)
    id: int = 0


@dataclass
class TodoList:
    others_can_append: bool = False
    others_can_complete: bool = False
    title: list[TextPart] = field(default_factory=list)
    items: list[TodoListItem] = field(default_factory=list)


@dataclass
class GiveawayStart:
    countries: list[str] = field(default_factory=list)
    channels: list[int] = field(default_factory=list)
    additional_prize: str = ""
    until_date: int = 0
    credits: int = 0
    quantity: int = 0
    months: int = 0
    all: bool = False


@dataclass
class GiveawayResults:
    channel: int = 0
    winners: list[int] = field(default_factory=list)
    additional_prize: str = ""
    until_date: int = 0
    launch_id: int = 0
    additional_peers_count: int = 0
    winners_count: int = 0
    unclaimed_count: int = 0
    months: int = 0
    credits: int = 0
    refunded: bool = False
    all: bool = False


@dataclass
class UnsupportedMedia:
    pass


MediaContent = Union[
    None,
    Photo,
    Document,
    SharedContact,
    GeoPoint,
    Venue,
    Game,
    Invoice,
    Poll,
    TodoList,
    GiveawayStart,
    GiveawayResults,
    PaidMedia,
    UnsupportedMedia,
]


@dataclass
class Media:
    content: MediaContent = None
    ttl: int = 0

    def file(self) -> File:
        """Media::file(): a detached empty File for content without one."""
        content = self.content
        if isinstance(content, Photo):
            return content.image.file
        if isinstance(content, Document):
            return content.file
        if isinstance(content, SharedContact):
            return content.vcard
        return File()

    def thumb(self) -> Image:
        return self.content.thumb if isinstance(self.content, Document) else Image()

    def reset_file(self) -> None:
        """`media.file() = File()`, which C++ can write through the returned reference."""
        content = self.content
        if isinstance(content, Photo):
            content.image.file = File()
        elif isinstance(content, Document):
            content.file = File()
        elif isinstance(content, SharedContact):
            content.vcard = File()


def parse_shared_contact(
    context: ParseMediaContext, data: tl.MessageMediaContact, suggested_folder: str
) -> SharedContact:
    result = SharedContact()
    result.info.user_id = data.user_id
    result.info.first_name = data.first_name or ""
    result.info.last_name = data.last_name or ""
    result.info.phone_number = data.phone_number or ""
    if data.vcard:
        result.vcard.content = data.vcard.encode("utf-8")
        result.vcard.size = len(result.vcard.content)
        context.contacts += 1
        result.vcard.suggested_path = (
            f"{suggested_folder}contacts/contact_{context.contacts}.vcard"
        )
    return result


def parse_geo_point(data: Any) -> GeoPoint:
    if isinstance(data, tl.GeoPoint):
        return GeoPoint(latitude=data.lat, longitude=data.long, valid=True)
    return GeoPoint()


def parse_venue(data: tl.MessageMediaVenue) -> Venue:
    return Venue(point=parse_geo_point(data.geo), title=data.title, address=data.address)


def parse_game(data: tl.Game, bot_id: int) -> Game:
    return Game(
        id=data.id,
        title=data.title,
        description=data.description,
        short_name=data.short_name,
        bot_id=bot_id,
    )


def parse_invoice(data: tl.MessageMediaInvoice) -> Invoice:
    return Invoice(
        title=data.title,
        description=data.description,
        currency=data.currency,
        amount=data.total_amount,
        receipt_msg_id=data.receipt_msg_id or 0,
    )


def parse_paid_media(
    context: ParseMediaContext, data: tl.MessageMediaPaidMedia, folder: str, date: int
) -> PaidMedia:
    result = PaidMedia(stars=data.stars_amount)
    for extended in data.extended_media:
        if isinstance(extended, tl.MessageExtendedMedia):
            result.extended.append(parse_media(context, extended.media, folder, date))
        else:
            result.extended.append(None)
    return result


def parse_poll(data: tl.MessageMediaPoll) -> Poll:
    result = Poll()
    poll = data.poll
    if isinstance(poll, tl.Poll):
        result.id = poll.id
        result.question = parse_text_with_entities(poll.question)
        result.closed = bool(poll.closed)
        for answer in poll.answers:
            parsed = Poll.Answer()
            if isinstance(answer, tl.PollAnswer):
                parsed.text = parse_text_with_entities(answer.text)
                parsed.option = bytes(answer.option)
            result.answers.append(parsed)
    results = data.results
    if isinstance(results, tl.PollResults):
        if results.total_voters is not None:
            result.total_votes = results.total_voters
        for voters in results.results or []:
            match = next((a for a in result.answers if a.option == bytes(voters.option)), None)
            if match is None:
                continue
            if voters.voters is not None:
                match.votes = voters.voters
            if voters.chosen:
                match.my = True
    return result


def parse_todo_list_item(item: tl.TodoItem) -> TodoListItem:
    return TodoListItem(text=parse_text_with_entities(item.title), id=item.id)


def parse_todo_list(data: tl.MessageMediaToDo) -> TodoList:
    result = TodoList()
    todo = data.todo
    if isinstance(todo, tl.TodoList):
        result.title = parse_text_with_entities(todo.title)
        result.others_can_append = bool(todo.others_can_append)
        result.others_can_complete = bool(todo.others_can_complete)
        result.items = [parse_todo_list_item(item) for item in todo.list]
    return result


def parse_giveaway_start(data: tl.MessageMediaGiveaway) -> GiveawayStart:
    return GiveawayStart(
        until_date=to_time(data.until_date),
        credits=data.stars or 0,
        quantity=data.quantity,
        months=data.months or 0,
        all=not data.only_new_subscribers,
        channels=list(data.channels),
        countries=list(data.countries_iso2 or []),
        additional_prize=data.prize_description or "",
    )


def parse_giveaway_results(data: tl.MessageMediaGiveawayResults) -> GiveawayResults:
    return GiveawayResults(
        channel=data.channel_id,
        until_date=to_time(data.until_date),
        launch_id=data.launch_msg_id,
        additional_peers_count=data.additional_peers_count or 0,
        winners_count=data.winners_count,
        unclaimed_count=data.unclaimed_count,
        months=data.months or 0,
        credits=data.stars or 0,
        refunded=bool(data.refunded),
        all=not data.only_new_subscribers,
        winners=list(data.winners),
        additional_prize=data.prize_description or "",
    )


def parse_media(context: ParseMediaContext, data: Any, folder: str, date: int) -> Media:
    """Data::ParseMedia. `folder` is "" or ends with '/'."""
    result = Media()
    if isinstance(data, tl.MessageMediaPhoto):
        photo = Photo()
        if data.photo is not None:
            context.photos += 1
            name = prepare_photo_file_name(context.photos, date)
            photo = parse_photo(data.photo, folder + "photos/" + name)
        photo.spoilered = bool(data.spoiler)
        if data.ttl_seconds is not None:
            result.ttl = data.ttl_seconds
            photo.image.file = File()
        result.content = photo
    elif isinstance(data, tl.MessageMediaGeo):
        result.content = parse_geo_point(data.geo)
    elif isinstance(data, tl.MessageMediaContact):
        result.content = parse_shared_contact(context, data, folder)
    elif isinstance(data, tl.MessageMediaUnsupported):
        result.content = UnsupportedMedia()
    elif isinstance(data, tl.MessageMediaDocument):
        document = (
            parse_document(context, data.document, folder, date)
            if data.document is not None
            else Document()
        )
        if data.ttl_seconds is not None:
            result.ttl = data.ttl_seconds
            document.file = File()
        document.spoilered = bool(data.spoiler)
        result.content = document
    elif isinstance(data, tl.MessageMediaVenue):
        result.content = parse_venue(data)
    elif isinstance(data, tl.MessageMediaGame):
        result.content = parse_game(data.game, context.bot_id)
    elif isinstance(data, tl.MessageMediaInvoice):
        result.content = parse_invoice(data)
    elif isinstance(data, tl.MessageMediaGeoLive):
        result.content = parse_geo_point(data.geo)
        result.ttl = data.period
    elif isinstance(data, tl.MessageMediaPoll):
        result.content = parse_poll(data)
    elif isinstance(data, tl.MessageMediaToDo):
        result.content = parse_todo_list(data)
    elif isinstance(data, tl.MessageMediaGiveaway):
        result.content = parse_giveaway_start(data)
    elif isinstance(data, tl.MessageMediaGiveawayResults):
        result.content = parse_giveaway_results(data)
    elif isinstance(data, tl.MessageMediaPaidMedia):
        result.content = parse_paid_media(context, data, folder, date)
    # WebPage, Dice, Story, VideoStream and Empty stay null, as in tdesktop.
    return result
