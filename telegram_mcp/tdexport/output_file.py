"""An export output file written block by block.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_file.cpp
File::writeBlock/reopen), GPL-3.0. tdesktop's Result becomes an OSError naming the path; Stats
are not kept.
"""

from __future__ import annotations

import os
from typing import BinaryIO


class File:
    """Output::File: created (or truncated) on the first block, then appended to."""

    def __init__(self, path: str) -> None:
        self._path = path
        self._offset = 0
        self._file: BinaryIO | None = None

    def size(self) -> int:
        return self._offset

    def empty(self) -> bool:
        return not self._offset

    def write_block(self, block: bytes) -> None:
        try:
            self._reopen()
            if block:
                assert self._file is not None
                self._file.write(block)
                self._file.flush()
                self._offset += len(block)
        except OSError:
            self.close()
            raise

    def close(self) -> None:
        if self._file is not None:
            try:
                self._file.close()
            finally:
                self._file = None

    def _reopen(self) -> None:
        if self._file is not None:
            return
        if os.path.exists(self._path):
            if os.path.getsize(self._path) < self._offset:
                raise OSError(f"Export file shrank while writing: {self._path}")
            os.truncate(self._path, self._offset)
        elif self._offset > 0:
            raise OSError(f"Export file vanished while writing: {self._path}")
        folder = os.path.dirname(self._path)
        if folder:
            os.makedirs(folder, exist_ok=True)
        self._file = open(self._path, "ab")
