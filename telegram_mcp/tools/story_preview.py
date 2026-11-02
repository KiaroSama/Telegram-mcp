"""Images and sampled video frames for visible stories, without seen signals."""

from telegram_mcp.runtime import *
from telegram_mcp.story_media import fetch_story_media
from telegram_mcp.media_transfer import (
    MAX_FRAME_SOURCE_BYTES,
    _download_capped,
    with_reference_retry,
)
from telegram_mcp.media_preview import (
    encode_still_cancellable,
    encode_frames_cancellable,
    _media_suffix,
)
from telegram_mcp.message_view import describe_media
from telegram_mcp.tools.inspection import require_explicit_account

__all__ = ["preview_story"]


@mcp.tool(
    annotations=ToolAnnotations(
        title="Preview Story",
        readOnlyHint=True,
        destructiveHint=False,
        idempotentHint=True,
        openWorldHint=True,
    )
)
@require_explicit_account
@with_account(readonly=True)
@validate_id("chat_id")
async def preview_story(
    chat_id: Union[int, str],
    story_id: int,
    count: int = 4,
    max_bytes: int = 50 * 1024 * 1024,
    max_dimension: int = 900,
    account: str = None,
) -> list:
    """View an accessible story photo or sampled video frames WITHOUT registering a view.

    Use list_peer_stories/get_stories for IDs. count is capped at10 and max_bytes
    at200MiB; transfer stops at the cap. Inaccessible/deleted/live stories refuse.
    To save the original use download_media(chat_id=..., story_id=...).
    No story read/view request, reaction or Ghost Mode change is performed.
    """
    try:
        if max_bytes <= 0:
            return "max_bytes must be positive."
        cap = min(int(max_bytes), MAX_FRAME_SOURCE_BYTES)
        cl = get_client(account)
        peer = await resolve_entity(chat_id, cl)
        message = await fetch_story_media(cl, peer, story_id)
        details = describe_media(message) or {}
        if (details.get("size_bytes") or 0) > cap:
            return "Story exceeds max_bytes; no media was downloaded."

        async def refresh():
            return await fetch_story_media(cl, peer, story_id)

        async def download(fresh):
            return await _download_capped(cl, fresh or message, cap)

        data, over_cap = await with_reference_retry(download, refresh)
        if over_cap:
            return "Story transfer exceeded max_bytes and was aborted."
        if not data:
            return "Telegram returned no story media bytes."
        if message.photo is not None:
            records, images = await encode_still_cancellable(data, max_dimension)
        else:
            records, images = await encode_frames_cancellable(
                data,
                _media_suffix(details),
                count,
                max_dimension,
            )
        return [
            format_tool_result(
                records,
                {
                    "story_id": story_id,
                    "chat_id": str(chat_id),
                    "seen_sent": False,
                    "source_bytes": len(data),
                    "media_kind": details.get("kind"),
                },
            ),
            *images,
        ]
    except ValueError as error:
        return str(error)
    except Exception as error:
        return log_and_format_error("preview_story", error, chat_id=chat_id, story_id=story_id)
