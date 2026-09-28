"""How many people saw a channel post, read without being one of them.

`messages.getMessagesViews` is documented as "Get and increase the view counter".
The `increment` flag chooses: True counts this account as a viewer, which tells
the channel someone looked - a seen signal under ghost mode that no approval
covers. So this module always sends `increment=False`, and the tool has no
argument that could change it. `tests/test_message_views.py` reads this source
to hold that line.
"""

from telegram_mcp.runtime import *

__all__ = ["get_message_views"]


@mcp.tool(
    annotations=ToolAnnotations(
        title="Get Message Views",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=True,
    )
)
@with_account(readonly=True)
@validate_id("chat_id")
async def get_message_views(
    chat_id: Union[int, str], message_ids: List[int], account: str = None
) -> str:
    """
    Read the view, forward and reply counters of channel posts.

    Args:
        chat_id: The channel (or the chat a channel post was forwarded into).
        message_ids: The post ids to read.

    Never counts a view: reading here does not add this account to the counter.
    A counter Telegram does not report comes back as null.
    """
    if not message_ids:
        return "Give at least one message id."
    try:
        ids = [int(i) for i in message_ids]
        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)
        result = await cl(
            functions.messages.GetMessagesViewsRequest(peer=entity, id=ids, increment=False)
        )
        records = []
        for message_id, counted in zip(ids, getattr(result, "views", [])):
            replies = getattr(counted, "replies", None)
            records.append(
                {
                    "message_id": message_id,
                    "views": getattr(counted, "views", None),
                    "forwards": getattr(counted, "forwards", None),
                    "replies": getattr(replies, "replies", None),
                }
            )
        return format_tool_result(records, {"chat_id": str(chat_id), "counted_a_view": False})
    except Exception as e:
        return log_and_format_error("get_message_views", e, chat_id=chat_id)
