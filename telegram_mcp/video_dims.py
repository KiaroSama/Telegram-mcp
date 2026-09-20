"""A video's real size and length, read from its container.

Telethon fills `DocumentAttributeVideo` from `hachoir`, and with no metadata
reader installed it falls back to `w=1, h=1, duration=0`. That is what this
server has been sending for every video. Telegram re-derives the true size on its
side, so a viewer sees nothing wrong — but the attribute is a lie, and a client
that lays out from it gets a 1x1 box.

`hachoir` was measured as the alternative and rejected: it reads `.mp4` correctly
and returns `w=1, h=1` for `.webm`, so it would fix one container and leave the
other, for the price of a dependency and stderr noise. Reading the container is
what `ogg_tags` already does for the audio duration, and it costs nothing.

This parses a CONTAINER. It never decodes a frame, and every unknown answers
zeros rather than a guess, because a wrong size is worse than none: the caller
sends no attribute at all and Telegram derives it, which is today's behaviour.
"""

__all__ = ["HEAD_BYTES", "TAIL_BYTES", "dimensions"]

#: Enough of the front for a faststart MP4 and for Matroska, whose Tracks
#: element is near the beginning by design.
HEAD_BYTES = 256 * 1024

#: An MP4 written without faststart puts `moov` at the END, so the tail is not
#: an optimisation - it is the only place the size lives for most files a phone
#: records.
TAIL_BYTES = 256 * 1024


def dimensions(head: bytes, tail: bytes = b"") -> tuple[int, int, int]:
    """``(width, height, duration_seconds)``, or ``(0, 0, 0)`` when unreadable.

    Takes both ends because an MP4's `moov` may be at either. They are searched
    separately rather than concatenated: joining them would invent byte offsets
    across the gap and could match a box header that does not exist.
    """
    for chunk in (head, tail):
        if not chunk:
            continue
        width, height, duration = _from_mp4(chunk)
        if width and height:
            return width, height, duration
        width, height = _from_matroska(chunk)
        if width and height:
            return width, height, _from_mp4(chunk)[2]
    return 0, 0, 0


def _from_mp4(data: bytes) -> tuple[int, int, int]:
    """`tkhd` carries the display size; `mvhd` carries the length."""
    width = height = duration = 0

    start = data.find(b"tkhd")
    if start >= 4:
        size = int.from_bytes(data[start - 4 : start], "big")
        end = start - 4 + size
        # A `tkhd` is 92 bytes at its smallest, and its width and height are the
        # LAST eight - two 16.16 fixed-point numbers, whatever version it is.
        if 92 <= size <= 4096 and end <= len(data):
            width = int.from_bytes(data[end - 8 : end - 6], "big")
            height = int.from_bytes(data[end - 4 : end - 2], "big")

    start = data.find(b"mvhd")
    if start >= 0 and start + 24 <= len(data):
        version = data[start + 4]
        if version == 0 and start + 24 <= len(data):
            timescale = int.from_bytes(data[start + 16 : start + 20], "big")
            ticks = int.from_bytes(data[start + 20 : start + 24], "big")
        elif version == 1 and start + 36 <= len(data):
            timescale = int.from_bytes(data[start + 24 : start + 28], "big")
            ticks = int.from_bytes(data[start + 28 : start + 36], "big")
        else:
            timescale = ticks = 0
        if timescale > 0:
            duration = int(ticks // timescale)

    return width, height, duration


def _from_matroska(data: bytes) -> tuple[int, int]:
    """Matroska's `PixelWidth` (0xB0) and `PixelHeight` (0xBA).

    ponytail: a bounded scan for the two element ids rather than a full EBML
    walk. Both are one-byte ids followed by a one-byte length of 1 or 2, so a
    false match needs that exact shape AND a plausible size; the pair is then
    sanity-checked together. A real EBML parser is the upgrade path if a file
    ever fools this.
    """
    if not data.startswith(b"\x1a\x45\xdf\xa3"):
        return 0, 0
    width = _ebml_uint(data, 0xB0)
    height = _ebml_uint(data, 0xBA)
    if 0 < width <= 16384 and 0 < height <= 16384:
        return width, height
    return 0, 0


def _ebml_uint(data: bytes, element_id: int) -> int:
    marker = bytes([element_id])
    start = 0
    while True:
        found = data.find(marker, start)
        if found == -1 or found + 2 > len(data):
            return 0
        length = data[found + 1]
        # 0x81/0x82: an EBML length of one or two bytes. Anything else here is
        # not the element being looked for.
        if length in (0x81, 0x82) and found + 2 + (length & 0x7F) <= len(data):
            size = length & 0x7F
            return int.from_bytes(data[found + 2 : found + 2 + size], "big")
        start = found + 1
