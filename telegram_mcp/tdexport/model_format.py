"""Value formatting shared by the export writers.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.cpp
FormatDateTime/FormatFileSize/FormatDuration/FormatMoneyAmount/FormatPhoneNumber/NumberToString,
ui/text/format_values.cpp, countries/countries_instance.cpp CountriesInstance::format), GPL-3.0.
"""

from __future__ import annotations

import math
import time
from dataclasses import dataclass, field
from datetime import datetime, tzinfo
from typing import Any

CREDITS_CURRENCY = "XTR"

# None means the machine's local zone, as QDateTime::fromSecsSinceEpoch uses. Tests pin it.
LOCAL_TIMEZONE: tzinfo | None = None


def fill_left(data: str, length: int, filler: str) -> str:
    if length <= len(data):
        return data
    return filler * (length - len(data)) + data


def number_to_string(value: int | float, length: int = 0, filler: str = "0") -> str:
    """Data::NumberToString: std::to_string, left-padded, ',' replaced by '.'."""
    text = f"{value:f}" if isinstance(value, float) else str(int(value))
    return fill_left(text, length, filler).replace(",", ".")


def _local_datetime(date: int) -> datetime:
    if LOCAL_TIMEZONE is not None:
        return datetime.fromtimestamp(date, LOCAL_TIMEZONE)
    return datetime.fromtimestamp(date)


def _standard_offset_seconds() -> int:
    if LOCAL_TIMEZONE is not None:
        offset = LOCAL_TIMEZONE.utcoffset(None)
        return int(offset.total_seconds()) if offset else 0
    return -time.timezone


def _iso_offset_format(offset_from_utc: int) -> str:
    """QTimeZonePrivate::isoOffsetFormat (Qt 5.15)."""
    minutes = int(offset_from_utc / 60)
    sign = "+" if minutes >= 0 else "-"
    return f"UTC{sign}{abs(minutes) // 60:02d}:{abs(minutes) % 60:02d}"


def format_date_time(
    date: int,
    has_time_zone: bool = False,
    date_separator: str = ".",
    time_separator: str = ":",
    separator: str = " ",
) -> str:
    if not date:
        return ""
    value = _local_datetime(date)
    zone = separator + _iso_offset_format(_standard_offset_seconds()) if has_time_zone else ""
    return (
        f"{value.day:02d}{date_separator}{value.month:02d}{date_separator}{value.year}"
        f"{separator}{value.hour:02d}{time_separator}{value.minute:02d}"
        f"{time_separator}{value.second:02d}{zone}"
    )


def format_file_size(size: int) -> str:
    """Ui::FormatSizeText."""
    if size >= 1024 * 1024:
        tenth = size * 10 // (1024 * 1024)
        return f"{tenth // 10}.{tenth % 10} MB"
    if size >= 1024:
        tenth = size * 10 // 1024
        return f"{tenth // 10}.{tenth % 10} KB"
    return f"{size} B"


def format_duration(seconds: int) -> str:
    """Ui::FormatDurationText."""
    hours, minutes, secs = seconds // 3600, (seconds % 3600) // 60, seconds % 60
    return (
        (f"{hours}:" if hours else "")
        + ("" if minutes >= 10 else "0")
        + f"{minutes}:"
        + ("" if secs >= 10 else "0")
        + str(secs)
    )


def format_image_size_text(width: int, height: int) -> str:
    """Ui::FormatImageSizeText."""
    return f"{width}\u00d7{height}"


@dataclass(frozen=True)
class CurrencyRule:
    international: str = ""
    thousands: str = ","
    decimal: str = "."
    left: bool = True
    space: bool = False
    exponent: int = 2
    strip_dot_zero: bool = False


