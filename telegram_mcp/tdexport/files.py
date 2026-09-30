"""File-name, folder and skip rules for exported media.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_file.cpp
PrepareRelativePath, output/export_output_abstract.cpp NormalizePath, export_api_wrap.cpp
DocumentMediaType/processFileLoad, data/export_data_types.cpp WriteImageThumb, core/mime_type.cpp
MimeTypeForName, lib_base base_file_utilities{,_win}.cpp FileNameFromUserString), GPL-3.0.
"""

from __future__ import annotations

import io
import os
from datetime import date as Date
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Protocol

from .mime_globs import ALIASES, FIRST_GLOBS
from .qt_jpeg import desktop_thumb
from .settings import MediaSettings, Settings


class SkipReason(Enum):
    """Data::File::SkipReason (`None` is a Python keyword, hence `None_`)."""

    None_ = 0
    Unavailable = 1
    FileType = 2
    FileSize = 3
    DateLimits = 4


# Core::MimeTypeForName special cases that never reach QMimeDatabase.
_KNOWN_GLOBS = {
    "image/webp": "*.webp",
    "application/x-tgsticker": "*.tgs",
    "application/x-tgwallpattern": "*.tgv",
    "application/x-tdesktop-theme": "*.tdesktop-theme",
    "application/x-tgtheme-tdesktop": "*.tdesktop-theme",
    "application/x-tdesktop-palette": "*.tdesktop-palette",
}


def mime_first_glob(mime: str) -> str:
    """Core::MimeTypeForName(mime).globPatterns().front(), or "" when there is none."""
    if mime in _KNOWN_GLOBS:
        return _KNOWN_GLOBS[mime]
    if mime == "audio/mpeg3":
        mime = "audio/mp3"
    return FIRST_GLOBS.get(ALIASES.get(mime, mime), "")


_BAD_FILE_NAME_CHARS = frozenset("\u200e\u200f\u202a\u202b\u202d\u202e\u2066\u2067" + '/\\<>:"|?*')
_WINDOWS_BAD_EXTENSIONS = (".lnk", ".scf")
_WINDOWS_BAD_NAMES = (
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{i}" for i in range(1, 10)),
    *(f"LPT{i}" for i in range(1, 10)),
)


def file_name_from_user_string(name: str) -> str:
    """base::FileNameFromUserString with the Windows platform rule (the owner's platform)."""
    name = "".join("_" if ord(ch) < 32 or ch in _BAD_FILE_NAME_CHARS else ch for ch in name)
    if not name or name.endswith(" ") or name.endswith("."):
        name += "_"
    for extension in _WINDOWS_BAD_EXTENSIONS:
        if name.lower().endswith(extension):
            name += ".download"
    upper = name.upper()
    for bad in _WINDOWS_BAD_NAMES:
        if upper.startswith(bad) and (len(name) == len(bad) or name[len(bad)] == "."):
            name = "_" + name
            break
    return name


def prepare_relative_path(folder: str, suggested: str) -> str:
    """Output::File::PrepareRelativePath: "name (N).ext" until the path is free."""
    if not os.path.exists(folder + suggested):
        return suggested
    # Not the last '.' so that "file.tar.xz" won't be messed up.
    position = suggested.find(".")
    base = suggested if position < 0 else suggested[:position]
    extension = "" if position < 0 else suggested[position:]
    attempt = 0
    while True:
        attempt += 1
        relative = f"{base} ({attempt}){extension}"
        if not os.path.exists(folder + relative):
            return relative


def normalize_path(settings: Settings, today: Date | None = None) -> str:
    """Output::NormalizePath: the export folder, "ChatExport_YYYY-MM-DD (N)/" when needed."""
    folder = Path(settings.path)
    path = os.path.abspath(settings.path).replace("\\", "/")
    result = path if path.endswith("/") else path + "/"
    if not settings.force_sub_path:
        # QDir::exists and entryInfoList only see directories.
        if not folder.is_dir() or not any(folder.iterdir()):
            return result
    stamp = (today or Date.today()).isoformat()
    base = ("ChatExport_" if settings.only_single_peer() else "DataExport_") + stamp

    def add(i: int) -> str:
        return base + (f" ({i})" if i else "")

    index = 0
    while os.path.isdir(result + add(index)):
        index += 1
    return result + add(index) + "/"


