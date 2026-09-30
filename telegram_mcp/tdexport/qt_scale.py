"""QImage::scaled(size, Qt::IgnoreAspectRatio, Qt::SmoothTransformation) on an opaque RGB32 image.

A port of Qt 5.15.19's qSmoothScaleImage (qtbase src/gui/painting/qimagescale.cpp, derived from
Imlib2; LGPL-3.0 / GPL-2.0+ / GPL-3.0), the routine QImage::transformed hands a pure scale to, and
of the SSE2 interpolate_4_pixels in qdrawhelper_p.h that the upscale path calls on x86-64. The
integer arithmetic is reproduced exactly; the SSE4.1 variants Desktop runs compute the same values
as the C ones, because the only difference, 32-bit wrap-around in the C sums, is masked away by
qRgb.

Each axis either averages a box (down: weights in 1/16384 that sum to 16384) or interpolates two
neighbours (up: weights in 1/256). Qt applies both axes per destination pixel; the sums are
separable, so this runs one axis over whole rows, then the other, in the order and with the
truncations Qt uses for each of its four cases.
"""

from importlib import import_module
from typing import Any

Taps = list[tuple[int, list[int]]]


def down_taps(source: int, target: int) -> Taps:
    """(first source index, weights) per target index: qimageCalcXPoints + qimageCalcApoints."""
    inc = (source << 16) // target
    cp = ((target << 14) + source - 1) // source
    taps: Taps = []
    val = 0
    for _ in range(target):
        ap = ((0x10000 - (val & 0xFFFF)) * cp) >> 16
        weights = [ap]
        rest = (1 << 14) - ap
        while rest > cp:  # qt_qimageScaleAARGB_helper's loop
            weights.append(cp)
            rest -= cp
        weights.append(rest)
        taps.append((val >> 16, weights))
        val += inc
    return taps


def up_taps(source: int, target: int) -> Taps:
    """(left/top source index, [256 - a, a]) per target index, the scaling-up branch."""
    inc = (source << 16) // target
    val = 0x8000 * source // target - 0x8000
    taps: Taps = []
    for _ in range(target):
        pos = val >> 16
        weight = 0 if pos < 0 or pos >= source - 1 else (val >> 8) & 0xFF
        taps.append((max(0, pos), [256 - weight, weight]))
        val += inc
    return taps


def _apply(np: Any, pixels: Any, taps: Taps, axis: int) -> Any:
    """Weighted sums along one axis, exact in int64."""
    source = pixels.shape[axis]
    count = max(len(weights) for _, weights in taps)
    index = np.zeros((len(taps), count), dtype=np.int64)
    weight = np.zeros((len(taps), count), dtype=np.int64)
    for row, (start, weights) in enumerate(taps):
        for k, value in enumerate(weights):
            # Only a zero weight ever lands past the edge (the right/bottom up neighbour).
            index[row, k] = min(start + k, source - 1)
            weight[row, k] = value
    moved = np.moveaxis(pixels, axis, 0)
    shape = (len(taps),) + (1,) * (moved.ndim - 1)
    total = np.zeros((len(taps),) + moved.shape[1:], dtype=np.int64)
    for k in range(count):
        total += moved[index[:, k]] * weight[:, k].reshape(shape)
    return np.moveaxis(total, 0, axis)


def smooth_scale(pixels: Any, width: int, height: int) -> Any:
    """Scale an (h, w, channels) uint8 array to (height, width); numpy is imported lazily."""
    np = import_module("numpy")  # by name: numpy's stubs need a 3.12+ type checker

    source_h, source_w = pixels.shape[:2]
    x_up, y_up = width >= source_w, height >= source_h
    if not x_up and not y_up:  # qt_qimageScaleAARGB_down_xy
        rows = _apply(np, pixels, down_taps(source_w, width), 1) >> 4
        result = _apply(np, rows, down_taps(source_h, height), 0) >> 24
    elif not y_up:  # _up_x_down_y: box down the column, then blend two columns
        columns = _apply(np, pixels, down_taps(source_h, height), 0)
        result = (_apply(np, columns, up_taps(source_w, width), 1) >> 8) >> 14
    elif not x_up:  # _down_x_up_y: box along the row, then blend two rows
        rows = _apply(np, pixels, down_taps(source_w, width), 1)
        result = (_apply(np, rows, up_taps(source_h, height), 0) >> 8) >> 14
    else:  # _up_xy: interpolate_4_pixels_sse2 blends vertically first
        rows = _apply(np, pixels, up_taps(source_h, height), 0) >> 8
        result = _apply(np, rows, up_taps(source_w, width), 1) >> 8
    return np.ascontiguousarray(result, dtype=np.uint8)