_R = CurrencyRule
# Ui::LookupCurrencyRule kRules ('\0' thousands means no separator).
_CURRENCY_RULES: dict[str, CurrencyRule] = {
    "AED": _R("", ",", ".", True, True),
    "AFN": _R(),
    "ALL": _R("", ".", ",", False),
    "AMD": _R("", ",", ".", False, True),
    "ARS": _R("", ".", ",", True, True),
    "AUD": _R("AU$"),
    "AZN": _R("", " ", ",", False, True),
    "BAM": _R("", ".", ",", False, True),
    "BDT": _R("", ",", ".", True, True),
    "BGN": _R("", " ", ",", False, True),
    "BHD": _R("", ",", ".", True, True, 3),
    "BND": _R("", ".", ","),
    "BOB": _R("", ".", ",", True, True),
    "BRL": _R("R$", ".", ",", True, True),
    "BYN": _R("", " ", ",", False, True),
    "CAD": _R("CA$"),
    "CHF": _R("", "'", ".", False, True),
    "CLP": _R("", ".", ",", True, True, 0),
    "CNY": _R("CN\u00a5"),
    "COP": _R("", ".", ",", True, True),
    "CRC": _R("", ".", ","),
    "CZK": _R("", " ", ",", False, True),
    "DKK": _R("", "\0", ",", False, True),
    "DOP": _R(),
    "DZD": _R("", ",", ".", True, True),
    "EGP": _R("", ",", ".", True, True),
    "ETB": _R(),
    "EUR": _R("\u20ac", " ", ",", False, True),
    "GBP": _R("\u00a3"),
    "GEL": _R("", " ", ",", False, True),
    "GHS": _R(),
    "GTQ": _R(),
    "HKD": _R("HK$"),
    "HNL": _R("", ",", ".", True, True),
    "HRK": _R("", ".", ",", False, True),
    "HUF": _R("", " ", ",", False, True),
    "IDR": _R("", ".", ","),
    "ILS": _R("\u20aa", ",", ".", True, True),
    "INR": _R("\u20b9"),
    "IQD": _R("", ",", ".", True, True, 3),
    "IRR": _R("", ",", "/", False, True),
    "ISK": _R("", ".", ",", False, True, 0),
    "JMD": _R(),
    "JOD": _R("", ",", ".", True, False, 3),
    "JPY": _R("\u00a5", ",", ".", True, False, 0),
    "KES": _R(),
    "KGS": _R("", " ", "-", False, True),
    "KRW": _R("\u20a9", ",", ".", True, False, 0),
    "KZT": _R("", " ", "-"),
    "LBP": _R("", ",", ".", True, True),
    "LKR": _R("", ",", ".", True, True),
    "MAD": _R("", ",", ".", True, True),
    "MDL": _R("", ",", ".", False, True),
    "MMK": _R(),
    "MNT": _R("", " ", ","),
    "MOP": _R(),
    "MUR": _R(),
    "MVR": _R("", ",", ".", False, True),
    "MXN": _R("MX$"),
    "MYR": _R(),
    "MZN": _R(),
    "NGN": _R(),
    "NIO": _R("", ",", ".", True, True),
    "NOK": _R("", " ", ",", True, True),
    "NPR": _R(),
    "NZD": _R("NZ$"),
    "PAB": _R("", ",", ".", True, True),
    "PEN": _R("", ",", ".", True, True),
    "PHP": _R(),
    "PKR": _R(),
    "PLN": _R("", " ", ",", False, True),
    "PYG": _R("", ".", ",", True, True, 0),
    "QAR": _R("", ",", ".", True, True),
    "RON": _R("", ".", ",", False, True),
    "RSD": _R("", ".", ",", False, True),
    "RUB": _R("", " ", ",", False, True),
    "SAR": _R("", ",", ".", True, True),
    "SEK": _R("", ".", ",", False, True),
    "SGD": _R(),
    "SYP": _R("", ",", ".", True, True),
    "THB": _R("\u0e3f"),
    "TJS": _R("", " ", ";", False, True),
    "TRY": _R("", ".", ",", False, True),
    "TTD": _R(),
    "TWD": _R("NT$"),
    "TZS": _R(),
    "UAH": _R("", " ", ",", False),
    "UGX": _R("", ",", ".", True, False, 0),
    "USD": _R("$"),
    "UYU": _R("", ".", ",", True, True),
    "UZS": _R("", " ", ",", False, True),
    "VEF": _R("", ".", ",", True, True),
    "VND": _R("\u20ab", ".", ",", False, True, 0),
    "YER": _R("", ",", ".", True, True),
    "ZAR": _R("", ",", ".", True, True),
}


def lookup_currency_rule(currency: str) -> CurrencyRule:
    return _CURRENCY_RULES.get(currency, CurrencyRule())


