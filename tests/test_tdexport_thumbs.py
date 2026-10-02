"""tdesktop export photo thumbnails, byte for byte (spec 030).

Desktop writes `*_thumb.jpg` as QImageReader -> QImage::scaled(SmoothTransformation) -> QImage::save
on Qt 5.15.19 with tdesktop's qtbase patches, whose JPEG plugin links mozjpeg 4.1.5. The expected
values below are derived by hand from qimagescale.cpp, qjpeghandler.cpp and mozjpeg's jcparam.c.
Synthetic images only, generated in tmp_path.
"""

import hashlib
import io
import struct
import sys

import pytest

from telegram_mcp.tdexport import qt_jpeg
from telegram_mcp.tdexport.files import write_image_thumb
from telegram_mcp.tdexport.qt_scale import down_taps, up_taps

# ImageMagick table (mozjpeg quant_tbl_master_idx 3, both luminance and chrominance), natural order.
_IMAGEMAGICK = [
    16, 16, 16, 18, 25, 37, 56, 85,
    16, 17, 20, 27, 34, 40, 53, 75,
    16, 20, 24, 31, 43, 62, 91, 135,
    18, 27, 31, 40, 53, 74, 106, 156,
    25, 34, 43, 53, 69, 94, 131, 189,
    37, 40, 62, 74, 94, 124, 169, 238,
    56, 53, 91, 106, 131, 169, 226, 311,
    85, 75, 135, 156, 189, 238, 311, 418,
]  # fmt: skip
_ZIGZAG = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5,
    12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
]  # fmt: skip


def _segments(data: bytes) -> list[tuple[int, bytes]]:
    """(marker, payload) up to the first SOS."""
    assert data[:2] == b"\xff\xd8"
    out, i = [], 2
    while True:
        marker = data[i + 1]
        length = struct.unpack(">H", data[i + 2 : i + 4])[0]
        out.append((marker, data[i + 4 : i + 2 + length]))
        if marker == 0xDA:
            return out
        i += 2 + length


def _jpeg(size, mode="RGB", **save):
    from PIL import Image as PilImage

    image = PilImage.new(mode, size)
    image.putdata(
        [
            (
                ((x * 7 + y * 3) % 256, (x * x + y) % 256, (255 - y * 5) % 256)
                if mode == "RGB"
                else (x * 11 + y * 5) % 256
            )
            for y in range(size[1])
            for x in range(size[0])
        ]
    )
    buffer = io.BytesIO()
    image.save(buffer, "JPEG", quality=90, **save)
    return buffer.getvalue()


def _mozjpeg():
    imagecodecs = pytest.importorskip("imagecodecs")
    if not imagecodecs.MOZJPEG.available:
        pytest.skip("this imagecodecs wheel has no mozjpeg (the manylinux ones)")
    return imagecodecs


def _thumb(tmp_path, source: bytes, size, name="p.jpg"):
    (tmp_path / "photos").mkdir(exist_ok=True)
    (tmp_path / "photos" / name).write_bytes(source)
    base = str(tmp_path).replace("\\", "/") + "/"
    path, final = write_image_thumb(base, "photos/" + name, lambda _size: size)
    assert final == size
    return (tmp_path / path).read_bytes()


# --- qimagescale.cpp: qimageCalcXPoints / qimageCalcApoints, by hand ---------------------------


def test_down_taps_follow_qimage_calc_apoints():
    # 4 -> 2: inc = 4<<16 / 2, Cp = ((2<<14) + 3) / 4 = 8192; each output covers two sources.
    assert down_taps(4, 2) == [(0, [8192, 8192]), (2, [8192, 8192])]
    # 3 -> 2: Cp = 32770 / 3 = 10923; the second starts half way through source 1.
    assert down_taps(3, 2) == [(0, [10923, 5461]), (1, [5461, 10923])]
    # Weights always sum to 1 << 14 and never read past the edge.
    for source in range(2, 90):
        for target in range(1, source):
            for start, weights in down_taps(source, target):
                assert sum(weights) == 1 << 14
                assert start + len(weights) <= source


def test_up_taps_follow_qimage_calc_apoints():
    # 2 -> 4: val starts at 0x8000 * 2 / 4 - 0x8000 = -16384, inc = 32768.
    assert up_taps(2, 4) == [(0, [256, 0]), (0, [192, 64]), (0, [64, 192]), (1, [256, 0])]


def test_smooth_scale_floors_box_average():
    np = pytest.importorskip("numpy")
    from telegram_mcp.tdexport.qt_scale import smooth_scale

    pixels = np.array([[[0, 0, 0], [1, 1, 1]], [[2, 2, 2], [3, 3, 3]]], dtype=np.uint8)
    # H = (p0 + p1) * 8192 >> 4, V = (H0 + H1) * 8192 >> 24: 6 / 4 floors to 1, never rounds.
    assert smooth_scale(pixels, 1, 1).tolist() == [[[1, 1, 1]]]


