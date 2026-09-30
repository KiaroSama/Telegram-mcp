"""One chat exported exactly as Telegram Desktop's "Export chat history" writes it (spec 030).

The export is `telegram_mcp.tdexport`, a port of Telegram Desktop v7.2.10's exporter:
a takeout session first (Telegram's own export mechanism) and ordinary reads when
Telegram refuses it, then `messages.html` pages and/or `result.json` with the media
kinds chosen, under the size limit, for the period chosen. The dialog before it -
every choice the owner's - is `telegram_mcp.export_dialog`; the safeguard asks the
owner before the tool runs at all.
"""

from telegram_mcp import export_dialog
from telegram_mcp.runtime import *
from telegram_mcp.tdexport.fetch import export_single_chat
from telegram_mcp.tdexport.settings import Environment

__all__ = ["export_chat_history"]

_TOOL = "export_chat_history"


@mcp.tool(
    annotations=ToolAnnotations(
        title="Export Chat History",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=False,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def export_chat_history(
    chat_id: Union[int, str],
    format: Optional[str] = None,
    photos: Optional[bool] = None,
    videos: Optional[bool] = None,
    voice_messages: Optional[bool] = None,
    video_messages: Optional[bool] = None,
    stickers: Optional[bool] = None,
    gifs: Optional[bool] = None,
    files: Optional[bool] = None,
    size_limit_mb: Optional[float] = None,
    date_from: str = "",
    date_to: str = "",
    destination: str = "",
    ctx: Optional[Context] = None,
    account: str = None,
) -> str:
    """
    Export one chat exactly as Telegram Desktop's "Export chat history" does.

    Same folder (`ChatExport_YYYY-MM-DD`), same `messages.html` pages with Desktop's own
    styles, same `result.json`, same media folders. Uses Telegram's export (takeout)
    session and falls back to ordinary reads if Telegram refuses it.

    The choices belong to the owner. A client with forms shows Desktop's export dialog;
    otherwise ASK THE OWNER each of these and pass them all, never a guess:
        format: "html", "json" or "html_and_json".
        photos, videos, voice_messages, video_messages, stickers, gifs, files: which
            media to download (Desktop's defaults: photos only).
        size_limit_mb: skip files larger than this (1-4000; Desktop's default 8).
        date_from / date_to: "YYYY-MM-DD" or "YYYY-MM-DD HH:MM", local time; empty is
            the beginning / the present.

    Args:
        chat_id: The chat ID or username.
        destination: The folder to export into, relative to files/ or absolute; empty is
            this server's downloads folder, where a `ChatExport_...` folder is made.
    """
    choices = dict(
        format=format,
        photos=photos,
        videos=videos,
        voice_messages=voice_messages,
        video_messages=video_messages,
        stickers=stickers,
        gifs=gifs,
        files=files,
        size_limit_mb=size_limit_mb,
        date_from=date_from,
        date_to=date_to,
        destination=destination,
    )
    try:
        settings, answer = await export_dialog.choose(
            ctx, choices, _TOOL, "Export chat history - choose what to export"
        )
        if settings is None:
            return answer

        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)
        settings.single_peer = await cl.get_input_entity(entity)
        # export_view_panel_controller.cpp: serverConfig().internalLinksDomain.
        config = await cl(functions.help.GetConfigRequest())
        environment = Environment(internal_links_domain=config.me_url_prefix)
        result = await export_single_chat(
            cl, settings, export_dialog.writer_for(settings.format), environment
        )
        return format_tool_result(
            {
                "chat_id": get_marked_id(entity),
                "folder": result.path,
                "format": settings.format.name,
                "messages": result.messages,
                "files": result.files,
                "takeout": result.takeout,
                "takeout_refused": result.takeout_error or None,
            }
        )
    except (asyncio.TimeoutError, TimeoutError):
        return "Export cancelled: the export dialog was not answered in 15 minutes."
    except Exception as e:
        return log_and_format_error(_TOOL, e, chat_id=chat_id, destination=destination)
