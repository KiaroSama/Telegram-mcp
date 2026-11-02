"""Held-directory download transaction shared by message and story media.

The callers inject their existing filesystem gates, retaining one guard/transaction
rather than implementing a less safe story-only download path.
"""

import logging
from pathlib import Path

from telegram_mcp.file_names import safe_suffix
from telegram_mcp.handles import NAME_ATTEMPTS, UnsafeTarget, open_verified_file
from telegram_mcp.safe_log import log_event


class DownloadTooLarge(Exception):
    """An over-cap stream, stopped before publication."""


async def save_media(client, message, out_path, cap, ctx, open_directory):
    async with open_directory(path=out_path.parent, ctx=ctx, tool_name="download_media") as (
        parent,
        error,
    ):
        if error:
            return error
        _name, staging = parent.make_private_subdirectory(".download-")
        try:

            def stop_at_cap(received, _total):
                if received > cap:
                    raise DownloadTooLarge(
                        f"Download aborted: the stream passed the {cap}-byte limit "
                        "(max_bytes). Nothing was kept."
                    )

            downloaded = await client.download_media(
                message,
                file=str(Path(staging.path) / "part"),
                progress_callback=stop_at_cap,
            )
            if not downloaded:
                return f"Download failed for message {message.id}."
            produced = Path(downloaded)
            if produced.parent.resolve(strict=False) != Path(staging.path).resolve(strict=False):
                return "Download refused: the transfer wrote outside the directory this call created for it."
            with open_verified_file(staging, produced.name) as fetched:
                if fetched.size > cap:
                    return (
                        f"Download refused: the file turned out to be larger than the "
                        f"{cap}-byte limit (max_bytes). Nothing was kept."
                    )
                staged = fetched.identity
            staging.sync_child(produced.name)
            final = out_path.with_suffix(safe_suffix(produced.suffix))
            final_name = parent.reserve_free_name(final.stem, final.suffix)
            if final_name is None:
                return (
                    f"Download refused: {NAME_ATTEMPTS} names near "
                    f"{out_path.name} are already taken. Pass file_path to choose one."
                )
            try:
                parent.install(staging, produced.name, final_name, expect_source=staged)
            except BaseException:
                parent.discard(final_name)
                raise
            parent.sync()
            return f"Media downloaded to {Path(parent.path) / final_name}."
        finally:
            try:
                staging.remove_tree()
                staging.remove_self()
            except (OSError, UnsafeTarget) as error:
                log_event(logging.WARNING, "could not remove the download directory", error=error)
            finally:
                staging.close()
