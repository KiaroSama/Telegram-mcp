"""How a channel is found: its usernames, and which groups can be its discussion group.

Phase 1 of `docs/api-coverage.md`, identity group. The writes reuse
`channel_settings._toggle` (resolve, refuse a basic group in words, one request,
a sentence back), so a channel-only request never reaches Telegram for a basic
group and an admin-rights refusal reads the same everywhere.

Every write here is reversible: a username switched off can be switched back on,
and `deactivate_channel_usernames` only deactivates - each one returns with
`toggle_channel_username`.
"""

from telegram_mcp.runtime import *
from telegram_mcp.tools.channel_settings import _toggle

__all__ = [
    "deactivate_channel_usernames",
    "list_discussion_candidates",
    "reorder_channel_usernames",
    "toggle_channel_username",
]


def _bare(username) -> str:
    return str(username or "").strip().lstrip("@").strip()


@mcp.tool(
    annotations=ToolAnnotations(
        title="Toggle Channel Username",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def toggle_channel_username(
    chat_id: Union[int, str], username: str, active: bool, account: str = None
) -> str:
    """
    Switch one of a channel's collectible usernames on or off.

    Args:
        chat_id: The channel or supergroup ID or username.
        username: One of the channel's own usernames (with or without @).
        active: True to make it resolve to the channel, False to park it.

    A username that is not the channel's own is refused by Telegram. Reversible.
    """
    name = _bare(username)
    if not name:
        return "Give the username to switch, for example @my_channel."
    return await _toggle(
        "toggle_channel_username",
        chat_id,
        account,
        lambda entity: functions.channels.ToggleUsernameRequest(
            channel=entity, username=name, active=active
        ),
        lambda title: f"@{name} is now {'active' if active else 'inactive'} on {title}.",
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Reorder Channel Usernames",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def reorder_channel_usernames(
    chat_id: Union[int, str], order: List[str], account: str = None
) -> str:
    """
    Set the order a channel's active usernames are shown in.

    Args:
        chat_id: The channel or supergroup ID or username.
        order: The active usernames, first shown first (with or without @).
    """
    names = [_bare(item) for item in (order or [])]
    if not names or not all(names):
        return "Give at least one username in order, and no empty entries."
    return await _toggle(
        "reorder_channel_usernames",
        chat_id,
        account,
        lambda entity: functions.channels.ReorderUsernamesRequest(channel=entity, order=names),
        lambda title: f"Usernames of {title} are now ordered: "
        + ", ".join(f"@{n}" for n in names)
        + ".",
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Deactivate Channel Usernames",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def deactivate_channel_usernames(chat_id: Union[int, str], account: str = None) -> str:
    """
    Deactivate every collectible username of a channel at once.

    Args:
        chat_id: The channel or supergroup ID or username.

    The channel stops being reachable by those names until each is switched back
    on with toggle_channel_username. Nothing is released or sold.
    """
    return await _toggle(
        "deactivate_channel_usernames",
        chat_id,
        account,
        lambda entity: functions.channels.DeactivateAllUsernamesRequest(channel=entity),
        lambda title: f"Every collectible username of {title} is now inactive.",
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="List Discussion Candidates",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=True,
    )
)
@with_account(readonly=True)
async def list_discussion_candidates(account: str = None) -> str:
    """
    List the groups this account could link to a channel as its discussion group.

    Pass one of the returned ids to set_discussion_group. Telegram decides the
    list: groups this account administers that are not already linked elsewhere.
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        result = await cl(functions.channels.GetGroupsForDiscussionRequest())
        records = [
            {
                "id": get_marked_id(chat),
                "title": sanitize_name(getattr(chat, "title", "")),
                "type": get_entity_type(chat),
                "username": (
                    sanitize_name(chat.username) if getattr(chat, "username", None) else None
                ),
            }
            for chat in getattr(result, "chats", [])
        ]
        return format_tool_result(records, {"count": len(records)})
    except Exception as e:
        return log_and_format_error("list_discussion_candidates", e)
