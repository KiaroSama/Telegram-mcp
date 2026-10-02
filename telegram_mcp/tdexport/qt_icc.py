"""Qt 5.15.19's matrix/TRC ICC conversion, before Desktop thumbnail scaling.

Port of QtGui qicc.cpp, qcolorspace.cpp, qcolormatrix_p.h, qcolortransferfunction_p.h,
qcolortransfertable_p.h, qcolortrclut.cpp and qcolortransform.cpp at c68297f66533200c9ff626b056d096c78b4b94db.
Copyright (C) 2016, 2018 The Qt Company Ltd. Licensed under LGPL-3.0 or GPL-2.0-or-later /
GPL-3.0 (this project's GPL-3.0 option). Modified to Python/NumPy; no Qt runtime dependency.

Only Qt's supported RGB/gray XYZ matrix forms convert. Invalid/unsupported profiles and equal
sRGB spaces keep the original bytes. Float32 operations are deliberately separate: BLAS or a
fused multiply-add changes Qt's SSE2 rounding. CPU only: no GPU path preserves this arithmetic.
"""

import struct
from functools import lru_cache
from importlib import import_module
from typing import Any

# QIcc::toIccProfile(SRgb) is invariant: all header dates/IDs are zero, creator is Qt 5.15,
# matrix/TRC/description are fixed. Captured exact-source oracle bytes, checked in fixtures.
_SRGB_PROFILE = bytes.fromhex(
    "000001cb00000000024000006d6e74725247422058595a20000000000000000000000000616373700000000000000000"
    "00000000000000000000000000000000000000010000f6d6000100000000d32d5174050f000000000000000000000000"
    "0000000000000000000000000000000000000000000000000000000000000000000000097258595a000000f000000014"
    "6758595a00000104000000146258595a0000011800000014777470740000012c0000001463707274000001400000000c"
    "725452430000014c00000020675452430000014c00000020625452430000014c00000020646573630000016c0000005f"
    "58595a200000000000006f9f000038f40000039158595a2000000000000062960000b787000018dc58595a2000000000"
    "000024a100000f850000b6d358595a20000000000000f34f00010000000116c274657874000000004e2f410070617261"
    "0000000000030000000266660000f2a700000d59000013d000000a5b6465736300000000000000057352474200000000"
    "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    "000000000000000000000000000000000000000000000000000000"
)
_PRIMARIES = (
    ((0.64, 0.33), (0.3, 0.6), (0.15, 0.06), (0.31271, 0.32902)),
    ((0.64, 0.33), (0.21, 0.71), (0.15, 0.06), (0.31271, 0.32902)),
    ((0.68, 0.32), (0.265, 0.69), (0.15, 0.06), (0.31271, 0.32902)),
    ((0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001), (0.34567, 0.35850)),
)
_RECOGNIZED = (
    (
        (0.4360217452, 0.2224751115, 0.0139281144),
        (0.3851087987, 0.7169067264, 0.0971015394),
        (0.1430812478, 0.0606181994, 0.7141585946),
    ),
    (
        (0.6097189188, 0.3111021519, 0.0194766335),
        (0.2052682191, 0.6256770492, 0.0608891509),
        (0.1492247432, 0.0632209629, 0.7448224425),
    ),
    (
        (0.5150973201, 0.2411795557, -0.0010491034),
        (0.2919696569, 0.6922441125, 0.0418830328),
        (0.1571449190, 0.0665764511, 0.7843542695),
    ),
    (
        (0.7976672649, 0.2880374491, 0),
        (0.1351922452, 0.7118769884, 0),
        (0.0313525312, 0.0000856627, 0.8251883388),
    ),
)


def _map(matrix: Any, values: Any) -> Any:
    return (values[..., 0:1] * matrix[0] + values[..., 1:2] * matrix[1]) + values[
        ..., 2:3
    ] * matrix[2]


def _multiply(left: Any, right: Any) -> Any:
    return _map(left, right)


def _det(matrix: Any) -> Any:
    r, g, b = matrix
    return (r[0] * (b[2] * g[1] - g[2] * b[1]) - r[1] * (b[2] * g[0] - g[2] * b[0])) + r[2] * (
        b[1] * g[0] - g[1] * b[0]
    )


