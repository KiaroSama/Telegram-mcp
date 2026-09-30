"""The slice of QUrl the HTML export relies on: a StrictMode parse and a FullyEncoded re-encode.

Ported from Qt 5.15.19 (src/corelib/io/qurl.cpp QUrlPrivate::parse/setAuthority/setHost/
validateComponent/validityError/toString, qurlrecode.cpp qt_urlRecode, qurlidna.cpp qt_ACE_do,
qipaddress.cpp), LGPL-3.0, which is what tdesktop v7.2.10 links on Windows x64. Only the
behaviour SafeMessageHref/SafeHttpHref observe is reproduced: validity, the lowercased scheme,
whether a host is present, and the re-encoded bytes.
"""

from __future__ import annotations

import re
import stringprep
from dataclasses import dataclass
from encodings import idna as _idna

_DECODE, _LEAVE, _ENCODE = 0, 1, 2


def _default_actions() -> list[int]:
    """qurlrecode.cpp defaultActionTable, indexed by `character - ' '`."""
    table = []
    for code in range(0x20, 0x80):
        ch = chr(code)
        if ch.isalnum() or ch in "-._~":
            table.append(_DECODE)
        elif ch in ' "%<>\\^`{|}\x7f':
            table.append(_ENCODE)
        else:
            table.append(_LEAVE)
    return table


_DEFAULT_ACTIONS = _default_actions()
_DELIMITERS = ':@][/?#"<>^\\|{}'
# The *InIsolation tables (all "decode") and the *InUrl tables (all "encode").
_USER_NAME_IN_ISOLATION = _DELIMITERS
_PASSWORD_IN_ISOLATION = _DELIMITERS[1:]
_PATH_IN_ISOLATION = _DELIMITERS[5:]
_QUERY_IN_ISOLATION = _DELIMITERS[6:]
_FRAGMENT_IN_ISOLATION = _DELIMITERS[7:]
_USER_NAME_IN_URL = ":@][/?#"
_PASSWORD_IN_URL = _USER_NAME_IN_URL[1:]
_PATH_IN_URL = _USER_NAME_IN_URL[5:]
_QUERY_IN_URL = _FRAGMENT_IN_URL = _USER_NAME_IN_URL[6:]


def _is_hex(ch: str) -> bool:
    return ch in "0123456789abcdefABCDEF"


def _decode_percent(text: str, index: int) -> int:
    if index + 2 >= len(text) or not _is_hex(text[index + 1]) or not _is_hex(text[index + 2]):
        return -1
    return int(text[index + 1 : index + 3], 16)


def _upper_hex(ch: str) -> str:
    return ch if ord(ch) < 0x60 else chr(ord(ch) - 0x20)


def _is_noncharacter(code: int) -> bool:
    return 0xFDD0 <= code <= 0xFDEF or (code & 0xFFFE) == 0xFFFE


def _decode_utf8_run(text: str, index: int, lead: int) -> tuple[str, int] | None:
    """encodedUtf8ToUtf16: a percent-encoded UTF-8 sequence starting at `index`."""
    if 0xC2 <= lead <= 0xDF:
        need = 1
    elif 0xE0 <= lead <= 0xEF:
        need = 2
    elif 0xF0 <= lead <= 0xF4:
        need = 3
    else:
        return None
    data = [lead]
    position = index + 3
    for _ in range(need):
        if position + 3 > len(text) or text[position] != "%":
            return None
        value = _decode_percent(text, position)
        if value < 0:
            return None
        data.append(value)
        position += 3
    try:
        char = bytes(data).decode("utf-8")
    except UnicodeDecodeError:
        return None
    if _is_noncharacter(ord(char)):
        return None
    return char, position - index