class _DocumentFlags(Protocol):
    is_sticker: bool
    is_video_message: bool
    is_voice_message: bool
    is_animated: bool
    is_video_file: bool


def document_media_type(document: _DocumentFlags) -> MediaSettings.Type:
    """ApiWrap DocumentMediaType."""
    kind = MediaSettings.Type
    if document.is_sticker:
        return kind.Sticker
    if document.is_video_message:
        return kind.VideoMessage
    if document.is_voice_message:
        return kind.VoiceMessage
    if document.is_animated:
        return kind.GIF
    if document.is_video_file:
        return kind.Video
    return kind.File


def apply_file_policy(
    file: Any,
    skip_by_date: bool,
    media_type: MediaSettings.Type,
    controlling_size: int,
    media: MediaSettings,
) -> bool:
    """The decision half of ApiWrap::processFileLoad (FilePolicy overload).

    Returns True when the file needs no download: it was handled already or got a skip reason.
    """
    if file.relative_path or file.skip_reason != SkipReason.None_:
        return True
    if skip_by_date:
        file.skip_reason = SkipReason.DateLimits
    elif not file.location and not file.content:
        file.skip_reason = SkipReason.Unavailable
    elif (media.types & media_type) != media_type:
        file.skip_reason = SkipReason.FileType
    elif controlling_size > media.size_limit:
        file.skip_reason = SkipReason.FileSize
    else:
        return False
    return True


_MAX_IMAGE_SIZE = 10000


def write_image_thumb(
    base_path: str,
    large_path: str,
    convert_size: Callable[[tuple[int, int]], tuple[int, int]],
    image_format: str | None = None,
    quality: int | None = None,
    postfix: str = "_thumb",
) -> tuple[str, tuple[int, int]]:
    """Data::WriteImageThumb. Returns ("", (0, 0)) wherever tdesktop returns {}.

    Scaling needs Pillow, imported lazily so the package itself stays stdlib + Telethon; without
    it every image counts as unreadable, which is tdesktop's own outcome for such a file. A JPEG
    saved as JPEG goes through qt_jpeg, byte-identical to Desktop when imagecodecs (mozjpeg) is
    installed; anything else, or no imagecodecs, is scaled and saved by Pillow.
    """
    empty: tuple[str, tuple[int, int]] = ("", (0, 0))
    if not large_path:
        return empty
    try:
        from PIL import Image as PilImage
    except ImportError:
        return empty
    try:
        source = Path(base_path + large_path).read_bytes()
        with PilImage.open(io.BytesIO(source)) as reader:
            width, height = reader.size
            if width <= 0 or height <= 0:
                return empty
            if width >= _MAX_IMAGE_SIZE or height >= _MAX_IMAGE_SIZE:
                return empty
            source_format = reader.format or "JPEG"
            if source_format == "MPO":  # a JPEG with a multi-picture APP2, to Qt
                source_format = "JPEG"
            image = reader.copy()
    except (OSError, ValueError):
        return empty
    final_size = convert_size((width, height))
    if final_size[0] <= 0 or final_size[1] <= 0:
        return empty
    last_slash = large_path.rfind("/")
    first_dot = large_path.find(".", last_slash + 1)
    thumb = (
        large_path[:first_dot] + postfix + large_path[first_dot:]
        if first_dot >= 0
        else large_path + postfix
    )
    result = prepare_relative_path(base_path, thumb)
    save_format = (image_format or source_format).upper()
    exact = None
    if save_format == "JPEG" and source_format == "JPEG":
        exact = desktop_thumb(image, source, final_size, quality)
    if exact is not None:
        try:
            Path(base_path + result).write_bytes(exact)
        except OSError:
            return empty
        return result, final_size
    if source_format == "JPEG":
        image = image.convert("RGB")
    image = image.resize(final_size, PilImage.Resampling.BILINEAR)
    options: dict[str, Any] = {}
    if quality is not None and quality >= 0:
        options["quality"] = quality
    try:
        image.save(base_path + result, format=save_format, **options)
    except (OSError, ValueError, KeyError):
        return empty
    return result, final_size


def write_image_thumb_sized(
    base_path: str, large_path: str, width: int, height: int, postfix: str = "_thumb"
) -> str:
    """The WriteImageThumb(basePath, largePath, width, height, postfix) overload."""
    return write_image_thumb(
        base_path, large_path, lambda _size: (width, height), None, None, postfix
    )[0]
