"""Secret chats: auto-save, and deleting a chat or its saved copy (spec 031).

The package (`telethon_secret_chat` 98c366e) does the saving and the deleting. What these
tools add is the part only this server knows: the folder must sit inside the allowed
roots and belong to the owner alone (the saved files are PLAINTEXT), and a deleted chat's
copies this server kept - its message log and file keys - go with it.

Every tool here waits for the owner's approval in the safeguard (owner, 2026-09-30).
"""

from typing import Optional

from telegram_mcp import export_dialog, export_jobs, file_roots
from telegram_mcp.owner_only import restrict_to_owner_strict
from telegram_mcp.secret_backend import erase_local, secret_manager
from telegram_mcp.secret_common import account_label, describe_refusal, to_secret_id
from telegram_mcp.tdexport.secret_saved import export_saved_chat
from telegram_mcp.tdexport.settings import Environment
from telegram_mcp.runtime import *

__all__ = [
    "delete_saved_secret_messages",
    "delete_secret_chat",
    "delete_secret_chat_both_sides",
    "export_secret_chat",
    "start_secret_auto_save",
    "stop_secret_auto_save",
]


def _account_label(account: Optional[str]) -> str:
    return account_label(account)


def _unknown(secret_chat_id: int) -> str:
    return f"No secret chat {secret_chat_id} for this login. `list_secret_chats` shows them."


def _refused(tool: str, error: Exception, **context) -> str:
    return describe_refusal(error) or log_and_format_error(tool, error, **context)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Start Secret Auto-Save",
        openWorldHint=False,
        destructiveHint=False,
        readOnlyHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
async def start_secret_auto_save(folder: str, account: str = None, ctx: Context = None) -> str:
    """
    Save every message and file of every secret chat of this login, from now on.

    Self-destructing messages are kept too, and files are decrypted as they arrive,
    whatever their size. It stays on across restarts until `stop_secret_auto_save`.
    The folder holds PLAINTEXT, so it is made readable by the owner only.

    Args:
        folder: Inside the allowed folders, e.g. `downloads/secret-chats`.
    """
    try:
        path, error = await file_roots.resolve_allowed_folder(
            folder, ctx, "start_secret_auto_save"
        )
        if error:
            return error
        path.mkdir(parents=True, exist_ok=True)
        owner_only = restrict_to_owner_strict(path)
        manager = await secret_manager(_account_label(account))
        await manager.start_auto_save_secret_chats(path)
        return format_tool_result(
            {"auto_save": True, "folder": str(path), "owner_only": owner_only}
        )
    except Exception as e:
        return _refused("start_secret_auto_save", e, folder=folder)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Stop Secret Auto-Save",
        openWorldHint=False,
        destructiveHint=False,
        readOnlyHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
async def stop_secret_auto_save(account: str = None) -> str:
    """
    Save nothing more from secret chats. What was saved stays on disk;
    `delete_saved_secret_messages` removes one chat's copy.
    """
    try:
        manager = await secret_manager(_account_label(account))
        await manager.stop_auto_save_secret_chats()
        return format_tool_result(
            {
                "auto_save": False,
                "note": "Saved messages stay; delete_saved_secret_messages removes them.",
            }
        )
    except Exception as e:
        return _refused("stop_secret_auto_save", e)


async def _delete(tool: str, secret_chat_id: int, account: Optional[str], both: bool) -> str:
    try:
        label = _account_label(account)
        manager = await secret_manager(label)
        secret_id = to_secret_id(secret_chat_id)
        if both:
            reached = await manager.delete_secret_chat_both_sides(secret_id)
        else:
            await manager.delete_secret_chat(secret_id)
        cleanup = erase_local(label, secret_id)
        cleared = cleanup["history"] and cleanup["media"]
        result = {"deleted": cleared, "secret_chat_id": secret_id}
        if not cleared:
            result["local_cleanup"] = cleanup
            result["local_cleanup_note"] = "The chat was removed, but local cleanup is incomplete."
        if both:
            result["delete_request_accepted"] = reached
            result["peer_history_deleted"] = None if reached else False
            result["note"] = (
                "Telegram accepted the deletion request; erasure on the peer is not confirmed."
                if reached
                else "Telegram could not accept the peer-deletion request; local cleanup was attempted."
            )
        return format_tool_result(result)
    except KeyError:
        return _unknown(secret_chat_id)
    except Exception as e:
        return _refused(tool, e, secret_chat_id=secret_chat_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Secret Chat",
        openWorldHint=True,
        destructiveHint=True,
        readOnlyHint=False,
        idempotentHint=False,
    )
)
@with_account(readonly=False)
async def delete_secret_chat(secret_chat_id: int, account: str = None) -> str:
    """
    Delete a secret chat on THIS side only.

    An open chat ends (the other side sees it end and keeps its history), then its
    record and history here are removed. Saved messages (auto-save) are not touched.
    Export first if the messages matter.

    Args:
        secret_chat_id: From `list_secret_chats`; the `chat_id` is accepted too.
    """
    return await _delete("delete_secret_chat", secret_chat_id, account, both=False)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Secret Chat For Both Sides",
        openWorldHint=True,
        destructiveHint=True,
        readOnlyHint=False,
        idempotentHint=False,
    )
)
@with_account(readonly=False)
async def delete_secret_chat_both_sides(secret_chat_id: int, account: str = None) -> str:
    """
    Delete a secret chat here AND ask Telegram to erase the other side's history.

    Best effort, like the official apps: a chat already ended on the server can no
    longer carry the request. `delete_request_accepted` reports Telegram's acceptance;
    `peer_history_deleted` is null when accepted (the peer's erasure is not confirmed),
    false otherwise. Local cleanup is attempted either way; `deleted` is true only when
    both local stores were cleared. Partial cleanup includes `local_cleanup` details.
    Auto-saved messages are not touched.

    Args:
        secret_chat_id: From `list_secret_chats`; the `chat_id` is accepted too.
    """
    return await _delete("delete_secret_chat_both_sides", secret_chat_id, account, both=True)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Saved Secret Messages",
        openWorldHint=False,
        destructiveHint=True,
        readOnlyHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