def _recode_run(text: str, encode: bool, table: list[int], retry: bool) -> str:
    """qurlrecode.cpp recode()."""
    out: list[str] = []
    action = _ENCODE
    index, size = 0, len(text)
    while index < size:
        ch = text[index]
        code = ord(ch)
        if code < 0x20:
            action = _ENCODE
        elif code < 0x80:
            action = table[code - 0x20]
            if action != _ENCODE:
                out.append(ch)
                index += 1
                continue
        decoded = code
        if ch == "%" and retry:
            out.append("%25")
            index += 1
            continue
        if ch == "%":
            decoded = _decode_percent(text, index)
            if decoded < 0:
                return _recode_run(text, encode, table, True)
            if decoded >= 0x80:
                if not encode:
                    run = _decode_utf8_run(text, index, decoded)
                    if run is not None:
                        out.append(run[0])
                        index += run[1]
                        continue
                action = _LEAVE
            elif decoded >= 0x20:
                action = table[decoded - 0x20]
        elif code >= 0x80:
            if encode:
                out.append("".join(f"%{b:02X}" for b in ch.encode("utf-8", "surrogatepass")))
            else:
                out.append(ch)
            index += 1
            continue
        if ch == "%" and action != _DECODE:
            out.append("%" + _upper_hex(text[index + 1]) + _upper_hex(text[index + 2]))
            index += 3
        elif ch == "%":
            out.append(chr(decoded))
            index += 3
        else:
            out.append(f"%{code >> 4:X}{code & 0xF:X}")
            index += 1
    return "".join(out)


def _recode(text: str, encode: bool, modifications: str) -> str:
    """qt_urlRecode: `encode` False is the parse-time form, True is QUrl::FullyEncoded."""
    table = list(_DEFAULT_ACTIONS)
    if not encode:
        table[0] = _DECODE
    for ch in modifications:
        table[ord(ch) - 0x20] = _ENCODE if encode else _DECODE
    return _recode_run(text, encode, table, False)


def _validate_component(text: str, user_info: bool = False) -> bool:
    """QUrlPrivate::validateComponent."""
    for index, ch in enumerate(text):
        code = ord(ch)
        if code >= 0x80:
            continue
        if ch == "%":
            if _decode_percent(text, index) < 0:
                return False
        elif code <= 0x20 or ch in '"<>\\^`{|}\x7f':
            return False
        elif user_info and ch in "/?#[]@":
            return False
    return True


def _parse_ip4(text: str) -> str | None:
    """QIPAddressUtils::parseIp4 (inet_aton forms, base 0 numbers) + toString."""
    if any(ord(ch) >= 0x7F for ch in text):
        return None
    parts = text.split(".")
    if len(parts) > 4:
        return None
    address = 0
    for index, part in enumerate(parts):
        match = re.fullmatch(r"\s*\+?(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)", part)
        if match is None:
            return None
        literal = match.group(1)
        if literal[:2] in ("0x", "0X"):
            value = int(literal[2:], 16)
        elif literal.startswith("0"):
            value = int(literal, 8)
        else:
            value = int(literal)
        last = index == len(parts) - 1
        bits = 32 - 8 * index if last else 8
        if value >> bits or value > 0xFFFFFFFF:
            return None
        address = (address << (bits if last else 8)) | value
    return ".".join(str((address >> shift) & 0xFF) for shift in (24, 16, 8, 0))


def _format_ip6(packed: bytes) -> str:
    """QIPAddressUtils::toString(IPv6Address)."""
    if packed[:10] == bytes(10):
        if packed[10:12] == b"\xff\xff":
            return "::ffff:" + ".".join(str(b) for b in packed[12:])
        if packed[10:12] == b"\0\0":
            if packed[12:15] != b"\0\0\0":
                return "::" + ".".join(str(b) for b in packed[12:])
            if packed[15] == 0:
                return "::"
    words = [packed[i] << 8 | packed[i + 1] for i in range(0, 16, 2)]
    run_start, run_length, index = -1, 0, 0
    while index < 8:
        if words[index] == 0:
            end = index
            while end < 8 and words[end] == 0:
                end += 1
            if end - index > run_length:
                run_start, run_length = index, end - index
            index = end
        else:
            index += 1
    if run_length < 2:
        return ":".join(f"{w:x}" for w in words)
    head = ":".join(f"{w:x}" for w in words[:run_start])
    tail = ":".join(f"{w:x}" for w in words[run_start + run_length :])
    return head + "::" + tail


