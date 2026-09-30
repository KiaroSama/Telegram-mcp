"""Telegram Desktop's export dialog, shared by the chat and secret-chat exports (spec 030/031).

The choices are the OWNER's (owner, 2026-09-30): the client's own form when it offers MCP
elicitation, prefilled with Desktop's defaults; otherwise the agent must pass every choice
explicitly, having asked the owner - nothing is silently defaulted. The folder is judged by
`file_roots` before any network call.
"""

import asyncio
from datetime import datetime
from pathlib import Path
from typing import Optional

from telegram_mcp import file_roots
from telegram_mcp.safeguard import folders
from telegram_mcp.tdexport.settings import Format, MediaSettings, Settings

_FORM_SECONDS = 900
_MB = 1024 * 1024
#: export_view_settings.cpp: the size slider runs from 1 MB to 4000 MB.
_MIN_MB, _MAX_MB = 1, 4000
#: export_view_settings.cpp kOffset: a "till" at or before the "from" lands this far after.
_TILL_OFFSET = 600

_FORMATS = {"html": Format.Html, "json": Format.Json, "html_and_json": Format.HtmlAndJson}
#: The dialog's checkboxes, in its order, with the flag each sets.
_KINDS = {
    "photos": ("Photos", MediaSettings.Type.Photo),
    "videos": ("Videos", MediaSettings.Type.Video),
    "voice_messages": ("Voice messages", MediaSettings.Type.VoiceMessage),
    "video_messages": ("Video messages", MediaSettings.Type.VideoMessage),
    "stickers": ("Stickers", MediaSettings.Type.Sticker),
    "gifs": ("GIFs", MediaSettings.Type.GIF),
    "files": ("Files", MediaSettings.Type.File),
}
_DEFAULT_TYPES = MediaSettings.default_types()


def _form(choices: dict) -> dict:
    """The elicitation schema: Telegram Desktop's export dialog, field for field."""
    properties = {
        name: {
            "type": "boolean",
            "title": title,
            "default": bool(choices[name]),
        }
        for name, (title, _) in _KINDS.items()
    }
    properties["size_limit_mb"] = {
        "type": "integer",
        "title": "Size limit (MB)",
        "minimum": _MIN_MB,
        "maximum": _MAX_MB,
        "default": choices["size_limit_mb"],
    }
    properties["format"] = {
        "type": "string",
        "title": "Format",
        "enum": list(_FORMATS),
        "enumNames": ["Human-readable HTML", "Machine-readable JSON", "Both"],
        "default": choices["format"],
    }
    properties["destination"] = {
        "type": "string",
        "title": "Download path (empty: this server's downloads folder)",
        "default": choices["destination"],
    }
    for name, title in (
        ("date_from", "From (YYYY-MM-DD [HH:MM], empty: the beginning)"),
        ("date_to", "To (YYYY-MM-DD [HH:MM], empty: present)"),
    ):
        properties[name] = {"type": "string", "title": title, "default": choices[name]}
    return {"type": "object", "properties": properties}


async def _ask_form(ctx, choices: dict, title: str) -> Optional[dict]:
    """The owner's answers from the client's form; None when it was dismissed."""
    session = getattr(ctx, "session", None)
    result = await asyncio.wait_for(
        session.elicit_form(
            title, _form(choices), related_request_id=getattr(ctx, "request_id", None)
        ),
        _FORM_SECONDS,
    )
    if result.action != "accept":
        return None
    return {**choices, **(result.content or {})}


def _has_form(ctx) -> bool:
    caps = getattr(getattr(ctx, "session", None), "client_capabilities", None)
    return getattr(caps, "elicitation", None) is not None


