"""The order of pinned chats, which `pin_chat` can set but not arrange.

Two pinned lists exist at the account level - All chats (folder 0) and the
Archive (folder 1) - and `messages.reorderPinnedDialogs` arranges one of them.
A custom folder's pinned chats are a different list, part of the folder itself
(pin_chat with `folder`); pinning in one place does not pin in the other, so
this tool never touches a custom folder.
"""

from telegram_mcp.runtime import *

__all__ = ["reorder_pinned_chats"]

_PINNED_LISTS = {0: "All chats", 1: "Archive"}


@mcp.tool(
    annotations=ToolAnnotations(
        title="Reorder Pinned Chats",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("order")
async def reorder_pinned_chats(
    order: List[Union[int, str]],
    folder_id: int = 0,
    force: bool = False,
    account: str = None,
) -> str:
    """
    Arrange the pinned chats of All chats or of the Archive, top first.

    Args:
        order: The pinned chats' ids or @usernames, in the order to show them.
        folder_id: 0 for All chats, 1 for the Archive. A custom folder's pinned
            chats are part of that folder (pin_chat with `folder`), not this list:
            pinning in a folder does not pin in All chats.
        force: True to make the list exactly `order`, unpinning any pinned chat
            left out; False to reorder only the chats named.
    """
    if not order:
        return "Give at least one pinned chat, in the order to show them."
    if folder_id not in _PINNED_LISTS:
        return "folder_id must be 0 (All chats) or 1 (Archive)."
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        peers = [
            types.InputDialogPeer(peer=await resolve_input_entity(chat, cl)) for chat in order
        ]
        await cl(
            functions.messages.ReorderPinnedDialogsRequest(
                folder_id=folder_id, order=peers, force=True if force else None
            )
        )
        records = [{"position": i, "chat": str(chat)} for i, chat in enumerate(order, 1)]
        return format_tool_result(
            records, {"list": _PINNED_LISTS[folder_id], "unpinned_the_rest": bool(force)}
        )
    except Exception as e:
        return log_and_format_error("reorder_pinned_chats", e, folder_id=folder_id)
