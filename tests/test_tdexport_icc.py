"""Synthetic ICC cases checked against Desktop's exact patched Qt 5.15.19 image engine.

The public seam is convert_to_srgb, before resizing/encoding. No native oracle or network is
needed in CI; captured, non-sensitive oracle pixels and profiles are maintained fixtures.
"""

from pathlib import Path
import struct

import pytest

from telegram_mcp.tdexport.qt_icc import convert_to_srgb


@pytest.fixture
def np():
    return pytest.importorskip(
        "numpy", reason="the optional Windows thumbnail codec supplies NumPy"
    )


_FIXTURES = Path(__file__).parent / "fixtures" / "tdexport_icc"


def _pixels(name):
    return bytes.fromhex((_FIXTURES / (name + ".hex")).read_text(encoding="utf-8"))


def test_adobe_rgb_matches_exact_qt_pixels_and_generated_profile(np):
    pixels = np.frombuffer(_pixels("source.rgb"), dtype=np.uint8).reshape(16, 16, 3)
    profile = (_FIXTURES / "adobe-rgb.icc").read_bytes()
    converted, emitted = convert_to_srgb(pixels, profile)
    assert converted.tobytes() == _pixels("adobe-rgb.rgb")
    assert emitted == (_FIXTURES / "srgb.icc").read_bytes()
    assert not np.array_equal(converted, pixels)


@pytest.mark.parametrize(
    "case",
    [
        "parametric-0",
        "parametric-1",
        "parametric-2",
        "parametric-3",
        "parametric-4",
        "table",
        "table-offset",
        "linear-table",
        "linear",
        "gamma88",
        "channels",
        "apple",
        "custom-matrix",
        "gray",
        "srgb",
    ],
)
def test_supported_curves_match_exact_qt(case, np):
    pixels = np.frombuffer(_pixels(case + ".source.rgb"), dtype=np.uint8).reshape(16, 16, 3)
    original = (_FIXTURES / (case + ".icc")).read_bytes()
    converted, emitted = convert_to_srgb(pixels, original)
    assert converted.tobytes() == _pixels(case + ".rgb")
    assert emitted == (_FIXTURES / "srgb.icc").read_bytes()
    if case == "srgb":
        assert converted is pixels
        assert emitted is original


@pytest.mark.parametrize(
    "mutation",
    [
        "empty",
        "truncated",
        "signature",
        "class",
        "cmyk",
        "lab",
        "illuminant",
        "count",
        "offset",
        "alignment",
        "tag-size",
        "missing-matrix",
        "xyz-type",
        "singular",
        "curve-type",
        "parametric-type",
        "short-parameters",
        "zero-a",
        "table-count",
        "table-order",
    ],
)
def test_invalid_or_unsupported_icc_preserves_original_bytes(mutation, np):
    profile = bytearray((_FIXTURES / "parametric-1.icc").read_bytes())
    count = struct.unpack_from(">I", profile, 128)[0]
    tags = {
        key: (132 + i * 12, offset)
        for i in range(count)
        for key, offset, _ in [struct.unpack_from(">4sII", profile, 132 + i * 12)]
    }
    trc_index, trc_offset = tags[b"rTRC"]
    if mutation == "empty":
        profile = bytearray()
    elif mutation == "truncated":
        del profile[-1:]
    elif mutation == "signature":
        profile[36:40] = b"nope"
    elif mutation == "class":
        profile[12:16] = b"link"
    elif mutation == "cmyk":
        profile[16:20] = b"CMYK"
    elif mutation == "lab":
        profile[20:24] = b"Lab "
    elif mutation == "illuminant":
        profile[68:80] = bytes(12)
    elif mutation == "count":
        struct.pack_into(">I", profile, 128, 0xFFFFFFFF)
    elif mutation == "offset":
        struct.pack_into(">I", profile, 136, 128)
    elif mutation == "alignment":
        struct.pack_into(">I", profile, 136, tags[b"rXYZ"][1] + 1)
    elif mutation == "tag-size":
        struct.pack_into(">I", profile, 140, 8)
    elif mutation == "missing-matrix":
        profile[132:136] = b"A2B0"
    elif mutation == "xyz-type":
        offset = tags[b"rXYZ"][1]
        profile[offset : offset + 4] = b"Lab "
    elif mutation == "singular":
        for key in (b"rXYZ", b"gXYZ", b"bXYZ"):
            offset = tags[key][1]
            profile[offset + 8 : offset + 20] = bytes(12)
    elif mutation == "curve-type":
        profile[trc_offset : trc_offset + 4] = b"mft2"
    elif mutation == "parametric-type":
        struct.pack_into(">H", profile, trc_offset + 8, 5)
    elif mutation == "short-parameters":
        struct.pack_into(">I", profile, trc_index + 8, 16)
    elif mutation == "zero-a":
        struct.pack_into(">i", profile, trc_offset + 16, 0)
    else:
        profile = bytearray((_FIXTURES / "table.icc").read_bytes())
        offset = struct.unpack_from(">I", profile, trc_index + 4)[0]
        if mutation == "table-count":
            struct.pack_into(">I", profile, offset + 8, 65537)
        else:
            struct.pack_into(">HH", profile, offset + 12, 65535, 0)
    original = bytes(profile)
    pixels = np.zeros((2, 2, 3), dtype=np.uint8)
    result, emitted = convert_to_srgb(pixels, original)
    assert result is pixels
    assert emitted is original


@pytest.mark.parametrize("case", ["gray-jpeg", "gray-adobe"])
def test_grayscale_jpeg_restores_original_format_after_qt_transform(case, np):
    pixels = np.frombuffer(_pixels(case + ".source.gray"), dtype=np.uint8).reshape(16, 16)
    converted, emitted = convert_to_srgb(pixels, (_FIXTURES / (case + ".icc")).read_bytes())
    assert converted.shape == pixels.shape
    assert converted.tobytes() == _pixels(case + ".gray")
    assert emitted == (_FIXTURES / "srgb.icc").read_bytes()


def test_out_of_range_parametric_curve_preserves_original_instead_of_indexing_past_lut(np):
    profile = bytearray((_FIXTURES / "parametric-4.icc").read_bytes())
    srgb = (_FIXTURES / "srgb.icc").read_bytes()

    def tags(data):
        return {
            key: offset
            for i in range(struct.unpack_from(">I", data, 128)[0])
            for key, offset, _ in [struct.unpack_from(">4sII", data, 132 + i * 12)]
        }

    locations, target = tags(profile), tags(srgb)
    for key in (b"rXYZ", b"gXYZ", b"bXYZ"):
        profile[locations[key] : locations[key] + 20] = srgb[target[key] : target[key] + 20]
    for key in (b"rTRC", b"gTRC", b"bTRC"):
        offset = locations[key]
        struct.pack_into(">H", profile, offset + 8, 4)
        struct.pack_into(">7i", profile, offset + 12, 65536, 65536, 0, 65536, 0, 256, 0)
    original = bytes(profile)
    pixels = np.full((1, 1, 3), 255, dtype=np.uint8)
    result, emitted = convert_to_srgb(pixels, original)
    assert result is pixels
    assert emitted is original
