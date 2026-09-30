"""The safeguard names the chat an action happens in (spec 032).

The "Chat" line of an approval, and the chat an "always approve" is stored for, come from
`middleware._target`. It used to know only `to_chat_id`, `chat_id`, `user_id`, `username`:
`promote_admin(group_id=..., user_id=...)` was shown - and granted - as the user, and tools
naming their chat `chat`, `community`, `channel_id`, ... had no chat at all, so an "always"
for them was bound to none.
"""

import pytest

from telegram_mcp.safeguard import policy
from telegram_mcp.safeguard.middleware import _target
from telegram_mcp.tools import mcp

#: Argument names that name the chat an action happens in.
CHAT_ARGUMENTS = (
    "to_chat_id",
    "chat_id",
    "chat",
    "channel_id",
    "group_id",
    "channel",
    "community",
    "secret_chat_id",
)
GUARDED = policy.GATED | policy.FREE_WRITES | policy.SEND


def test_promote_admin_is_shown_in_its_group_not_as_the_new_admin():
    arguments = {"group_id": "@NumerGroup", "user_id": "@NumeraGroup10Bot"}

    assert _target(arguments) == "@NumerGroup"


def _cases():
    for tool in mcp._tool_manager.list_tools():
        if tool.name not in GUARDED:
            continue
        properties = (tool.parameters or {}).get("properties", {})
        chat = next((name for name in CHAT_ARGUMENTS if name in properties), None)
        if chat:
            yield pytest.param(tool.name, chat, sorted(properties), id=tool.name)


@pytest.mark.parametrize("tool, chat, parameters", list(_cases()))
def test_every_guarded_tool_is_approved_for_its_own_chat(tool, chat, parameters):
    arguments = {name: f"value-of-{name}" for name in parameters}

    assert _target(arguments) == f"value-of-{chat}"


def test_a_secret_chat_is_shown_as_one_not_looked_up_as_a_peer():
    from telegram_mcp.safeguard.middleware import _shown_target

    arguments = {"secret_chat_id": 627857491}

    assert _shown_target(arguments, _target(arguments)) == "secret chat 627857491"