def _parse_ip6(text: str) -> str | None:
    import ipaddress

    zone = ""
    if text.count("%25") == 1:
        text, zone = text.split("%25")
        if not zone:
            return None
    if not text or "%" in text or any(ord(ch) >= 0x7F for ch in text):
        return None
    try:
        packed = ipaddress.IPv6Address(text).packed
    except ValueError:
        return None
    return "[" + _format_ip6(packed) + ("%25" + zone if zone else "") + "]"


def _std3(label: str, case_insensitive: bool) -> bool:
    """qt_find_nonstd3 == nullptr."""
    if len(label) > 63:
        return False
    for index, ch in enumerate(label):
        if ch == "-" and index in (0, len(label) - 1):
            return False
        if case_insensitive and "A" <= ch <= "Z":
            continue
        if not (ch == "-" or ch == "_" or "0" <= ch <= "9" or "a" <= ch <= "z"):
            return False
    return True


def _ace_label(label: str) -> str | None:
    lowered = "".join(chr(ord(ch) | 0x20) if "A" <= ch <= "Z" else ch for ch in label)
    simple = all(ord(ch) <= 0x7F for ch in label)
    if simple and len(label) > 6 and lowered.startswith("xn--"):
        simple = False
    if simple:
        return lowered if _std3(lowered, True) else None
    try:
        prepped = _idna.nameprep(lowered)
    except UnicodeError:
        return None
    if any(stringprep.in_table_a1(ch) for ch in prepped):
        return None
    if all(ord(ch) < 0x80 for ch in prepped):
        ace = prepped
    else:
        ace = "xn--" + prepped.encode("punycode").decode("ascii")
    return ace if _std3(ace, True) else None


def _ace(domain: str) -> str | None:
    """qt_ACE_do(domain, ToAceOnly/NormalizeAce, ForbidLeadingDot), in its ACE form."""
    if not domain:
        return domain
    labels = re.split("[.\u3002\uff0e\uff61]", domain)
    result = []
    for index, label in enumerate(labels):
        if not label:
            if index == len(labels) - 1:
                result.append("")
                break
            return None
        ace = _ace_label(label)
        if ace is None:
            return None
        result.append(ace)
    return ".".join(result)


def _set_host(value: str) -> str | None:
    """QUrlPrivate::setHost in StrictMode; returns the host as FullyEncoded writes it."""
    if not value:
        return ""
    if value[0] == "[":
        if value[-1] != "]":
            return None
        if len(value) > 5 and value[1] == "v":
            match = re.fullmatch(r"\[v([0-9A-Fa-f])\.([A-Za-z0-9!$&'()*+,;=:\-._~]+)\]", value)
            return f"[v{match.group(1).upper()}.{match.group(2)}]" if match else None
        if value[1] == "v":
            return None
        return _parse_ip6(value[1:-1])
    ip4 = _parse_ip4(value)
    if ip4 is not None:
        return ip4
    ace = _ace(value)
    if not ace:
        return None
    return _parse_ip4(ace) or ace


@dataclass
class ParsedUrl:
    valid: bool = True
    scheme: str | None = None
    user_name: str | None = None
    password: str | None = None
    # None when there is no authority; the host as FullyEncoded writes it otherwise.
    host: str | None = None
    port: int = -1
    path: str = ""
    query: str | None = None
    fragment: str | None = None


def _scheme(value: str) -> str | None:
    if not value or not ("a" <= value[0].lower() <= "z"):
        return None
    if re.fullmatch(r"[A-Za-z][A-Za-z0-9+\-.]*", value) is None:
        return None
    return value.lower()