def _timestamp(value: str, name: str) -> "tuple[int, Optional[str]]":
    """Local time, as Desktop's calendar: a date alone is the start of that day."""
    text = str(value or "").strip()
    if not text:
        return 0, None
    for pattern in ("%Y-%m-%d %H:%M", "%Y-%m-%d"):
        try:
            return int(datetime.strptime(text, pattern).timestamp()), None
        except ValueError:
            continue
    return 0, f"{name} must look like 2026-09-01 or 2026-09-01 18:30 (local time)."


def _settings(choices: dict) -> "tuple[Optional[Settings], Optional[str]]":
    fmt = _FORMATS.get(str(choices["format"]).strip().lower())
    if fmt is None:
        return None, f"format must be one of {', '.join(_FORMATS)}."
    size = choices["size_limit_mb"]
    if isinstance(size, bool) or not isinstance(size, (int, float)):
        return None, "size_limit_mb must be a number of megabytes."
    if not _MIN_MB <= size <= _MAX_MB:
        return None, f"size_limit_mb must be between {_MIN_MB} and {_MAX_MB}."
    since, error = _timestamp(choices["date_from"], "date_from")
    if error:
        return None, error
    till, error = _timestamp(choices["date_to"], "date_to")
    if error:
        return None, error
    if till and since and till <= since:
        till = since + _TILL_OFFSET
    types = MediaSettings.Type(0)
    for name, (_, flag) in _KINDS.items():
        if choices[name]:
            types |= flag
    media = MediaSettings(types=types, size_limit=int(size * _MB))
    settings = Settings(format=fmt, media=media)
    settings.single_peer_from, settings.single_peer_till = since, till
    return settings, None


def writer_for(fmt: Format):
    """The output writer Desktop's AbstractWriter::Create picks for a format."""
    from telegram_mcp.tdexport import html_and_json, html_writer, json_writer

    if fmt is Format.Json:
        return json_writer.JsonWriter()
    if fmt is Format.Html:
        return html_writer.HtmlWriter()
    return html_and_json.HtmlAndJsonWriter(html_writer.HtmlWriter(), json_writer.JsonWriter())


#: The options a caller passes; `destination` last, as in the dialog.
OPTIONS = (*_KINDS, "size_limit_mb", "format", "date_from", "date_to", "destination")


async def choose(ctx, choices: dict, tool: str, title: str) -> "tuple[Optional[Settings], str]":
    """Settings with the folder resolved, or the sentence to answer with instead.

    Raises asyncio.TimeoutError when the form stays unanswered.
    """
    if _has_form(ctx):
        defaults = {name: bool(_DEFAULT_TYPES & flag) for name, (_, flag) in _KINDS.items()}
        defaults.update(format="html", size_limit_mb=8)
        prefilled = {k: v if v is not None else defaults.get(k) for k, v in choices.items()}
        answered = await _ask_form(ctx, prefilled, title)
        if answered is None:
            return None, "Export cancelled: the owner closed the export dialog."
        choices = answered
    else:
        missing = [k for k, v in choices.items() if v is None]
        if missing:
            return None, (
                "Ask the owner before exporting - this client has no form to show "
                "Telegram Desktop's export dialog. Still needed: "
                + ", ".join(missing)
                + ". Also confirm the period (date_from/date_to, empty = whole "
                "history) and the folder."
            )
    settings, error = _settings(choices)
    if error:
        return None, error
    # `destination`, not `folder`: the safeguard asks the owner about any path argument
    # of that name outside files/, so another folder can be approved on the phone.
    chosen = str(choices["destination"] or "").strip()
    default = folders.default_download_dir()
    base, refusal = await file_roots.resolve_allowed_folder(chosen or str(default), ctx, tool)
    if refusal:
        return None, refusal
    base.mkdir(parents=True, exist_ok=True)
    # export_view_panel_controller.cpp ResolveSettings: the default folder always gets a
    # ChatExport_ subfolder, a folder the owner chose only when it is not empty.
    settings.path = base.as_posix()
    settings.force_sub_path = not chosen or base.resolve() == Path(default).resolve()
    return settings, ""
