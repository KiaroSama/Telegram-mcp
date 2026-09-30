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

Known gap: a source ICC profile Qt does not consider sRGB makes Desktop convert the pixels
(tdesktop's 0021-convert-qimage-to-srgb) and write a Qt-generated profile; this copies both
unchanged. COM text and CMYK sources are not reproduced either (the caller falls back to Pillow
for CMYK).
"""

import struct
from importlib import import_module
from typing import Any

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


def _splice(encoded: bytes, dpm: tuple[int, int], icc: bytes) -> bytes | None:
    """Put Qt's density into the JFIF header and its APP2 ICC markers right after it."""
    if encoded[2:4] != b"\xff\xe0" or encoded[6:11] != b"JFIF\x00":
        return None
    end = 4 + struct.unpack(">H", encoded[4:6])[0]
    app0 = bytearray(encoded[4:end])
    unit, x_density, y_density = density_fields(*dpm)
    if x_density > 0xFFFF or y_density > 0xFFFF:
        return None
    app0[9:14] = struct.pack(">BHH", unit, x_density, y_density)
    markers = b""
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
    other than RGB and L.
    """
    if image.mode not in ("RGB", "L"):
        return None
    try:  # by name, like qt_scale: their stubs need a 3.12+ type checker
        imagecodecs = import_module("imagecodecs")
        numpy = import_module("numpy")
    except ImportError:
        return None
    if not imagecodecs.MOZJPEG.available:  # the manylinux wheels leave mozjpeg out
        return None
    if size != image.size:
        # QImage::smoothScaled turns Grayscale8 into RGB32 before scaling.
        rgb = image.convert("RGB") if image.mode == "L" else image
        pixels = smooth_scale(numpy.asarray(rgb), size[0], size[1])
    else:  # QImage::scaled returns the image itself, saved again as it is
        pixels = numpy.ascontiguousarray(numpy.asarray(image))
    level = min(quality, 100) if quality is not None and quality >= 0 else 75
    try:
        encoded = bytes(imagecodecs.mozjpeg_encode(pixels, level, subsampling="444"))
    except (imagecodecs.MozjpegError, ValueError):
        return None  # the caller falls back to Pillow rather than losing the thumbnail
    dpm, icc = source_metadata(source)
    return _splice(encoded, dpm, icc)