def _inverse(matrix: Any, np: Any) -> Any:
    r, g, b = matrix
    inv = np.array(
        (
            (g[1] * b[2] - b[1] * g[2], b[1] * r[2] - r[1] * b[2], r[1] * g[2] - g[1] * r[2]),
            (b[0] * g[2] - g[0] * b[2], r[0] * b[2] - b[0] * r[2], g[0] * r[2] - r[0] * g[2]),
            (g[0] * b[1] - b[0] * g[1], b[0] * r[1] - r[0] * b[1], r[0] * g[1] - g[0] * r[1]),
        ),
        dtype=np.float32,
    )
    return inv * (np.float32(1) / _det(matrix))


def _xyz(point: tuple[float, float], np: Any) -> Any:
    x, y = point
    return np.array((x / y, 1, (1 - x - y) / y), dtype=np.float32)


def _matrix(points: Any, np: Any) -> Any:
    white = _xyz(points[3], np)
    d50 = _xyz((0.34567, 0.35850), np)
    matrix = np.array([_xyz(point, np) for point in points[:3]])
    matrix = _multiply(matrix, np.diag(_map(_inverse(matrix, np), white)))
    if not np.all(np.abs(white - d50) < np.float32(1 / 2048)):
        brad = np.array(
            ((0.8951, -0.7502, 0.0389), (0.2664, 1.7135, -0.0685), (-0.1614, 0.0367, 1.0296)),
            dtype=np.float32,
        )
        inv = np.array(
            (
                (0.9869929, 0.4323053, -0.0085287),
                (-0.1470543, 0.5183603, 0.0400428),
                (0.1599627, 0.0492912, 0.9684867),
            ),
            dtype=np.float32,
        )
        adaptation = _multiply(inv, _multiply(np.diag(_map(brad, d50) / _map(brad, white)), brad))
        matrix = _multiply(adaptation, matrix)
    return matrix


def _srgb(np: Any) -> Any:
    f = np.float32
    return np.array(
        (f(1) / f(1.055), f(0.055) / f(1.055), f(1) / f(12.92), f(0.04045), 0, 0, f(2.4)),
        dtype=np.float32,
    )


def _gamma(g: float, np: Any) -> Any:
    return np.array((1, 0, 0, 0, 0, 0, g), dtype=np.float32)


def _matches(a: Any, b: Any, np: Any) -> bool:
    return a.shape == b.shape and bool(np.all(np.abs(a - b) <= np.float32(1 / 512)))


def _same_curve(a: Any, b: Any, np: Any) -> bool:
    if a[0] != b[0]:
        return False
    return bool(np.array_equal(a[1], b[1])) if a[0] else _matches(a[1], b[1], np)


def _curve(tag: bytes, np: Any) -> tuple[bool, Any] | None:
    if tag[:4] == b"curv":
        count = struct.unpack_from(">I", tag, 8)[0]
        if count > 65536 or len(tag) < 12 + 2 * count:
            return None
        if count == 0:
            return False, np.array((1, 0, 1, 0, 0, 0, 1), dtype=np.float32)
        if count == 1:
            return False, _gamma(struct.unpack_from(">H", tag, 12)[0] / 256, np)
        table = np.frombuffer(tag, dtype=">u2", count=count, offset=12).astype(np.float32)
        if np.any(table[1:] < table[:-1]):
            return None
        if table[0] == 0 and table[-1] == 65535:
            if count == 2:
                return False, np.array((1, 0, 1, 0, 0, 0, 1), dtype=np.float32)
            heuristics = {
                26: ((6, 12, 18), (3062, 12824, 31237)),
                1024: ((257, 513, 768), (3366, 14116, 34318)),
                4096: ((515, 1025, 2051), (960, 3342, 14079)),
            }
            if count in heuristics:
                indices, values = heuristics[count]
                if np.array_equal(table[list(indices)], values):
                    return False, _srgb(np)
        return True, table
    if tag[:4] != b"para" or len(tag) < 16:
        return None
    kind = struct.unpack_from(">H", tag, 8)[0]
    lengths = (1, 3, 4, 5, 7)
    if kind > 4 or len(tag) < 12 + 4 * lengths[kind]:
        return None
    p = np.array(
        struct.unpack_from(">" + "i" * lengths[kind], tag, 12), dtype=np.float32
    ) / np.float32(65536)
    if kind == 0:
        return False, _gamma(p[0], np)
    g, a, b = p[:3]
    if kind in (1, 2):
        if a == 0:
            return None
        c = p[3] if kind == 2 else np.float32(0)
        return False, np.array((a, b, 0, -b / a, c, c, g), dtype=np.float32)
    c, d = p[3:5]
    e, f = p[5:7] if kind == 4 else (0, 0)
    return False, np.array((a, b, c, d, e, f, g), dtype=np.float32)