async def delete_saved_secret_messages(secret_chat_id: int, account: str = None) -> str:
    """
    Remove one secret chat's auto-saved messages and files from disk. Works for a
    deleted chat too. Nothing is sent.

    Args:
        secret_chat_id: From `list_secret_chats`, or the id a deleted chat had.
    """
    try:
        manager = await secret_manager(_account_label(account))
        secret_id = to_secret_id(secret_chat_id)
        manager.delete_saved_messages(secret_id)
        return format_tool_result({"deleted": True, "secret_chat_id": secret_id})
    except Exception as e:
        return _refused("delete_saved_secret_messages", e, secret_chat_id=secret_chat_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Export Secret Chat",
        openWorldHint=True,
        destructiveHint=False,
        readOnlyHint=False,
        idempotentHint=False,
    )
)
@with_account(readonly=False)
async def export_secret_chat(
    secret_chat_id: int,
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
    peer_user: Optional[Union[int, str]] = None,
    ctx: Context = None,
    account: str = None,
) -> str:
    """
    Export a secret chat's AUTO-SAVED messages exactly as Telegram Desktop exports a chat.

    Runs in the background and answers at once with a `job_id`; `export_status` shows
    its progress and result, `cancel_export` stops it.

    Only what `start_secret_auto_save` kept can be exported: a secret chat has no
    server copy. Same options, and the same rule, as `export_chat_history`: a client
    with forms shows Desktop's export dialog; otherwise ASK THE OWNER format, photos,
    videos, voice_messages, video_messages, stickers, gifs, files, size_limit_mb and the
    period (date_from/date_to), and pass them all.

    Args:
        secret_chat_id: From `list_secret_chats`; the `chat_id` is accepted too, and a
            deleted chat's id works while its saved copy exists.
        destination: The folder to export into; empty is this server's downloads folder.
        peer_user: The other person, needed only for a DELETED chat (id or username).
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
        manager = await secret_manager(_account_label(account))
        secret_id = to_secret_id(secret_chat_id)
        records = manager.read_saved_messages(secret_id)
        if not records:
            return (
                f"Nothing was auto-saved for secret chat {secret_chat_id}. Only what "
                "`start_secret_auto_save` kept can be exported."
            )
        try:
            peer_id: Union[int, str] = manager.status(secret_id).peer_user_id
        except KeyError:
            if peer_user is None:
                return (
                    f"Secret chat {secret_chat_id} is deleted, so who it was with is not "
                    "known here. Pass peer_user (their id or username)."
                )
            peer_id = peer_user
        settings, answer = await export_dialog.choose(
            ctx, choices, "export_secret_chat", "Export secret chat - choose what to export"
        )
        if settings is None:
            return answer
        cl = get_client(account)
        await ensure_connected(cl)
        me, peer = await cl.get_me(), await cl.get_entity(peer_id)
        config = await cl(functions.help.GetConfigRequest())
        environment = Environment(internal_links_domain=config.me_url_prefix)
        writer = export_dialog.writer_for(settings.format)
        # A secret chat exports as the 1:1 chat with its peer (export_saved_chat sets the
        # same peer); set here so the folder is named ChatExport_ before the job starts.
        settings.single_peer = types.InputPeerUser(peer.id, getattr(peer, "access_hash", 0) or 0)
        export_dialog.pin_folder(settings)

        async def work() -> dict:
            # Copying saved files is blocking disk work; keep the event loop free.
            # ponytail: a cancel stops waiting, not the copy thread; fine for local copies.
            result = await asyncio.to_thread(
                export_saved_chat, records, settings, writer, me, peer, environment
            )
            return {
                "secret_chat_id": secret_id,
                "folder": result.path,
                "format": settings.format.name,
                "messages": result.messages,
                "files": result.files,
            }

        job = export_jobs.start("secret chat", secret_id, settings.path, work)
        return format_tool_result(export_jobs.describe(job))
    except (asyncio.TimeoutError, TimeoutError):
        return "Export cancelled: the export dialog was not answered in 15 minutes."
    except Exception as e:
        return _refused("export_secret_chat", e, secret_chat_id=secret_chat_id)
