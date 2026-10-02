"""The JPEG side of Data::WriteImageThumb as Telegram Desktop's Windows build runs it.

Desktop (v7.2.10, Qt 5.15.19) reads the photo with QImageReader and saves the scaled copy with
QImage::save, both through Qt's qjpeghandler.cpp (qtbase, LGPL-3.0 / GPL-2.0+ / GPL-3.0) linked
against mozjpeg 4.1.5 built WITH_JPEG8, with tdesktop's qtbase patch
0020-no-jpeg-chroma-subsampling. What that writes, and what is reproduced here:

- jpeg_set_defaults under mozjpeg's default JCP_MAX_COMPRESSION profile: progressive with
  optimized scans, optimized Huffman tables, trellis quantisation, ImageMagick base tables;
- Y sampled 1x1 (the patch), so 4:4:4; quality 75, QJpegHandler's default, since
  reader.quality() is -1;
- the JFIF density the image carries: the source's own, else QImage's 96 dpi;
- the source's ICC profile, copied back, as APP2 after the JFIF header.

imagecodecs ships that exact mozjpeg and calls it in the same order (defaults, colour space,
quality, sampling), but cannot write a density or a marker, so both are spliced into its output:
the APP0 and APP2 bytes are ones libjpeg would have written itself, and the entropy-coded data
does not depend on them. The decode side needs no port: Pillow's libjpeg-turbo produces the same
pixels as mozjpeg's decoder with Qt's settings (ISLOW, fancy upsampling).

Qt's supported non-sRGB matrix/TRC profiles convert through qt_icc before scaling, including
its generated sRGB output profile. Unsupported profiles retain their original bytes, as in Qt.
COM markers and raw CMYK conversion also follow Qt; synthetic branches were compared with an
exact-source Qt 5.15.19 oracle, not an all-input or all-platform parity guarantee.
"""

import struct
from importlib import import_module
from typing import Any

from .qt_icc import convert_to_srgb
from .qt_scale import smooth_scale

# QImageData::dpmx = qt_defaultDpiX() * 100 / 2.54, read back through qRound.
DEFAULT_DOTS_PER_METER = int(96 * 100 / 2.54 + 0.5)
_MAX_ICC_CHUNK = 65533 - (12 + 2)  # qjpeghandler.cpp maxMarkerSize minus the ICC header


def _qround(value: float) -> int:
    return int(value + 0.5)


def source_metadata(data: bytes) -> tuple[tuple[int, int], bytes]:
    """((dots per metre x, y), ICC profile) as QJpegHandler reads them from a JPEG header."""
    dpm = [DEFAULT_DOTS_PER_METER, DEFAULT_DOTS_PER_METER]
    icc = b""
    i = 2
    while i + 4 <= len(data) and data[i] == 0xFF:
        marker = data[i + 1]
        if marker == 0xFF:  # fill byte
            i += 1
            continue
        if marker in (0xD9, 0xDA):
            break
        length = struct.unpack(">H", data[i + 2 : i + 4])[0]
        payload = data[i + 4 : i + 2 + length]
        if marker == 0xE0 and payload[:5] == b"JFIF\x00" and len(payload) >= 12:
            unit = payload[7]
            density = struct.unpack(">HH", payload[8:12])
            for axis in (0, 1):
                if unit == 1:
                    value = int(100.0 * density[axis] / 2.54)
                elif unit == 2:
                    value = int(100.0 * density[axis])
                else:
                    continue
                if value:  # QImage::setDotsPerMeterX ignores 0
                    dpm[axis] = value
        elif marker == 0xE2 and len(payload) > 128 + 4 + 14 and payload[:12] == b"ICC_PROFILE\0":
            icc += payload[14:]
        i += 2 + length
    return (dpm[0], dpm[1]), icc


def density_fields(dpm_x: int, dpm_y: int) -> tuple[int, int, int]:
    """(density_unit, X_density, Y_density): the unit that loses less, as do_write_jpeg_image."""
    inch_x, inch_y = dpm_x * 2.54 / 100.0, dpm_y * 2.54 / 100.0
    diff_inch = abs(inch_x - _qround(inch_x)) + abs(inch_y - _qround(inch_y))
    diff_cm = (
        abs(dpm_x / 100.0 - _qround(dpm_x / 100.0)) + abs(dpm_y / 100.0 - _qround(dpm_y / 100.0))
    ) * 2.54
    if diff_inch < diff_cm:
        return 1, _qround(inch_x), _qround(inch_y)
    return 2, (dpm_x + 50) // 100, (dpm_y + 50) // 100


