"""Clearing a chat's history, and deleting a chat from the owner's list.

Clear history removes the messages and keeps the chat; Delete chat removes the chat,
and for a group or channel that means leaving it - the group itself is never deleted.
"Both sides" (also removing the other person's copy) is never assumed: a call without
it is refused, so the agent has to ask the owner. It exists only for private chats;
anywhere else it is refused rather than silently dropped.
"""

from telegram_mcp.runtime import *

_ASK = (
    "both_sides is required and has no default: ask the owner whether the other side's "
    "copy should go too, then call again with both_sides=true or both_sides=false."
)
# Each pass of messages.deleteHistory removes a batch; a positive offset means "repeat".
_MAX_PASSES = 50


def _both_sides_refusal(entity, both_sides):
    if both_sides is None:
        return _ASK
    private = isinstance(entity, User) and not entity.is_self
    if both_sides and not private:
        return (
            "Both sides exists only for a private chat or bot; this chat has no other "
            "side's copy to remove. Call again with both_sides=false."
        )
    return None


async def _delete_history(cl, entity, *, just_clear: bool, both_sides: bool) -> bool:
    """Repeat messages.deleteHistory until Telegram reports nothing left; True when done."""
    peer = utils.get_input_peer(entity)
    for _ in range(_MAX_PASSES):
        result = await cl(
            functions.messages.DeleteHistoryRequest(
                peer=peer, max_id=0, just_clear=just_clear or None, revoke=both_sides or None
            )
        )
        if not (getattr(result, "offset", 0) or 0) > 0:
            return True
    return False


@mcp.tool(
    annotations=ToolAnnotations(
        title="Clear Chat History",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=True,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat")
async def clear_chat_history(
    chat: Union[int, str], both_sides: Optional[bool] = None, account: str = None
) -> str:
    """
    Remove a chat's messages; the chat stays in the list.

    Args:
        chat: The chat's id or @username (search_my_chats finds it by name).
        both_sides: REQUIRED, no default - ask the owner every time. True also removes
            the other person's copy (private chats and bots only).
    """
    try:
        cl = get_client(account)
        entity = await resolve_entity(chat, cl)
        refusal = _both_sides_refusal(entity, both_sides)
        if refusal:
            return refusal
        if isinstance(entity, Channel):
            if not entity.megagroup:
                return "Telegram offers no history clearing for a channel; only its admins delete posts."
            await cl(functions.channels.DeleteHistoryRequest(channel=entity, max_id=0))
            return "History cleared for you; the group stays in your list."
        if not await _delete_history(cl, entity, just_clear=True, both_sides=bool(both_sides)):
            return "Telegram kept reporting messages left after many passes; the history is only partly cleared."
        side = " for both sides" if both_sides else " for you"
        return f"History cleared{side}; the chat stays in your list."
    except Exception as e:
        return log_and_format_error("clear_chat_history", e, chat=chat)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Chat",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=True,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat")
async def delete_chat(
    chat: Union[int, str], both_sides: Optional[bool] = None, account: str = None
) -> str:
    """
    Remove a chat from the list. For a group or channel this leaves it; the group or
    channel itself goes on existing.

    Args:
        chat: The chat's id or @username (search_my_chats finds it by name).
        both_sides: REQUIRED, no default - ask the owner every time. True also deletes
            the other person's copy (private chats and bots only).
    """
    try:
        cl = get_client(account)
        entity = await resolve_entity(chat, cl)
        refusal = _both_sides_refusal(entity, both_sides)
        if refusal:
            return refusal
        if isinstance(entity, Channel):
            await cl(functions.channels.LeaveChannelRequest(channel=entity))
            return f"Left {sanitize_name(entity.title)}; it is gone from your list."
        if isinstance(entity, Chat):
            await cl(
                functions.messages.DeleteChatUserRequest(
                    chat_id=entity.id, user_id=types.InputUserSelf()
                )
            )
            await _delete_history(cl, entity, just_clear=False, both_sides=False)
            return f"Left {sanitize_name(entity.title)}; it is gone from your list."
        if not await _delete_history(cl, entity, just_clear=False, both_sides=bool(both_sides)):
            return "Telegram kept reporting messages left after many passes; the chat is only partly deleted."
        side = " for both sides" if both_sides else " for you"
        return f"Chat deleted{side}; it is gone from your list."
    except Exception as e:
        return log_and_format_error("delete_chat", e, chat=chat)


__all__ = ["clear_chat_history", "delete_chat"]
