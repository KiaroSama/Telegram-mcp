"""A group's or channel's Recent actions exported like a Telegram Desktop chat export (spec 033 R10).

Desktop itself has no admin log export; the owner chose (2026-10-01) its chat export's format:
the same dialog (`export_dialog`), the same folder, pages and `result.json`, the same
background job (`export_status`, `cancel_export`), plus Recent actions' own filter
(`get_recent_actions`: event types, admins, search text). The rendering is
`telegram_mcp.admin_log_export`; the safeguard asks the owner before the tool runs.
"""

from telegram_mcp import admin_log_export, export_dialog, export_jobs
from telegram_mcp.runtime import *
from telegram_mcp.tdexport.settings import Environment
from telegram_mcp.tools.moderation import admin_log_filter, unknown_event_types

__all__ = ["export_recent_actions"]

_TOOL = "export_recent_actions"


@mcp.tool(
    annotations=ToolAnnotations(
        title="Export Recent Actions",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=False,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def export_recent_actions(
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
    event_types: Optional[List[str]] = None,
    admins: Optional[List[Union[int, str]]] = None,
    query: str = "",
    ctx: Optional[Context] = None,
    account: str = None,
) -> str:
    """
    Export a supergroup's or channel's Recent actions (admin log) as Telegram Desktop's
    chat export writes a chat: `messages.html` pages and/or `result.json`.

    Every event reads as Desktop's Recent actions shows it - "Sara deleted message:" and
    then the deleted message itself, with its text and media; an edit with its original;
    bans and admin rights with Desktop's list of rights. Oldest first. Telegram keeps
    about 48 hours of it.

    Runs in the background and answers at once with a `job_id`; `export_status` shows its
    progress and result, `cancel_export` stops it.

    The export choices belong to the owner. A client with forms shows Desktop's export
    dialog; otherwise ASK THE OWNER each of these and pass them all, never a guess:
        format: "html", "json" or "html_and_json".
        photos, videos, voice_messages, video_messages, stickers, gifs, files: which
            media of deleted and edited messages to download (Desktop's default: photos).
        size_limit_mb: skip files larger than this (1-4000; Desktop's default 8).
        date_from / date_to: "YYYY-MM-DD" or "YYYY-MM-DD HH:MM", local time; empty is
            the oldest kept event / the present.

    Args:
        chat_id: The supergroup or channel.
        destination: The folder to export into, relative to files/ or absolute; empty is
            this server's downloads folder, where a `ChatExport_...` folder is made.
        event_types: Recent actions' filter checkboxes, as `get_recent_actions` takes
            them; none = all actions.
        admins: Only actions by these users (ids or usernames); none = everyone.
        query: Desktop's search text.
    """
    refusal = unknown_event_types(event_types)
    if refusal:
        return refusal
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
            ctx, choices, _TOOL, "Export recent actions - choose what to export"
        )
        if settings is None:
            return answer

        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)
        if not isinstance(entity, types.Channel):
            return "Error: Recent actions exist only in supergroups and channels."
        events_filter, refusal = admin_log_filter(event_types, bool(entity.broadcast))
        if refusal:
            return refusal
        admin_users = [utils.get_input_user(await resolve_entity(a, cl)) for a in admins or []]
        settings.single_peer = await cl.get_input_entity(entity)
        config = await cl(functions.help.GetConfigRequest())
        environment = Environment(internal_links_domain=config.me_url_prefix)
        export_dialog.pin_folder(settings)
        writer = admin_log_export.writer_for(settings.format)
        chat_key = get_marked_id(entity)

        async def work() -> dict:
            result = await admin_log_export.export_admin_log(
                cl,
                settings,
                writer,
                environment,
                events_filter=events_filter,
                admins=admin_users,
                query=query,
            )
            return {
                "chat_id": chat_key,
                "folder": result.path,
                "format": settings.format.name,
                "events": result.events,
                "messages": result.messages,
                "files": result.files,
            }

        job = export_jobs.start("recent_actions", chat_key, settings.path, work)
        return format_tool_result(export_jobs.describe(job))
    except (asyncio.TimeoutError, TimeoutError):
        return "Export cancelled: the export dialog was not answered in 15 minutes."
    except Exception as e:
        return log_and_format_error(_TOOL, e, chat_id=chat_id, destination=destination)