def _set_authority(url: ParsedUrl, auth: str) -> bool:
    url.host = ""
    if not auth:
        return True
    start = 0
    at = auth.find("@")
    if at >= 0:
        user_info = auth[:at]
        colon = user_info.find(":")
        url.user_name = _recode(
            user_info if colon < 0 else user_info[:colon], False, _USER_NAME_IN_ISOLATION
        )
        if colon >= 0:
            url.password = _recode(user_info[colon + 1 :], False, _PASSWORD_IN_ISOLATION)
        if not _validate_component(user_info, user_info=True):
            return False
        start = at + 1
    colon = auth.rfind(":", start)
    if colon >= 0 and start < len(auth) and auth[start] == "[":
        closing = auth.find("]", start)
        if closing < 0 or closing > colon:
            colon = -1
    if 0 <= colon < len(auth) - 1:
        port = 0
        for ch in auth[colon + 1 :]:
            if not "0" <= ch <= "9":
                return False
            port = (port * 10 + ord(ch) - 0x30) & 0xFFFFFFFF
        if port > 0xFFFF:
            return False
        url.port = port
    raw_host = auth[start : colon if colon >= 0 else len(auth)]
    host = _set_host(raw_host)
    if host is None or not _validate_component(raw_host):
        return False
    url.host = host
    return True


def parse_strict(text: str) -> ParsedUrl:
    """QUrl(text, QUrl::StrictMode); `valid` is QUrl::isValid()."""
    url = ParsedUrl()
    colon = question = hash_ = -1
    for index, ch in enumerate(text):
        if ch == "#":
            hash_ = index
            break
        if question == -1:
            if ch == ":" and colon == -1:
                colon = index
            elif ch == "?":
                question = index
    size = len(text)
    hier_start = 0
    if colon != -1:
        url.scheme = _scheme(text[:colon])
        if url.scheme is not None:
            hier_start = colon + 1
    hier_end = min(i for i in (question, hash_, size) if i >= 0)
    if text[hier_start:hier_end].startswith("//"):
        authority_end = text.find("/", hier_start + 2, hier_end)
        if authority_end < 0:
            authority_end = hier_end
        if not _set_authority(url, text[hier_start + 2 : authority_end]):
            url.valid = False
            return url
        path_start = authority_end
    else:
        path_start = hier_start
    raw_path = text[path_start:hier_end]
    url.path = _recode(raw_path, False, _PATH_IN_ISOLATION)
    raw_query = text[question + 1 : hash_ if hash_ >= 0 else size] if question >= 0 else None
    raw_fragment = text[hash_ + 1 :] if hash_ >= 0 else None
    if raw_query is not None:
        url.query = _recode(raw_query, False, _QUERY_IN_ISOLATION)
    if raw_fragment is not None:
        url.fragment = _recode(raw_fragment, False, _FRAGMENT_IN_ISOLATION)
    for raw in (raw_path, raw_query, raw_fragment):
        if raw is not None and not _validate_component(raw):
            url.valid = False
            return url
    url.valid = _validity(url)
    return url


def _validity(url: ParsedUrl) -> bool:
    """QUrl::isValid after a successful parse: not isEmpty() and no validityError."""
    path = url.path
    if not path and all(x is None for x in (url.scheme, url.host, url.query, url.fragment)):
        return False
    if not path:
        return True
    if path[0] == "/":
        return url.host is not None or len(path) == 1 or path[1] != "/"
    if url.host is not None:
        return False
    if url.scheme is not None:
        return True
    slash, colon = path.find("/"), path.find(":")
    return colon < 0 or (0 <= slash < colon)


def to_encoded(url: ParsedUrl) -> str:
    """QUrl::toEncoded(QUrl::FullyEncoded) of a valid URL."""
    out = ""
    if url.scheme is not None:
        out += url.scheme + ":"
    if url.host is not None:
        out += "//"
        if url.user_name is not None:
            out += _recode(url.user_name, True, _USER_NAME_IN_URL)
            if url.password is not None:
                out += ":" + _recode(url.password, True, _PASSWORD_IN_URL)
            out += "@"
        out += _recode(url.host, True, "") if url.host.startswith("[") else url.host
        if url.port != -1:
            out += ":" + str(url.port)
    out += _recode(url.path, True, _PATH_IN_URL)
    if url.query is not None:
        out += "?" + _recode(url.query, True, _QUERY_IN_URL)
    if url.fragment is not None:
        out += "#" + _recode(url.fragment, True, _FRAGMENT_IN_URL)
    return out
