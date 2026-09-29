"""Every photo tool tells the agent which one does what, and to ask first (spec 026).

Agents asked to "change the profile photo" kept reaching for `edit_chat_photo`, which
keeps every earlier photo - and `set_profile_photo`'s own description pointed them
there. The descriptions are the only guide an agent reads, so they are pinned here.
"""

import ast
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent / "telegram_mcp" / "tools"
TOOLS = {
    "edit_chat_photo": "groups.py",
    "replace_chat_photo": "chat_photo_history.py",
    "set_profile_photo": "profile.py",
    "set_community_photo": "communities.py",
}


def _description(tool: str) -> str:
    tree = ast.parse((ROOT / TOOLS[tool]).read_text(encoding="utf-8"))
    node = next(
        n for n in ast.walk(tree) if isinstance(n, ast.AsyncFunctionDef) and n.name == tool
    )
    return ast.get_docstring(node)


@pytest.mark.parametrize("tool", sorted(TOOLS))
def test_every_photo_tool_says_to_ask_whose_photo_and_whether_to_keep_the_old(tool):
    text = _description(tool)
    assert "ask them" in text
    for other in TOOLS:
        assert f"`{other}`" in text or other == tool


def test_edit_chat_photo_says_the_old_photos_stay():
    text = _description("edit_chat_photo")
    assert "KEEP every earlier one" in text and "`replace_chat_photo` instead" in text


def test_set_profile_photo_no_longer_sends_groups_to_edit_chat_photo():
    assert "take their photo through `edit_chat_photo`" not in _description("set_profile_photo")
