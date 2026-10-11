"""Approved one-time archive defaults, with private destinations before startup."""

import os
from pathlib import Path

from telegram_mcp import file_roots
from telegram_mcp.handles import is_link, open_allowed_directory
from telegram_mcp.owner_only import restrict_to_owner_strict, verify_owner_only

MARKER = "mcp_archive_policy"
VERSION = 1
_pins = {}


def pin(manager, folder):
    """Retain the Windows directory identity while the pathname writer is active."""
    if os.name != "nt" or folder is None:
        return
    path = Path(folder)
    held = open_allowed_directory(path, [path])
    held.verify()
    previous = _pins.pop(id(manager), None)
    _pins[id(manager)] = held
    if previous is not None:
        previous.close()


def unpin(manager):
    held = _pins.pop(id(manager), None)
    if held is not None:
        held.close()


def _setting(storage):
    setting = storage.load_setting("auto_save_secret_chats")
    if setting is None:
        return {"folder": None, "on": False}
    if not isinstance(setting, dict) or type(setting.get("on")) is not bool:
        raise ValueError("Invalid secret archive preference; state was preserved")
    folder = setting.get("folder")
    if folder is not None and (not isinstance(folder, str) or not folder.strip()):
        raise ValueError("Invalid secret archive destination; state was preserved")
    return {"folder": folder, "on": setting["on"]}


async def private_destination(folder, account, ctx=None):
    if folder is None:
        folder = str(file_roots.folders.default_download_dir() / "secret-chats" / account)
    raw = Path(folder)
    if not raw.is_absolute():
        raise ValueError("Stored secret archive destination must be absolute")
    for parent in (raw, *raw.parents):
        if parent.exists() and is_link(parent.lstat()):
            raise ValueError("Secret archive destination must not traverse links")
    path, error = await file_roots.resolve_allowed_folder(str(raw), ctx, "secret archive")
    if error:
        raise ValueError(error)
    roots = await file_roots._get_effective_allowed_roots(ctx)
    with open_allowed_directory(path, roots, create=True) as held:
        if not restrict_to_owner_strict(path):
            raise OSError("Secret archive directory could not be made owner-only")
        held.verify()
        for child in path.rglob("*"):
            if is_link(child.lstat()):
                raise OSError("Existing secret archive contents must not be links")
            if child.is_dir() and not restrict_to_owner_strict(child):
                raise OSError("Existing secret archive directory could not be made owner-only")
            if not verify_owner_only(child):
                raise OSError("Existing secret archive contents are not safely owner-only")
        return path


async def prepare(storage, account):
    setting = _setting(storage)
    marker = storage.load_setting(MARKER)
    if marker is not None and marker != {"version": VERSION}:
        raise ValueError("Invalid secret archive policy marker; state was preserved")
    if marker is None or setting["on"]:
        path = await private_destination(setting["folder"], account)
        setting = {"folder": str(path), "on": True}
        with storage.transaction():
            storage.save_setting("auto_save_secret_chats", setting)
            storage.save_setting(MARKER, {"version": VERSION})
    return setting


def disable(storage):
    setting = _setting(storage)
    setting["on"] = False
    with storage.transaction():
        storage.save_setting("auto_save_secret_chats", setting)
        storage.save_setting(MARKER, {"version": VERSION})
    return setting