def test_smooth_scale_down_x_only():
    np = pytest.importorskip("numpy")
    from telegram_mcp.tdexport.qt_scale import smooth_scale

    row = np.array([[[0, 0, 0], [100, 100, 100], [200, 200, 200]]], dtype=np.uint8)
    # (100 * 5461) >> 14 = 33 and (100 * 5461 + 200 * 10923) >> 14 = 166.
    assert smooth_scale(row, 2, 1)[0, :, 0].tolist() == [33, 166]


def test_smooth_scale_up_interpolates_and_truncates():
    np = pytest.importorskip("numpy")
    from telegram_mcp.tdexport.qt_scale import smooth_scale

    row = np.array([[[0, 0, 0], [200, 200, 200]]], dtype=np.uint8)
    # (200 * 64) >> 8 = 50 and (200 * 192) >> 8 = 150.
    assert smooth_scale(row, 4, 1)[0, :, 0].tolist() == [0, 50, 150, 200]
    square = np.array([[[0] * 3, [255] * 3], [[100] * 3, [0] * 3]], dtype=np.uint8)
    # Pixel (1, 1) of 2x2 -> 4x4: vertical first (SSE2 interpolate_4_pixels), then horizontal:
    # left (100 * 64) >> 8 = 25, right (255 * 192) >> 8 = 191, (25 * 192 + 191 * 64) >> 8 = 66.
    assert int(smooth_scale(square, 4, 4)[1, 1, 0]) == 66


# --- qjpeghandler.cpp write path ---------------------------------------------------------------


def test_density_fields_match_qjpeghandler():
    assert qt_jpeg.density_fields(3780, 3780) == (1, 96, 96)  # QImage default: 96 dpi
    assert qt_jpeg.density_fields(2834, 2834) == (1, 72, 72)  # int(100 * 72 / 2.54)
    assert qt_jpeg.density_fields(2800, 2800) == (2, 28, 28)  # 28 dots/cm


def test_source_metadata_reads_density_and_icc():
    profile = b"\x00" * 200
    icc = b"ICC_PROFILE\x00\x01\x01" + profile
    source = (
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x01\x00\x48\x00\x48\x00\x00"
        + b"\xff\xe2"
        + struct.pack(">H", len(icc) + 2)
        + icc
        + b"\xff\xda\x00\x02"
    )
    assert qt_jpeg.source_metadata(source) == ((2834, 2834), profile)
    # Aspect-ratio-only JFIF and no JFIF at all keep QImage's 96 dpi default.
    assert qt_jpeg.source_metadata(source[:13] + b"\x00" + source[14:])[0] == (3780, 3780)
    assert qt_jpeg.source_metadata(b"\xff\xd8\xff\xda\x00\x02") == ((3780, 3780), b"")