def _parse(profile: bytes, np: Any) -> tuple[Any, list[Any]] | None:
    if len(profile) < 132 or profile[36:40] != b"acsp" or profile[20:24] != b"XYZ ":
        return None
    size, count = struct.unpack_from(">I", profile)[0], struct.unpack_from(">I", profile, 128)[0]
    gray = profile[16:20] == b"GRAY"
    if profile[16:20] not in (b"RGB ", b"GRAY") or profile[12:16] not in (
        (b"mntr", b"scnr", b"prtr") if gray else (b"mntr", b"scnr")
    ):
        return None
    if not 132 + count * 12 <= size <= len(profile):
        return None
    illuminant = np.array(struct.unpack_from(">iii", profile, 68), dtype=np.float32) / np.float32(
        65536
    )
    if not np.all(np.abs(illuminant - _xyz((0.34567, 0.35850), np)) < np.float32(1 / 2048)):
        return None
    tags = {}
    for i in range(count):
        key, offset, length = struct.unpack_from(">4sII", profile, 132 + i * 12)
        if offset < 132 + count * 12 or offset % 4 or length < 12 or offset + length > size:
            return None
        tags[key] = profile[offset : offset + length]
    required = (
        (b"wtpt", b"kTRC")
        if gray
        else (b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC")
    )
    if any(key not in tags for key in required):
        return None
    xyz = {}
    for key in ((b"wtpt",) if gray else (b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ")):
        tag = tags[key]
        if len(tag) < 20 or tag[:4] != b"XYZ ":
            return None
        xyz[key] = np.array(struct.unpack_from(">iii", tag, 8), dtype=np.float32) / np.float32(
            65536
        )
    if gray:
        white = xyz[b"wtpt"]
        if (
            abs(white[1] - np.float32(1)) > np.float32(1e-5)
            or (np.float32(1) + white[2]) + white[0] == 0
        ):
            return None
        if np.all(np.abs(white - _xyz((0.31271, 0.32902), np)) < np.float32(1 / 2048)):
            matrix = _matrix(_PRIMARIES[0], np)
        else:
            y = np.float32(1) / ((np.float32(1) + white[2]) + white[0])
            x = white[0] * y
            if not (0 <= x <= 1 and 0 < y <= 1 and float(x) + float(y) <= 1):
                return None
            matrix = _matrix((*_PRIMARIES[0][:3], (float(x), float(y))), np)
        keys = (b"kTRC",) * 3
    else:
        matrix = np.array([xyz[key] for key in (b"rXYZ", b"gXYZ", b"bXYZ")])
        for points, known in zip(_PRIMARIES, _RECOGNIZED):
            if np.all(np.abs(matrix - np.array(known, dtype=np.float32)) < np.float32(1 / 2048)):
                matrix = _matrix(points, np)
                break
        keys = (
            (b"aarg", b"aagg", b"aabg")
            if all(key in tags for key in (b"aarg", b"aagg", b"aabg"))
            else (b"rTRC", b"gTRC", b"bTRC")
        )
    curves = [_curve(tags[key], np) for key in keys]
    if any(curve is None for curve in curves) or abs(_det(matrix)) <= np.float32(1e-5):
        return None
    if all(not curve[0] for curve in curves) and all(
        _matches(curves[0][1], curve[1], np) for curve in curves[1:]
    ):
        fun = curves[0][1]
        if np.all(
            np.abs(fun[[0, 1, 3, 4]] - np.array((1, 0, 0, 0), dtype=np.float32))
            <= np.float32(1 / 512)
        ):
            fun = _gamma(1 if abs(fun[6] - np.float32(1)) <= np.float32(1e-5) else fun[6], np)
        elif _matches(fun, _srgb(np), np):
            fun = _srgb(np)
        curves = [(False, fun)] * 3
    return matrix, curves


def _apply_function(fun: Any, x: Any, np: Any) -> Any:
    a, b, c, d, e, f, g = fun
    # std::pow(float,float) returns float, not a float64 intermediate used in later operations.
    base = a * x + b
    powered = np.power(base.astype(np.float64), float(g)).astype(np.float32)
    return np.where(x < d, c * x + f, powered + e)


def _lut(curve: Any, np: Any) -> Any:
    table, values = curve
    x = np.arange(256, dtype=np.float32) / np.float32(255)
    if table:
        at = x * np.float32(len(values) - 1)
        lo = np.floor(at).astype(np.int32)
        hi = np.minimum(lo + 1, len(values) - 1)
        fraction = at - lo.astype(np.float32)
        y = (values[lo] * (np.float32(1) - fraction) + values[hi] * fraction) * np.float32(
            1 / 65535
        )
        return np.clip(np.floor(y * np.float32(65280) + np.float32(0.5)), 0, 65280).astype(
            np.uint16
        )
    y = _apply_function(values, x, np)
    if not np.all(np.isfinite(y)):
        raise ValueError("ICC curve produced non-finite values")
    # Qt ushort conversion wraps, unlike the table path's qBound.
    scaled = y * np.float32(65280)
    negative_base = np.trunc(scaled - np.float32(1))
    rounded = np.where(
        scaled < 0,
        np.trunc(scaled - negative_base + np.float32(0.5)) + negative_base,
        np.trunc(scaled + np.float32(0.5)),
    )
    return np.remainder(rounded, 65536).astype(np.uint16)


@lru_cache(maxsize=1)
def _target_lut() -> Any:
    np = import_module("numpy")
    fun = _srgb(np)
    a, b, c, d, e, f, g = fun
    inv_a = np.float32(float(np.float32(1) / a) ** float(g))
    inverse = np.array(
        (inv_a, -inv_a * e, np.float32(1) / c, c * d + f, -b / a, -f / c, np.float32(1) / g),
        dtype=np.float32,
    )
    x = (np.arange(4081, dtype=np.float64) / 4080).astype(np.float32)
    return np.floor(_apply_function(inverse, x, np) * np.float32(65280) + np.float32(0.5)).astype(
        np.uint16
    )


def convert_to_srgb(pixels: Any, profile: bytes) -> tuple[Any, bytes]:
    """Return Qt-converted uint8 RGB samples and emitted ICC; preserve bytes on Qt no-op."""
    if not profile:
        return pixels, profile
    np = import_module("numpy")
    parsed = _parse(profile, np)
    if parsed is None:
        return pixels, profile
    matrix, curves = parsed
    target = _matrix(_PRIMARIES[0], np)
    if np.all(np.abs(matrix - target) < np.float32(1 / 2048)) and all(
        not curve[0] and _matches(curve[1], _srgb(np), np) for curve in curves
    ):
        return pixels, profile
    if pixels.dtype != np.uint8 or (
        pixels.ndim != 2 and (pixels.ndim != 3 or pixels.shape[-1] != 3)
    ):
        raise ValueError("ICC conversion requires a uint8 RGB or grayscale array")
    grayscale = pixels.ndim == 2
    samples = np.repeat(pixels[..., None], 3, axis=-1) if grayscale else pixels
    transform = _multiply(_inverse(target, np), matrix)
    if abs(_det(transform)) <= np.float32(1e-5):
        return pixels, profile
    try:
        with np.errstate(invalid="ignore", over="ignore"):
            tables = [_lut(curve, np) for curve in curves]
            if all(_same_curve(curves[0], curve, np) for curve in curves[1:]):
                tables = [tables[0]] * 3
    except ValueError:
        return pixels, profile
    linear = np.stack([tables[i][samples[..., i]] for i in range(3)], axis=-1).astype(
        np.float32
    ) * np.float32(1 / 65280)
    if not np.all(np.abs(transform - np.eye(3, dtype=np.float32)) < np.float32(1 / 2048)):
        linear = np.clip(_map(transform, linear), 0, 1)
    # SSE2 cvtps_epi32 rounds nearest-even; the following ushort -> byte adds 0x80 then >>8.
    indices = np.rint(linear * np.float32(4080)).astype(np.int32)
    if np.any(indices < 0) or np.any(indices > 4080):
        # Out-of-range curves would access native Qt's LUT outside its defined domain.
        return pixels, profile
    converted = ((_target_lut()[indices].astype(np.uint32) + 128) >> 8).astype(np.uint8)
    if grayscale:
        # QImage::applyColorTransform restores Grayscale8 via qGray after RGB32 conversion.
        rgb = converted.astype(np.uint16)
        converted = ((rgb[..., 0] * 11 + rgb[..., 1] * 16 + rgb[..., 2] * 5) >> 5).astype(np.uint8)
    return converted, _SRGB_PROFILE
