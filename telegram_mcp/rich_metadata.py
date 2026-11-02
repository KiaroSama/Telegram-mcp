"""Bounded rich-post destinations and media pointers (upstream c7f200b adaptation)."""

from collections import deque
from telethon import types
from telegram_mcp import rich_blocks
from telegram_mcp.text_fidelity import display_name


def describe(rich):
    links, media = [], []
    catalogue = rich_blocks.file_catalogue(rich)
    pending = deque((getattr(rich, "blocks", None) or [])[:4096])
    visited = 0
    while pending and visited < 4096:
        node = pending.popleft()
        visited += 1
        target = getattr(node, "url", None)
        if isinstance(node, types.TextEmail):
            target = "mailto:" + node.email
        elif isinstance(node, types.TextPhone):
            target = "tel:" + node.phone
        if target:
            cleaned = display_name(target, max_length=2048)
            if cleaned and cleaned not in links:
                links.append(cleaned)
        identifier = rich_blocks.named_file(node)
        kind = {
            "PageBlockPhoto": "photo",
            "PageBlockVideo": "video",
            "PageBlockAudio": "audio",
        }.get(type(node).__name__)
        if identifier and kind:
            record = {
                "kind": kind,
                "file_id": str(identifier),
                "available": identifier in catalogue,
            }
            if record not in media:
                media.append(record)
        if hasattr(node, "to_dict"):
            for name, value in vars(node).items():
                if name.startswith("_"):
                    continue
                capacity = 4096 - visited - len(pending)
                if capacity <= 0:
                    break
                if isinstance(value, (list, tuple)):
                    pending.extend(value[:capacity])
                elif hasattr(value, "to_dict"):
                    pending.append(value)
    return links, media
