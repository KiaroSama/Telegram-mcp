"""Default forum icons, not arbitrary premium emoji (upstream69f360e, Apache-2.0)."""

from telegram_mcp.runtime import *

__all__ = ["list_forum_topic_icons"]


@mcp.tool(
    annotations=ToolAnnotations(
        title="List Forum Topic Icons",
        readOnlyHint=True,
        destructiveHint=False,
        idempotentHint=True,
        openWorldHint=True,
    )
)
@with_account(readonly=True)
async def list_forum_topic_icons(account: str = None) -> str:
    """List Telegram's default topic icons for create_forum_topic/edit_forum_topic.

    icon_emoji_id names the custom emoji document; emoji is only the fallback glyph,
    not a visual preview. This does not change any topic or install a sticker set.
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        result = await cl(
            functions.messages.GetStickerSetRequest(
                stickerset=types.InputStickerSetEmojiDefaultTopicIcons(), hash=0
            )
        )
        records = []
        for document in getattr(result, "documents", []) or []:
            alt = next(
                (
                    getattr(a, "alt", "")
                    for a in document.attributes
                    if isinstance(a, types.DocumentAttributeCustomEmoji)
                ),
                "",
            )
            records.append(
                {"icon_emoji_id": str(document.id), "emoji": sanitize_user_content(alt)}
            )
        return format_tool_result(records)
    except Exception as error:
        return log_and_format_error("list_forum_topic_icons", error)
