"""Album geometry for rich-message collages.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/ui/grouped_layout.cpp Layouter and
ComplexLayouter, ui/grouped_layout_geometry.cpp LayoutMediaGroupGeometry) and Qt's
QRect::united, GPL-3.0 / LGPL-3.0. A rect is (x, y, width, height).
"""

from __future__ import annotations

import math

Rect = tuple[int, int, int, int]
Size = tuple[int, int]


def _round(value: float) -> int:
    """base::SafeRound: std::round (half away from zero), 0 for NaN."""
    if math.isnan(value):
        return 0
    return int(math.floor(value + 0.5)) if value >= 0 else -int(math.floor(-value + 0.5))


def _clamp(value: float, low: float, high: float) -> float:
    return low if value < low else high if high < value else value


class _Layouter:
    def __init__(self, sizes: list[Size], max_width: int, min_width: int, spacing: int) -> None:
        self.sizes = sizes
        self.ratios = [w / float(h) for w, h in sizes]
        self.proportions = "".join(
            "w" if r > 1.2 else "n" if r < 0.8 else "q" for r in self.ratios
        )
        self.count = len(self.ratios)
        # All apps currently use square max size first.
        self.max_width = max_width
        self.max_height = max_width
        self.min_width = min_width
        self.spacing = spacing
        self.average_ratio = (1.0 + sum(self.ratios)) / self.count if self.count else 1.0
        self.max_size_ratio = self.max_width / float(self.max_height)

    def layout(self) -> list[Rect]:
        if not self.count:
            return []
        if self.count == 1:
            return self._one()
        if self.count >= 5 or any(r > 2 for r in self.ratios):
            return _complex(
                self.ratios, self.average_ratio, self.max_width, self.min_width, self.spacing
            )
        if self.count == 2:
            if (
                self.proportions == "ww"
                and self.average_ratio > 1.4 * self.max_size_ratio
                and self.ratios[1] - self.ratios[0] < 0.2
            ):
                return self._two_top_bottom()
            if self.proportions in ("ww", "qq"):
                return self._two_left_right_equal()
            return self._two_left_right()
        if self.count == 3:
            if self.proportions[0] == "n":
                return self._three_left_and_other()
            return self._three_top_and_other()
        if self.proportions[0] == "w":
            return self._four_top_and_other()
        return self._four_left_and_other()

    def _one(self) -> list[Rect]:
        width = self.max_width
        return [(0, 0, width, _cdiv(self.sizes[0][1] * width, self.sizes[0][0]))]

    def _two_top_bottom(self) -> list[Rect]:
        r, width, sp = self.ratios, self.max_width, self.spacing
        height = _round(min(width / r[0], min(width / r[1], (self.max_height - sp) / 2.0)))
        return [(0, 0, width, height), (0, height + sp, width, height)]

    def _two_left_right_equal(self) -> list[Rect]:
        r, sp = self.ratios, self.spacing
        width = _cdiv(self.max_width - sp, 2)
        height = _round(min(width / r[0], min(width / r[1], self.max_height * 1.0)))
        return [(0, 0, width, height), (width + sp, 0, width, height)]

    def _two_left_right(self) -> list[Rect]:
        r, sp, mw = self.ratios, self.spacing, self.max_width
        minimal_width = _round(self.min_width * 1.5)
        second_width = min(
            _round(max(0.4 * (mw - sp), (mw - sp) / r[0] / (1.0 / r[0] + 1.0 / r[1]))),
            mw - sp - minimal_width,
        )
        first_width = mw - second_width - sp
        height = min(self.max_height, _round(min(first_width / r[0], second_width / r[1])))
        return [(0, 0, first_width, height), (first_width + sp, 0, second_width, height)]

    def _three_left_and_other(self) -> list[Rect]:
        r, sp, mw, mh = self.ratios, self.spacing, self.max_width, self.max_height
        first_height = mh
        third_height = _round(min((mh - sp) / 2.0, (r[1] * (mw - sp) / (r[2] + r[1]))))
        second_height = first_height - third_height - sp
        right_width = max(
            self.min_width,
            _round(min((mw - sp) / 2.0, min(third_height * r[2], second_height * r[1]))),
        )
        left_width = min(_round(first_height * r[0]), mw - sp - right_width)
        return [
            (0, 0, left_width, first_height),
            (left_width + sp, 0, right_width, second_height),
            (left_width + sp, second_height + sp, right_width, third_height),
        ]

    def _three_top_and_other(self) -> list[Rect]:
        r, sp, mw, mh = self.ratios, self.spacing, self.max_width, self.max_height
        first_width = mw
        first_height = _round(min(first_width / r[0], (mh - sp) * 0.66))
        second_width = _cdiv(mw - sp, 2)
        second_height = min(
            mh - first_height - sp, _round(min(second_width / r[1], second_width / r[2]))
        )
        third_width = first_width - second_width - sp
        return [
            (0, 0, first_width, first_height),
            (0, first_height + sp, second_width, second_height),
            (second_width + sp, first_height + sp, third_width, second_height),
        ]

    def _four_top_and_other(self) -> list[Rect]:
        r, sp, mw, mh = self.ratios, self.spacing, self.max_width, self.max_height
        w = mw
        h0 = _round(min(w / r[0], (mh - sp) * 0.66))
        h = _round((mw - 2 * sp) / (r[1] + r[2] + r[3]))
        w0 = max(self.min_width, _round(min((mw - 2 * sp) * 0.4, h * r[1])))
        w2 = _round(max(max(self.min_width * 1.0, (mw - 2 * sp) * 0.33), h * r[3]))
        w1 = w - w0 - w2 - 2 * sp
        h1 = min(mh - h0 - sp, h)
        return [
            (0, 0, w, h0),
            (0, h0 + sp, w0, h1),
            (w0 + sp, h0 + sp, w1, h1),
            (w0 + sp + w1 + sp, h0 + sp, w2, h1),
        ]

    def _four_left_and_other(self) -> list[Rect]:
        r, sp, mw, mh = self.ratios, self.spacing, self.max_width, self.max_height
        h = mh
        w0 = _round(min(h * r[0], (mw - sp) * 0.6))
        w = _round((mh - 2 * sp) / (1.0 / r[1] + 1.0 / r[2] + 1.0 / r[3]))
        h0 = _round(w / r[1])
        h1 = _round(w / r[2])
        h2 = h - h0 - h1 - 2 * sp
        w1 = max(self.min_width, min(mw - w0 - sp, w))
        return [
            (0, 0, w0, h),
            (w0 + sp, 0, w1, h0),
            (w0 + sp, h0 + sp, w1, h1),
            (w0 + sp, h0 + h1 + 2 * sp, w1, h2),
        ]