def source_comments(data: bytes) -> bytes:
    """QJpegHandler's readTexts -> QImage text map -> sorted UTF-8 COM markers."""
    text = {}
    i = 2
    while i + 4 <= len(data) and data[i] == 0xFF:
        marker = data[i + 1]
        if marker == 0xFF:
            i += 1
            continue
        if marker in (0xD9, 0xDA):
            break
        length = struct.unpack(">H", data[i + 2 : i + 4])[0]
        if length < 2 or i + 2 + length > len(data):
            break
        if marker == 0xFE:
            comment = data[i + 4 : i + 2 + length].decode("utf-8", errors="replace")
            index = comment.find(": ")
            if index == -1 or comment.find(" ") < index:
                key, value = "Description", comment
            else:
                key, value = comment[:index], comment[index + 2 :]
            if key:
                text[key] = value
        i += 2 + length
    markers = bytearray()
    for key in sorted(text, key=lambda value: value.encode("utf-16-be")):
        payload = (key + ": " + text[key]).encode("utf-8")[:65533]
        markers.extend(bytes((0xFF, 0xFE)) + struct.pack(">H", len(payload) + 2) + payload)
    return bytes(markers)


def _splice(
    encoded: bytes, dpm: tuple[int, int], icc: bytes, comments: bytes = b""
) -> bytes | None:
    """Put Qt's density, COM text and APP2 ICC markers after the JFIF header."""
    if encoded[2:4] != b"\xff\xe0" or encoded[6:11] != b"JFIF\x00":
        return None
    end = 4 + struct.unpack(">H", encoded[4:6])[0]
    app0 = bytearray(encoded[4:end])
    unit, x_density, y_density = density_fields(*dpm)
    if x_density > 0xFFFF or y_density > 0xFFFF:
        return None
    app0[9:14] = struct.pack(">BHH", unit, x_density, y_density)
    markers = comments
    chunks = [icc[i : i + _MAX_ICC_CHUNK] for i in range(0, len(icc), _MAX_ICC_CHUNK)]
    for number, chunk in enumerate(chunks, 1):
        block = b"ICC_PROFILE\x00" + bytes([number, len(chunks)]) + chunk
        markers += b"\xff\xe2" + struct.pack(">H", len(block) + 2) + block
    return encoded[:4] + bytes(app0) + markers + encoded[end:]


def desktop_thumb(
    image: Any, source: bytes, size: tuple[int, int], quality: int | None
) -> bytes | None:
    """The thumbnail file Desktop writes for a decoded JPEG `image` (Pillow) whose file bytes are
    `source`, or None when this cannot reproduce it: no imagecodecs with mozjpeg, or a mode
    other than RGB, L and CMYK.
    """
    if image.mode not in ("RGB", "L", "CMYK"):
        return None
    try:  # by name, like qt_scale: their stubs need a 3.12+ type checker
        imagecodecs = import_module("imagecodecs")
        numpy = import_module("numpy")
    except ImportError:
        return None
    if not imagecodecs.MOZJPEG.available:  # the manylinux wheels leave mozjpeg out
        return None
    if image.mode == "CMYK":
        # Use raw libjpeg samples, including its YCCK conversion, as Qt does.
        try:
            # Both locked wheels enable fancy upsampling by default; 2026.3.6 has no keyword.
            cmyk = imagecodecs.jpeg8_decode(
                source, outcolorspace=imagecodecs.JPEG8.CS.CMYK
            ).astype(numpy.uint16)
        except (imagecodecs.Jpeg8Error, ValueError):
            return None
        pixels = (cmyk[..., :3] * cmyk[..., 3:] // 255).astype(numpy.uint8)
    else:
        pixels = numpy.ascontiguousarray(numpy.asarray(image))
    dpm, icc = source_metadata(source)
    pixels, icc = convert_to_srgb(pixels, icc)
    if size != image.size:
        # QImage::smoothScaled promotes Grayscale8 only after color conversion.
        if pixels.ndim == 2:
            pixels = numpy.repeat(pixels[..., None], 3, axis=-1)
        pixels = smooth_scale(pixels, size[0], size[1])
    level = min(quality, 100) if quality is not None and quality >= 0 else 75
    try:
        encoded = bytes(imagecodecs.mozjpeg_encode(pixels, level, subsampling="444"))
    except (imagecodecs.MozjpegError, ValueError):
        return None  # the caller falls back to Pillow rather than losing the thumbnail
    return _splice(encoded, dpm, icc if pixels.ndim == 3 else b"", source_comments(source))
