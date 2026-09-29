"""Keep a client's dropped socket out of the error log.

Windows' proactor event loop reports `ConnectionResetError` (WinError 10054) from
`_ProactorBasePipeTransport._call_connection_lost` as "Exception in callback" with a
full traceback whenever an HTTP client resets its connection instead of closing it.
Every request has been answered by then; the traceback only made a healthy server look
broken (owner, 2026-09-30). Only that one shape is dropped - everything else still
reaches the handler that was there before.
"""

import asyncio

__all__ = ["install"]


def _is_peer_reset(context: dict) -> bool:
    where = f"{context.get('handle')!r} {context.get('message', '')}"
    return isinstance(context.get("exception"), ConnectionResetError) and (
        "_call_connection_lost" in where
    )


def install(loop: asyncio.AbstractEventLoop) -> None:
    previous = loop.get_exception_handler()

    def handler(this_loop, context):
        if _is_peer_reset(context):
            return
        if previous is not None:
            previous(this_loop, context)
        else:
            this_loop.default_exception_handler(context)

    loop.set_exception_handler(handler)
