"""Managing forum topics past create/edit, and making a basic group a supergroup (spec 020).

``topics`` lists, creates and edits topics; this module deletes, pins and orders them,
turns a forum back into a plain supergroup, and upgrades a basic group - the step a
basic group needs before it can have topics at all (https://core.telegram.org/api/forum).

Deleting a topic deletes every message in it and cannot be undone; General (id 1) cannot
be deleted at all. An upgrade cannot be undone either and gives the group a new id.
"""

from typing import List, Union

from telethon.errors import RPCError

from telegram_mcp.runtime import *

__all__ = [
    "delete_forum_topic",
    "disable_forum_topics",
    "pin_forum_topic",
    "reorder_pinned_topics",
    "upgrade_to_supergroup",
]

GENERAL_TOPIC = 1
# deleteTopicHistory answers with an offset while messages remain; each round deletes a
# batch. Bounded so a topic that never reports "done" cannot hold the call forever.
_DELETE_ROUNDS = 50


def _refused(tool: str, error: RPCError, chat_id) -> str:
    if type(error).__name__.startswith("FloodWait"):
        return log_and_format_error(tool, error, chat_id=chat_id)
    return f"Telegram refused: {error.message}. Nothing was changed."


async def _forum(chat_id, account):
    """``(client, entity, problem)``: the forum supergroup, or why it is not one."""
    cl = get_client(account)
    await ensure_connected(cl)
    entity = await resolve_entity(chat_id, cl)
    if not isinstance(entity, Channel) or not getattr(entity, "megagroup", False):
        return cl, entity, "The specified chat is not a supergroup."
    if not getattr(entity, "forum", False):
        return cl, entity, "The specified supergroup does not have forum topics enabled."
    return cl, entity, None


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Forum Topic",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=True,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def delete_forum_topic(chat_id: Union[int, str], topic_id: int, account: str = None) -> str:
    """
    Delete a forum topic and EVERY message in it, for everyone. Cannot be undone.
    General (topic 1) cannot be deleted. Always asks the owner first.

    Args:
        chat_id: The forum supergroup.
        topic_id: The topic, from list_topics.
    """
    try:
        if int(topic_id) == GENERAL_TOPIC:
            return "The General topic (id 1) cannot be deleted. Nothing was changed."
        cl, entity, problem = await _forum(chat_id, account)
        if problem:
            return problem
        rounds = 0
        while rounds < _DELETE_ROUNDS:
            rounds += 1
            result = await cl(
                functions.messages.DeleteTopicHistoryRequest(peer=entity, top_msg_id=int(topic_id))
            )
            if not getattr(result, "offset", 0):
                break
        done = rounds < _DELETE_ROUNDS
        return format_tool_result(
            [{"topic_id": int(topic_id), "deleted": done, "rounds": rounds}],
            (
                {"chat_id": str(chat_id)}
                if done
                else {
                    "chat_id": str(chat_id),
                    "note": "Telegram still reports messages; call again.",
                }
            ),
        )
    except RPCError as e:
        return _refused("delete_forum_topic", e, chat_id)
    except Exception as e:
        return log_and_format_error("delete_forum_topic", e, chat_id=chat_id, topic_id=topic_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Pin Forum Topic",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def pin_forum_topic(
    chat_id: Union[int, str], topic_id: int, pinned: bool = True, account: str = None
) -> str:
    """
    Pin a forum topic to the top of the topic list, or unpin it (pinned=False).
    Telegram caps how many topics one forum may pin.

    Args:
        chat_id: The forum supergroup.
        topic_id: The topic, from list_topics.
        pinned: True pins, False unpins.
    """
    try:
        cl, entity, problem = await _forum(chat_id, account)
        if problem:
            return problem
        await cl(
            functions.messages.UpdatePinnedForumTopicRequest(
                peer=entity, topic_id=int(topic_id), pinned=bool(pinned)
            )
        )
        return format_tool_result(
            [{"topic_id": int(topic_id), "pinned": bool(pinned)}], {"chat_id": str(chat_id)}
        )
    except RPCError as e:
        return _refused("pin_forum_topic", e, chat_id)
    except Exception as e:
        return log_and_format_error("pin_forum_topic", e, chat_id=chat_id, topic_id=topic_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Reorder Pinned Topics",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def reorder_pinned_topics(
    chat_id: Union[int, str], topic_ids: List[int], force: bool = False, account: str = None
) -> str:
    """
    Set the order of the pinned topics, first to last.

    Args:
        chat_id: The forum supergroup.
        topic_ids: Every pinned topic id in the wanted order.
        force: Also pin listed topics that are not pinned yet, unpinning the rest.
    """
    try:
        order = [int(t) for t in topic_ids or []]
        if not order:
            return "Give at least one topic id. Nothing was changed."
        cl, entity, problem = await _forum(chat_id, account)
        if problem:
            return problem
        await cl(
            functions.messages.ReorderPinnedForumTopicsRequest(
                peer=entity, order=order, force=bool(force) or None
            )
        )
        return format_tool_result([{"pinned_order": order}], {"chat_id": str(chat_id)})
    except RPCError as e:
        return _refused("reorder_pinned_topics", e, chat_id)
    except Exception as e:
        return log_and_format_error("reorder_pinned_topics", e, chat_id=chat_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Disable Forum Topics",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def disable_forum_topics(chat_id: Union[int, str], account: str = None) -> str:
    """
    Turn forum topics off: the group becomes a plain supergroup again. Needs the owner
    (creator) of the group. Turn them back on with enable_forum_topics.

    Args:
        chat_id: The forum supergroup.
    """
    try:
        cl, entity, problem = await _forum(chat_id, account)
        if problem:
            if isinstance(entity, Channel) and getattr(entity, "megagroup", False):
                return "Forum topics are already off for this supergroup."
            return problem
        await cl(functions.channels.ToggleForumRequest(channel=entity, enabled=False, tabs=False))
        return format_tool_result([{"topics_enabled": False}], {"chat_id": str(chat_id)})
    except RPCError as e:
        return _refused("disable_forum_topics", e, chat_id)
    except Exception as e:
        return log_and_format_error("disable_forum_topics", e, chat_id=chat_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Upgrade To Supergroup",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=True,
        idempotentHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def upgrade_to_supergroup(
    chat_id: Union[int, str], enable_topics: bool = False, account: str = None
) -> str:
    """
    Turn a basic group into a supergroup. Cannot be undone, and the group gets a NEW id
    (returned here; the old id stops working). Always asks the owner first.

    Args:
        chat_id: The basic group.
        enable_topics: Also turn forum topics on right after (needs the group's owner).
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)
        if isinstance(entity, Channel):
            kind = "channel" if getattr(entity, "broadcast", False) else "supergroup"
            return f"This chat is already a {kind}; only a basic group can be upgraded."
        if not isinstance(entity, Chat):
            return "Only a basic group can be upgraded to a supergroup."
        updates = await cl(functions.messages.MigrateChatRequest(chat_id=entity.id))
        new = next(
            (c for c in getattr(updates, "chats", []) or [] if isinstance(c, Channel)), None
        )
        if new is None:
            return "Telegram upgraded the group but did not return the new supergroup."
        row = {
            "old_chat_id": get_marked_id(entity),
            "new_chat_id": get_marked_id(new),
            "topics_enabled": False,
        }
        if enable_topics:
            try:
                await cl(
                    functions.channels.ToggleForumRequest(channel=new, enabled=True, tabs=True)
                )
                row["topics_enabled"] = True
            except RPCError as e:
                row["topics_error"] = f"Telegram refused enabling topics: {e.message}"
        return format_tool_result([row], {"note": "Use new_chat_id from now on."})
    except RPCError as e:
        return _refused("upgrade_to_supergroup", e, chat_id)
    except Exception as e:
        return log_and_format_error("upgrade_to_supergroup", e, chat_id=chat_id)
