"""HTML escaping, text entities, dates, the tag stack, peers and userpics for the HTML export.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
SerializeString, SerializeList, FormatTextLink, FormatCustomEmoji, FormatText, DisplayDate,
FormatDateText, FormatTimeText, details::HtmlContext, details::UserpicData, details::PeersMap,
FillUserpicNames, ComposeName, HtmlWriter::Wrap::pushTag/popTag/pushDiv/pushUserpic/relativePath),
GPL-3.0.

tdesktop builds bytes; this port builds `str`. Where tdesktop cuts a UTF-8 sequence in half
(QByteArray::mid), the loose bytes are kept as surrogate escapes and the writer encodes the file
with "surrogateescape", so the bytes on disk stay the same.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime

from . import model_format
from .html_safe import qt_trimmed, safe_message_href
from .model import TextPart, peer_from_user
from .model_format import number_to_string
from .model_peers import Peer, User

LINE_BREAK = "<br>"

_SERIALIZE = {
    ord("\n"): "<br>",
    ord('"'): "&quot;",
    ord("&"): "&amp;",
    ord("'"): "&apos;",
    ord("<"): "&lt;",
    ord(">"): "&gt;",
    0x2028: "<br>",
    0x2029: "<br>",
}
for _code in range(32):
    if _code != ord("\n"):
        _SERIALIZE[_code] = f"&#x{_code >> 4}{_code & 0xF:X};"


def serialize_string(value: str) -> str:
    return value.translate(_SERIALIZE)


def serialize_list(values: list[str]) -> str:
    if len(values) == 1:
        return values[0]
    if len(values) > 1:
        return ", ".join(values[:-1]) + " and " + values[-1]
    return ""


def byte_mid(value: str, position: int) -> str:
    """QByteArray::mid(position) on the UTF-8 bytes of `value`."""
    data = value.encode("utf-8", "surrogateescape")
    return data[position:].decode("utf-8", "surrogateescape")


def format_text_link(text: str, target: str) -> str:
    href = safe_message_href(target)
    return f'<a href="{serialize_string(href)}">{text}</a>' if href is not None else text


def format_custom_emoji(custom_emoji: str, text: str, relative_link_base: str) -> str:
    if not custom_emoji:
        start = '<a href="" onclick="return ShowNotLoadedEmoji();">'
    elif custom_emoji == TextPart.unavailable_emoji():
        start = '<a href="" onclick="return ShowNotAvailableEmoji();">'
    else:
        start = '<a href = "' + relative_link_base + custom_emoji + '">'
    return start + text + "</a>"


def _format_part(part: TextPart, internal_links_domain: str, relative_link_base: str) -> str:
    text = serialize_string(part.text)
    kind = TextPart.Type
    if part.type in (kind.Text, kind.Unknown, kind.BankCard):
        return text
    if part.type == kind.Mention:
        return f'<a href="{internal_links_domain}{byte_mid(text, 1)}">{text}</a>'
    if part.type in (kind.Hashtag, kind.BotCommand, kind.Cashtag):
        function = {
            kind.Hashtag: "ShowHashtag",
            kind.BotCommand: "ShowBotCommand",
            kind.Cashtag: "ShowCashtag",
        }[part.type]
        argument = serialize_string('"' + byte_mid(text, 1) + '"')
        return f'<a href="" onclick="return {function}({argument})">{text}</a>'
    if part.type == kind.Url:
        return format_text_link(text, part.text)
    if part.type == kind.Email:
        return f'<a href="mailto:{text}">{text}</a>'
    if part.type == kind.TextUrl:
        return format_text_link(text, part.additional)
    if part.type == kind.MentionName:
        return f'<a href="" onclick="return ShowMentionName()">{text}</a>'
    if part.type == kind.Phone:
        return f'<a href="tel:{text}">{text}</a>'
    if part.type == kind.Spoiler:
        return (
            '<span class="spoiler hidden" onclick="ShowSpoiler(this)">'
            f'<span aria-hidden="true">{text}</span></span>'
        )
    if part.type == kind.CustomEmoji:
        return format_custom_emoji(part.additional, text, relative_link_base)
    tag = {
        kind.Bold: "strong",
        kind.Italic: "em",
        kind.Code: "code",
        kind.Pre: "pre",
        kind.Underline: "u",
        kind.Strike: "s",
        kind.Blockquote: "blockquote",
    }[part.type]
    return f"<{tag}>{text}</{tag}>"


def format_text(data: list[TextPart], internal_links_domain: str, relative_link_base: str) -> str:
    return "".join(_format_part(part, internal_links_domain, relative_link_base) for part in data)


def local_datetime(date: int) -> datetime:
    """QDateTime::fromSecsSinceEpoch: the local zone, pinned by model_format in tests."""
    zone = model_format.LOCAL_TIMEZONE
    return datetime.fromtimestamp(date, zone) if zone is not None else datetime.fromtimestamp(date)


def display_date(date: int, previous_date: int) -> bool:
    if not previous_date:
        return True
    return local_datetime(date).date() != local_datetime(previous_date).date()


_MONTHS = (
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
)


def month_name(index: int) -> str:
    return _MONTHS[index - 1] if 1 <= index <= 12 else "Unknown"


def format_date_text(date: int) -> str:
    parsed = local_datetime(date)
    return f"{parsed.day} {month_name(parsed.month)} {parsed.year}"


def format_time_text(date: int) -> str:
    parsed = local_datetime(date)
    return number_to_string(parsed.hour, 2) + ":" + number_to_string(parsed.minute, 2)


def iso_date_time(date: int) -> str:
    """QDateTime::fromSecsSinceEpoch(date).toString(Qt::ISODate) (local time, no zone)."""
    value = local_datetime(date)
    return (
        f"{value.year:04d}-{value.month:02d}-{value.day:02d}"
        f"T{value.hour:02d}:{value.minute:02d}:{value.second:02d}"
    )


class HtmlContext:
    """details::HtmlContext: the open-tag stack that indents block tags."""

    def __init__(self) -> None:
        self._tags: list[tuple[str, bool]] = []

    def push_tag(self, tag: str, attributes: dict[str, str] | None = None) -> str:
        block, empty, inner = True, False, ""
        # std::map iterates its keys in byte order.
        for name in sorted(attributes or {}):
            if name == "inline":
                block = False
            elif name == "empty":
                empty = True
            else:
                inner += f' {name}="{serialize_string((attributes or {})[name])}"'
        result = (
            ("\n" + self.indent() if block else "")
            + f"<{tag}{inner}{'/' if empty else ''}>"
            + ("\n" if block else "")
        )
        if not empty:
            self._tags.append((tag, block))
        return result

    def pop_tag(self) -> str:
        tag, block = self._tags.pop()
        return ("\n" + self.indent() if block else "") + f"</{tag}>" + ("\n" if block else "")

    def indent(self) -> str:
        return " " * len(self._tags)

    def empty(self) -> bool:
        return not self._tags


@dataclass
class UserpicData:
    color_index: int = 0
    pixel_size: int = 0
    image_link: str = ""
    large_link: str = ""
    first_name: str = ""
    last_name: str = ""
    tooltip: str = ""


@dataclass
class StoryData:
    image_link: str = ""
    large_link: str = ""


@dataclass
class PeersMap:
    data: dict[int, Peer] = field(default_factory=dict)

    def peer(self, peer_id: int) -> Peer:
        found = self.data.get(peer_id)
        return found if found is not None else Peer(User())

    def user(self, user_id: int) -> User:
        found = self.peer(peer_from_user(user_id)).user()
        return found if found is not None else User()

    def wrap_peer_name(self, peer_id: int) -> str:
        result = self.peer(peer_id).name()
        return serialize_string(result) if result else "Deleted"

    def wrap_user_name(self, user_id: int) -> str:
        result = self.user(user_id).name()
        return serialize_string(result) if result else "Deleted Account"

    def wrap_user_names(self, data: list[int]) -> str:
        return serialize_list([self.wrap_user_name(user_id) for user_id in data])


def fill_userpic_names_from_peer(data: UserpicData, peer: Peer) -> None:
    user = peer.user()
    if user is not None:
        data.first_name = user.info.first_name
        data.last_name = user.info.last_name
    elif peer.chat() is not None:
        data.first_name = peer.name()


def fill_userpic_names_from_full(data: UserpicData, full: str) -> None:
    names = full.split(" ")
    data.first_name = names[0]
    for name in names[1:]:
        if not name:
            continue
        if data.last_name:
            data.last_name += " "
        data.last_name += name


def compose_name(data: UserpicData, empty: str) -> str:
    if not data.first_name and not data.last_name:
        return empty
    return data.first_name + " " + data.last_name


def _initial(value: str) -> str:
    trimmed = qt_trimmed(value)
    if not trimmed:
        return ""
    # QString::mid(0, 1) keeps one UTF-16 unit; Qt 5 writes a lone surrogate as '?'.
    return serialize_string(trimmed[0] if ord(trimmed[0]) <= 0xFFFF else "?")


class WrapBase:
    """The tag helpers of HtmlWriter::Wrap that every block renderer shares."""

    def __init__(self, base: str) -> None:
        self._base = base
        self._context = HtmlContext()

    def push_tag(self, tag: str, attributes: dict[str, str] | None = None) -> str:
        return self._context.push_tag(tag, attributes)

    def pop_tag(self) -> str:
        return self._context.pop_tag()

    def indent(self) -> str:
        return self._context.indent()

    def push_div(self, class_name: str, style: str = "") -> str:
        if not style:
            return self.push_tag("div", {"class": class_name})
        return self.push_tag("div", {"class": class_name, "style": style})

    def relative_path(self, path: str) -> str:
        return self._base + path

    def push_userpic(self, userpic: UserpicData) -> str:
        size = number_to_string(userpic.pixel_size) + "px"
        result = ""
        if userpic.large_link:
            result += self.push_tag(
                "a", {"class": "userpic_link", "href": self.relative_path(userpic.large_link)}
            )
        size_style = "width: " + size + "; height: " + size
        if userpic.image_link:
            result += self.push_tag(
                "img",
                {
                    "class": "userpic",
                    "style": size_style,
                    "src": self.relative_path(userpic.image_link),
                    "empty": "",
                },
            )
        else:
            result += self.push_tag(
                "div",
                {
                    "class": "userpic userpic" + number_to_string(userpic.color_index + 1),
                    "style": size_style,
                },
            )
            if not userpic.tooltip:
                result += self.push_div("initials", "line-height: " + size)
            else:
                result += self.push_tag(
                    "div",
                    {
                        "class": "initials",
                        "style": "line-height: " + size,
                        "title": userpic.tooltip,
                    },
                )
            result += _initial(userpic.first_name)
            result += _initial(userpic.last_name)
            result += self.pop_tag()
            result += self.pop_tag()
        if userpic.large_link:
            result += self.pop_tag()
        return result
