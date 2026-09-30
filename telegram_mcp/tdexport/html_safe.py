"""Target validation for links written by the HTML export.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
IsGlobalLink, IsStrictTarget, HasEncodedControl, SafeMessageHref, SafeHttpHref, SafeEmailHref,
SafePhoneHref, SafeMentionHref, IsAsciiWord, IsUnicodeHashtagWord, Is*Action, SafeActionData,
IsAsciiDecimal, SafeRelativeEmojiHref, AnchorToken), GPL-3.0. QByteArray values are `str`.
"""

from __future__ import annotations

import base64
import hashlib
import unicodedata

from .html_qurl import parse_strict, to_encoded
from .model import TextPart

RICH_TARGET_MAX_BYTES = 4096
RICH_ANCHOR_DIRECT_BYTES = 72
RICH_ANCHOR_PREFIX_BYTES = 48
RICH_HASHTAG_MIN_LENGTH = 2
RICH_HASHTAG_MAX_LENGTH = 64
RICH_COMMAND_MAX_LENGTH = 64
RICH_CASHTAG_MAX_LENGTH = 8
RICH_USERNAME_MIN_LENGTH = 5
RICH_USERNAME_MAX_LENGTH = 32

_ALLOWED_SCHEMES = ("http", "https", "mailto", "tel", "tg")
_HOST_SCHEMES = ("http", "https")


def utf8_size(value: str) -> int:
    """QByteArray::size() of the UTF-8 form (surrogate escapes stand for single bytes)."""
    return len(value.encode("utf-8", "surrogateescape"))


def is_global_link(link: str) -> bool:
    lowered = link[:8].lower()
    return lowered.startswith("http://") or lowered.startswith("https://")


def qt_is_space(ch: str) -> bool:
    """QChar::isSpace (Qt 5.15)."""
    code = ord(ch)
    if code == 0x20 or 0x09 <= code <= 0x0D:
        return True
    if code < 128:
        return False
    return code in (0x85, 0xA0) or unicodedata.category(ch) in ("Zs", "Zl", "Zp")


def qt_trimmed(value: str) -> str:
    """QString::trimmed."""
    start, end = 0, len(value)
    while start < end and qt_is_space(value[start]):
        start += 1
    while end > start and qt_is_space(value[end - 1]):
        end -= 1
    return value[start:end]


def _hex_value(ch: str) -> int:
    if "0" <= ch <= "9":
        return ord(ch) - 0x30
    if "a" <= ch <= "f":
        return ord(ch) - 0x61 + 10
    if "A" <= ch <= "F":
        return ord(ch) - 0x41 + 10
    return -1


def has_encoded_control(value: str) -> bool:
    for index in range(len(value) - 2):
        if value[index] != "%":
            continue
        high, low = _hex_value(value[index + 1]), _hex_value(value[index + 2])
        if high < 0 or low < 0:
            continue
        decoded = (high << 4) | low
        if decoded < 32 or decoded == 127:
            return True
    return False


def is_strict_target(value: str) -> bool:
    if not value or utf8_size(value) > RICH_TARGET_MAX_BYTES:
        return False
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        return False
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        return False
    if qt_trimmed(value) != value:
        return False
    return all(unicodedata.category(ch) not in ("Cc", "Zl", "Zp") for ch in value)


def safe_message_href(value: str) -> str | None:
    if not is_strict_target(value) or value.startswith("//") or has_encoded_control(value):
        return None
    url = parse_strict(value)
    if url.scheme is None:
        url = parse_strict("https://" + value)
    scheme = url.scheme or ""
    if (
        not url.valid
        or url.scheme is None
        or scheme not in _ALLOWED_SCHEMES
        or (scheme in _HOST_SCHEMES and not url.host)
    ):
        return None
    result = to_encoded(url)
    return result if is_strict_target(result) and not has_encoded_control(result) else None


def safe_http_href(value: str) -> str | None:
    if not is_strict_target(value) or value.startswith("//") or has_encoded_control(value):
        return None
    url = parse_strict(value)
    if not url.valid or url.scheme is None or not url.host or url.scheme not in _HOST_SCHEMES:
        return None
    result = to_encoded(url)
    return result if is_strict_target(result) and not has_encoded_control(result) else None


def safe_email_href(address: str) -> str | None:
    if not is_strict_target(address):
        return None
    separator = address.find("@")
    if separator <= 0 or separator != address.rfind("@") or separator + 1 == len(address):
        return None
    if any(qt_is_space(ch) for ch in address):
        return None
    if any(forbidden in address for forbidden in "/\\:%?#&="):
        return None
    result = "mailto:" + address
    return result if is_strict_target(result) else None