def format_with_separators(amount: float, precision: int, decimal: str, thousands: str) -> str:
    """Ui::FormatWithSeparators: std::fixed with grouping by three."""
    text = f"{amount:,.{precision}f}"
    whole, _, fraction = text.partition(".")
    whole = whole.replace(",", "" if thousands == "\0" else thousands)
    return whole + (decimal + fraction if fraction else "")


def fill_amount_and_currency(
    amount: int, currency: str, force_strip_dot_zero: bool = False
) -> str:
    if currency == CREDITS_CURRENCY:
        # Lang::FormatCountDecimal with the English (default) language.
        return "\u2b50" + f"{abs(amount):,}"
    rule = lookup_currency_rule(currency)
    prefix = "\u2212" if amount < 0 else ""
    value = abs(amount) / math.pow(10.0, rule.exponent)
    name = rule.international or currency
    result = prefix
    if rule.left:
        result += name
        if rule.space:
            result += " "
    strip = rule.strip_dot_zero or force_strip_dot_zero
    precision = rule.exponent if (not strip or math.floor(value) != value) else 0
    result += format_with_separators(value, precision, rule.decimal, rule.thousands)
    if not rule.left:
        if rule.space:
            result += " "
        result += name
    return result


def format_money_amount(amount: int, currency: str) -> str:
    return fill_amount_and_currency(amount, currency)


@dataclass
class CallingCodeInfo:
    calling_code: str
    prefixes: list[str] = field(default_factory=list)
    patterns: list[str] = field(default_factory=list)


@dataclass
class CountryInfo:
    name: str
    iso2: str
    codes: list[CallingCodeInfo] = field(default_factory=list)
    is_hidden: bool = False


# Countries::Instance().list(): the server list tdesktop caches. Its built-in fallback has no
# prefixes, so it never formats anything; an empty list reproduces that exactly.
_COUNTRIES: list[CountryInfo] = []


def set_countries_list(countries: list[CountryInfo]) -> None:
    _COUNTRIES[:] = countries


def countries_from_tl(result: Any) -> list[CountryInfo]:
    """countries_manager.cpp Manager::request: help.countriesList -> list, hidden skipped."""
    infos: list[CountryInfo] = []
    for country in getattr(result, "countries", None) or []:
        if country.hidden:
            continue
        info = CountryInfo(name=country.default_name, iso2=country.iso2)
        for code in country.country_codes:
            info.codes.append(
                CallingCodeInfo(
                    calling_code=code.country_code,
                    prefixes=list(code.prefixes) if code.prefixes is not None else [""],
                    patterns=list(code.patterns or []),
                )
            )
        infos.append(info)
    return infos


def _countries_format(phone: str) -> str:
    """CountriesInstance::format for FormatArgs{ .phone } (no groups, code kept)."""
    if not phone:
        return ""
    best: CallingCodeInfo | None = None
    best_length = 0
    for country in _COUNTRIES:
        for code in country.codes:
            if not phone.startswith(code.calling_code):
                continue
            code_size = len(code.calling_code)
            for prefix in code.prefixes:
                if code_size + len(prefix) > best_length and phone[code_size:].startswith(prefix):
                    best = code
                    best_length = code_size + len(prefix)
    if best is None:
        return phone
    formatted_part = phone[len(best.calling_code) :]
    formatted = formatted_part
    max_matched = 0
    for pattern in best.patterns:
        result = ""
        position = 0
        failed = False
        matched = 0
        for c in formatted_part:
            while (
                position < len(pattern)
                and pattern[position] != "X"
                and not pattern[position].isdigit()
            ):
                result += pattern[position]
                position += 1
            if position >= len(pattern) or pattern[position] == "X":
                position += 1
                result += c
            elif c == pattern[position]:
                matched += 1
                position += 1
                result += c
            else:
                failed = True
                break
        if not failed and matched >= max_matched:
            max_matched = matched
            formatted = result
    return "+" + best.calling_code + " " + formatted


def format_phone(phone: str) -> str:
    """Ui::FormatPhone."""
    if not phone:
        return ""
    if phone[0] == "0":
        return phone
    phone = phone.replace(" ", "")
    return _countries_format(phone[1:] if phone.startswith("+") else phone)


def format_phone_number(phone_number: str) -> str:
    return format_phone(phone_number) if phone_number else ""
