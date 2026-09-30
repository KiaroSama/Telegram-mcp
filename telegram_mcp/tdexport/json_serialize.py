"""JSON primitives of the export writer: strings, dates, objects, arrays and message text.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_json.cpp
SerializeString/SerializeDate/SerializeObject/SerializeArray/SerializeText and friends,
export_output_json.h JsonContext), GPL-3.0.

Everything is built as UTF-8 `bytes`, like tdesktop's QByteArray, so the output matches it byte
for byte, including the bytes SerializeString passes through unchanged.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Sequence

from .model import File, TextPart
from .model_format import _local_datetime, number_to_string

K_OBJECT = True
K_ARRAY = False

_U64 = 0xFFFFFFFFFFFFFFFF

Pairs = list[tuple[str, bytes]]


@dataclass
class JsonContext:
    """details::JsonContext: one entry per open object (True) or array (False)."""

    nesting: list[bool] = field(default_factory=list)


def utf8(value: str | bytes) -> bytes:
    """QString::toUtf8: an unpaired surrogate becomes U+FFFD."""
    if isinstance(value, bytes):
        return value
    try:
        return value.encode("utf-8")
    except UnicodeEncodeError:
        fixed = value.encode("utf-16-le", "surrogatepass").decode("utf-16-le", "replace")
        return fixed.encode("utf-8")


def number(value: int | float) -> bytes:
    """Data::NumberToString as bytes."""
    return number_to_string(value).encode("ascii")


def u64(value: int) -> int:
    """A C++ uint64 field printed from a Telethon signed `long`."""
    return value & _U64


_ESCAPED = re.compile(rb'[\x00-\x1f"\\]|\xe2\x80[\xa8\xa9]')
_SIMPLE = {b"\n": b"\\n", b"\r": b"\\r", b"\t": b"\\t", b'"': b'\\"', b"\\": b"\\\\"}
_HEX = b"0123456789ABCDEF"


def _escape(match: re.Match[bytes]) -> bytes:
    found = match.group()
    if len(found) == 3:
        # tdesktop writes the escape for the lead byte and then copies the two continuation
        # bytes as they are, so the result keeps them after the escape.
        return (b"\\u2028" if found[2] == 0xA8 else b"\\u2029") + found[1:]
    if found in _SIMPLE:
        return _SIMPLE[found]
    code = found[0]
    return b"\\x" + bytes((_HEX[code >> 4], _HEX[code & 0x0F]))


def serialize_string(value: str | bytes) -> bytes:
    """SerializeString: tdesktop's own escaping, not JSON's (\\xNN for control bytes)."""
    return b'"' + _ESCAPED.sub(_escape, utf8(value)) + b'"'


def serialize_date(date: int) -> bytes:
    """SerializeDate: QDateTime::fromSecsSinceEpoch(date).toString(Qt::ISODate), local time."""
    value = _local_datetime(date)
    return serialize_string(
        f"{value.year:04d}-{value.month:02d}-{value.day:02d}"
        f"T{value.hour:02d}:{value.minute:02d}:{value.second:02d}"
    )


def serialize_date_raw(date: int) -> bytes:
    return serialize_string(number(date))


def string_allow_empty(data: str) -> bytes:
    return serialize_string(data) if data else b""


def string_allow_null(data: str) -> bytes:
    return serialize_string(data) if data else b"null"


def indentation(context: JsonContext) -> bytes:
    return b" " * len(context.nesting)


def serialize_object(context: JsonContext, values: Sequence[tuple[str, bytes]]) -> bytes:
    """SerializeObject: pairs with an empty value are left out."""
    indent = b" " * len(context.nesting)
    after = b"\n" + b" " * (len(context.nesting) + 1)
    items = [after + serialize_string(key) + b": " + value for key, value in values if value]
    return b"{" + b",".join(items) + b"\n" + indent + b"}"


def serialize_array(context: JsonContext, values: Sequence[bytes]) -> bytes:
    indent = b" " * len(context.nesting)
    after = b"\n" + b" " * (len(context.nesting) + 1)
    return b"[" + b",".join(after + value for value in values) + b"\n" + indent + b"]"


_T = TextPart.Type
_TEXT_TYPE_NAMES = {
    _T.Unknown: "unknown",
    _T.Mention: "mention",
    _T.Hashtag: "hashtag",
    _T.BotCommand: "bot_command",
    _T.Url: "link",
    _T.Email: "email",
    _T.Bold: "bold",
    _T.Italic: "italic",
    _T.Code: "code",
    _T.Pre: "pre",
    _T.Text: "plain",
    _T.TextUrl: "text_link",
    _T.MentionName: "mention_name",
    _T.Phone: "phone",
    _T.Cashtag: "cashtag",
    _T.Underline: "underline",
    _T.Strike: "strikethrough",
    _T.Blockquote: "blockquote",
    _T.BankCard: "bank_card",
    _T.Spoiler: "spoiler",
    _T.CustomEmoji: "custom_emoji",
}
_ADDITIONAL_NAMES = {
    _T.MentionName: "user_id",
    _T.CustomEmoji: "document_id",
    _T.Pre: "language",
    _T.TextUrl: "href",
    _T.Blockquote: "collapsed",
}


def _additional_value(part: TextPart) -> bytes:
    if part.type == _T.MentionName:
        return utf8(part.additional)
    if part.type in (_T.Pre, _T.TextUrl, _T.CustomEmoji):
        return serialize_string(part.additional)
    if part.type == _T.Blockquote:
        return b"true" if part.additional else b"false"
    return b""


def serialize_text(
    context: JsonContext, data: list[TextPart], serialize_to_objects: bool = False
) -> bytes:
    """SerializeText: a lone plain part becomes a string, anything else an array."""
    if not data:
        return b"[]" if serialize_to_objects else serialize_string("")
    context.nesting.append(K_ARRAY)
    text = [
        (
            serialize_string(part.text)
            if part.type == _T.Text and not serialize_to_objects
            else serialize_object(
                context,
                [
                    ("type", serialize_string(_TEXT_TYPE_NAMES[part.type])),
                    ("text", serialize_string(part.text)),
                    (_ADDITIONAL_NAMES.get(part.type, "none"), _additional_value(part)),
                ],
            )
        )
        for part in data
    ]
    context.nesting.pop()
    if not serialize_to_objects and len(data) == 1 and data[0].type == _T.Text:
        return text[0]
    return serialize_array(context, text)


def format_username(username: str) -> str:
    return "@" + username if username else username


def format_file_path(file: File) -> bytes:
    return utf8(file.relative_path)