def safe_phone_href(phone: str) -> str | None:
    if not is_strict_target(phone):
        return None
    has_digit = False
    for ch in phone:
        if "0" <= ch <= "9":
            has_digit = True
        elif ch not in "+- .()":
            return None
    result = "tel:" + phone
    return result if has_digit and is_strict_target(result) else None


def _is_word_char(ch: str) -> bool:
    return "a" <= ch <= "z" or "A" <= ch <= "Z" or "0" <= ch <= "9" or ch == "_"


def safe_mention_href(mention: str, internal_links_domain: str) -> str | None:
    if not is_strict_target(mention) or utf8_size(mention) < 2 or mention[0] != "@":
        return None
    username = mention[1:]
    if not all(_is_word_char(ch) for ch in username):
        return None
    return safe_http_href(internal_links_domain + username)


def is_ascii_word(value: str, minimum_length: int, maximum_length: int) -> bool:
    size = utf8_size(value)
    if size < minimum_length or size > maximum_length:
        return False
    return all(_is_word_char(ch) for ch in value)


def is_unicode_hashtag_word(value: str) -> bool:
    if not RICH_HASHTAG_MIN_LENGTH <= len(value) <= RICH_HASHTAG_MAX_LENGTH:
        return False
    all_decimal_digits = True
    for ch in value:
        category = unicodedata.category(ch)
        if category[0] not in "LN" and category != "Pc" and category not in ("Mn", "Mc", "Me"):
            return False
        all_decimal_digits = all_decimal_digits and category == "Nd"
    return not all_decimal_digits


def _split_at(value: str) -> tuple[str, str | None]:
    separator = value.find("@")
    return (value, None) if separator < 0 else (value[:separator], value[separator + 1 :])


def is_hashtag_action(value: str) -> bool:
    hashtag, rest = _split_at(value)
    if not is_unicode_hashtag_word(hashtag):
        return False
    return rest is None or (
        value.count("@") == 1 and is_ascii_word(rest, 1, RICH_USERNAME_MAX_LENGTH)
    )


def is_bot_command_action(value: str) -> bool:
    command, rest = _split_at(value)
    if not is_ascii_word(command, 1, RICH_COMMAND_MAX_LENGTH):
        return False
    return rest is None or (
        value.count("@") == 1
        and is_ascii_word(rest, RICH_USERNAME_MIN_LENGTH, RICH_USERNAME_MAX_LENGTH)
    )


def is_cashtag_action(value: str) -> bool:
    if not value or utf8_size(value) > RICH_CASHTAG_MAX_LENGTH:
        return False
    return all("A" <= ch <= "Z" for ch in value)


def is_valid_action_value(value: str, prefix: str) -> bool:
    if prefix == "#":
        return is_hashtag_action(value)
    if prefix == "/":
        return is_bot_command_action(value)
    if prefix == "$":
        return is_cashtag_action(value)
    return False


def safe_action_data(target: str, prefix: str) -> str | None:
    if not is_strict_target(target) or target[0] != prefix or utf8_size(target) < 2:
        return None
    result = target[1:]
    return result if is_valid_action_value(result, prefix) and is_strict_target(result) else None


def is_ascii_decimal(value: str) -> bool:
    return bool(value) and all("0" <= ch <= "9" for ch in value)


def safe_relative_emoji_href(path: str, relative_base: str) -> str | None:
    if (
        not is_strict_target(path)
        or path.startswith("/")
        or any(ch in path for ch in "\\:%?#")
        or path == TextPart.unavailable_emoji()
        or is_ascii_decimal(path)
    ):
        return None
    if any(component in ("", ".", "..") for component in path.split("/")):
        return None
    result = relative_base + path
    return result if is_strict_target(result) else None


def anchor_token(name: str) -> str:
    if not name:
        return "empty"
    data = name.encode("utf-8", "surrogateescape")

    def encoded(value: bytes) -> str:
        return base64.urlsafe_b64encode(value).decode("ascii").rstrip("=")

    if len(data) <= RICH_ANCHOR_DIRECT_BYTES:
        return encoded(data)
    digest = hashlib.sha256(data).hexdigest()[:16]
    return encoded(data[:RICH_ANCHOR_PREFIX_BYTES]) + "-" + digest