def _cdiv(a: int, b: int) -> int:
    """C++ integer division (truncation toward zero)."""
    quotient = abs(a) // abs(b)
    return quotient if (a >= 0) == (b >= 0) else -quotient


def _complex(
    source: list[float], average_ratio: float, max_width: int, min_width: int, spacing: int
) -> list[Rect]:
    """ComplexLayouter::layout."""
    ratios = [
        _clamp(r, 1.0, 2.75) if average_ratio > 1.1 else _clamp(r, 0.6667, 1.0) for r in source
    ]
    count = len(ratios)
    max_height = _cdiv(max_width * 4, 3)

    def multi_height(offset: int, line: int) -> float:
        return (max_width - (line - 1) * spacing) / sum(ratios[offset : offset + line])

    attempts: list[tuple[list[int], list[float]]] = []

    def push(line_counts: list[int]) -> None:
        heights, offset = [], 0
        for line in line_counts:
            heights.append(multi_height(offset, line))
            offset += line
        attempts.append((line_counts, heights))

    for first in range(1, count):
        second = count - first
        if first <= 3 and second <= 3:
            push([first, second])
    for first in range(1, count - 1):
        for second in range(1, count - first):
            third = count - first - second
            if first <= 3 and second <= (4 if average_ratio < 0.85 else 3) and third <= 3:
                push([first, second, third])
    for first in range(1, count - 1):
        for second in range(1, count - first):
            for third in range(1, count - first - second):
                fourth = count - first - second - third
                if max(first, second, third, fourth) <= 3:
                    push([first, second, third, fourth])

    optimal: tuple[list[int], list[float]] | None = None
    optimal_diff = 0.0
    for counts, heights in attempts:
        total_height = sum(heights) + spacing * (len(counts) - 1)
        bad1 = 1.5 if min(heights) < min_width else 1.0
        bad2 = 1.5 if any(counts[i - 1] > counts[i] for i in range(1, len(counts))) else 1.0
        diff = abs(total_height - max_height) * bad1 * bad2
        if optimal is None or diff < optimal_diff:
            optimal, optimal_diff = (counts, heights), diff
    if optimal is None:
        raise ValueError("No layout attempt in ComplexLayouter::layout.")

    result: list[Rect] = []
    index, y = 0, 0.0
    for row, (columns, line_height) in enumerate(zip(*optimal)):
        height = _round(line_height)
        x = 0
        for column in range(columns):
            width = max_width - x if column == columns - 1 else _round(ratios[index] * line_height)
            result.append((x, int(y), width, height))
            x += width + spacing
            index += 1
        y += height + spacing
    return result


def layout_media_group_geometry(
    sizes: list[Size], max_width: int, min_width: int, spacing: int
) -> list[Rect]:
    return _Layouter(sizes, max_width, min_width, spacing).layout()


def united(first: Rect | None, second: Rect) -> Rect:
    """QRect::united; None stands for the null QRect()."""
    if first is None or (first[2] == 0 and first[3] == 0):
        return second
    if second[2] == 0 and second[3] == 0:
        return first

    def span(start: int, length: int) -> tuple[int, int]:
        end = start + length - 1
        return (end, start) if end - start + 1 < 0 else (start, end)

    l1, r1 = span(first[0], first[2])
    l2, r2 = span(second[0], second[2])
    t1, b1 = span(first[1], first[3])
    t2, b2 = span(second[1], second[3])
    left, right, top, bottom = min(l1, l2), max(r1, r2), min(t1, t2), max(b1, b2)
    return (left, top, right - left + 1, bottom - top + 1)