def test_thumb_is_mozjpeg_progressive_444_with_desktop_tables(tmp_path):
    _mozjpeg()
    data = _thumb(tmp_path, _jpeg((64, 48)), (40, 30))
    segments = _segments(data)
    markers = [marker for marker, _ in segments]
    assert markers[0] == 0xE0 and 0xE2 not in markers and 0xFE not in markers
    assert segments[0][1][:14] == b"JFIF\x00\x01\x01\x01\x00\x60\x00\x60\x00\x00"
    (sof,) = [payload for marker, payload in segments if marker in (0xC0, 0xC1, 0xC2)]
    assert 0xC2 in markers and 0xC0 not in markers  # progressive, from mozjpeg's defaults
    assert struct.unpack(">HH", sof[1:5]) == (30, 40)
    assert [sof[7 + 3 * k] for k in range(sof[5])] == [0x11, 0x11, 0x11]  # 4:4:4
    tables = [payload for marker, payload in segments if marker == 0xDB]
    assert len(tables) == 1 and len(tables[0]) == 130  # both tables in one DQT
    # jpeg_set_quality(75): scale 200 - 2 * 75 = 50%, (v * 50 + 50) / 100, baseline-limited.
    expected = [min(255, max(1, (_IMAGEMAGICK[z] * 50 + 50) // 100)) for z in _ZIGZAG]
    assert tables[0][0] == 0x00 and list(tables[0][1:65]) == expected
    assert tables[0][65] == 0x01 and list(tables[0][66:130]) == expected


def test_thumb_carries_source_density_and_icc(tmp_path):
    _mozjpeg()
    from PIL import ImageCms

    profile = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    source = _jpeg((50, 40), dpi=(72, 72), icc_profile=profile)
    segments = _segments(_thumb(tmp_path, source, (25, 20)))
    assert segments[0][1][7:12] == b"\x01\x00\x48\x00\x48"
    assert segments[1][0] == 0xE2
    assert segments[1][1] == b"ICC_PROFILE\x00\x01\x01" + profile


def test_unscaled_grayscale_stays_one_component(tmp_path):
    _mozjpeg()
    same = _segments(_thumb(tmp_path, _jpeg((16, 16), "L"), (16, 16)))
    (sof,) = [payload for marker, payload in same if marker == 0xC2]
    assert sof[5] == 1
    # Scaled, QImage::smoothScaled converts Grayscale8 to RGB32 first.
    scaled = _segments(_thumb(tmp_path, _jpeg((16, 16), "L"), (8, 8), "g.jpg"))
    (sof,) = [payload for marker, payload in scaled if marker == 0xC2]
    assert sof[5] == 3


def test_thumb_bytes_are_pinned(tmp_path):
    imagecodecs = _mozjpeg()
    # Desktop's own mozjpeg: another version changes the entropy coding and trellis choices.
    assert imagecodecs.mozjpeg_version().startswith("mozjpeg 4.1.5")
    data = _thumb(tmp_path, _jpeg((64, 48)), (40, 30))
    assert hashlib.sha256(data).hexdigest() == _PINNED_SHA256


# Computed by this pipeline, the one that reproduced 87 of 87 real Desktop thumbnails; the same
# on Windows with imagecodecs 2026.3.6 (Python 3.11) and 2026.8.16 (3.13).
_PINNED_SHA256 = "5b3ee84dc8e3fee426431a1b46e3d7671df2c88ae71d14e35b7050c3727eeae0"


def test_without_mozjpeg_pillow_still_writes_a_thumb(tmp_path, monkeypatch):
    pytest.importorskip("PIL")
    monkeypatch.setitem(sys.modules, "imagecodecs", None)
    data = _thumb(tmp_path, _jpeg((64, 48)), (40, 30))
    assert 0xC0 in [marker for marker, _ in _segments(data)]  # Pillow's baseline encoder


@pytest.mark.parametrize("size", [(16, 16), (8, 8)])
def test_cmyk_thumbnail_uses_desktop_rgb_conversion(tmp_path, size, monkeypatch):
    imagecodecs = _mozjpeg()
    from PIL import Image

    decode = imagecodecs.jpeg8_decode

    # Python 3.11's locked wheel enables fancy upsampling without exposing a keyword.
    def older_decoder(data, /, *, outcolorspace):
        return decode(data, outcolorspace=outcolorspace)

    monkeypatch.setattr(imagecodecs, "jpeg8_decode", older_decoder)
    image = Image.new("CMYK", (16, 16), (70, 130, 190, 90))
    buffer = io.BytesIO()
    image.save(buffer, "JPEG", quality=90)
    source = buffer.getvalue()
    data = _thumb(tmp_path, source, size)
    assert 0xC2 in [marker for marker, _ in _segments(data)]
    with Image.open(io.BytesIO(source)) as decoded:
        c, m, y, k = decoded.getpixel((0, 0))
    expected = tuple((255 - k) * (255 - value) // 255 for value in (c, m, y))
    with Image.open(io.BytesIO(data)) as thumb:
        assert thumb.mode == "RGB"
        assert all(abs(a - b) <= 2 for a, b in zip(thumb.getpixel((0, 0)), expected))


def test_jpeg_comments_use_qt_key_order_and_utf8(tmp_path):
    _mozjpeg()
    comments = ["Zeta: final", "a description", "Alpha: first", "Zeta: replaced"]
    source = _jpeg((16, 16))
    markers = b"".join(
        bytes((0xFF, 0xFE))
        + struct.pack(">H", len(text.encode("utf-8")) + 2)
        + text.encode("utf-8")
        for text in comments
    )
    data = _thumb(tmp_path, source[:2] + markers + source[2:], (8, 8))
    written = [payload for marker, payload in _segments(data) if marker == 0xFE]
    assert written == [b"Alpha: first", b"Description: a description", b"Zeta: replaced"]


def test_non_srgb_thumbnail_matches_exact_qt_oracle(tmp_path):
    _mozjpeg()
    from pathlib import Path

    fixtures = Path(__file__).parent / "fixtures" / "tdexport_icc"
    source = (fixtures / "adobe-thumbnail.source.jpg").read_bytes()
    expected = (fixtures / "adobe-thumbnail.expected.jpg").read_bytes()
    assert _thumb(tmp_path, source, (8, 8)) == expected
