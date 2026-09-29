"""A client that drops its socket is not an error (spec 027-log).

On Windows the proactor loop logs `ConnectionResetError: [WinError 10054]` as an ERROR
with a full traceback from `_ProactorBasePipeTransport._call_connection_lost` whenever
an HTTP client resets the connection instead of closing it - after every request had
already been answered. The owner read those tracebacks as the server being broken.
"""

import asyncio

from telegram_mcp import loop_noise


class _Handle:
    def __repr__(self):
        return "<Handle _ProactorBasePipeTransport._call_connection_lost()>"


def _loop_with_spy():
    loop = asyncio.new_event_loop()
    seen = []
    loop.default_exception_handler = seen.append
    return loop, seen


def test_a_peer_reset_on_connection_lost_is_dropped():
    loop, seen = _loop_with_spy()
    try:
        loop_noise.install(loop)
        loop.call_exception_handler(
            {
                "message": "Exception in callback _ProactorBasePipeTransport._call_connection_lost()",
                "exception": ConnectionResetError(10054, "forcibly closed"),
                "handle": _Handle(),
            }
        )
        assert seen == []
    finally:
        loop.close()


def test_any_other_error_is_still_reported():
    loop, seen = _loop_with_spy()
    try:
        loop_noise.install(loop)
        other = {"message": "boom", "exception": ValueError("x"), "handle": _Handle()}
        reset_elsewhere = {
            "message": "Exception in callback other()",
            "exception": ConnectionResetError(),
        }
        loop.call_exception_handler(other)
        loop.call_exception_handler(reset_elsewhere)
        assert seen == [other, reset_elsewhere]
    finally:
        loop.close()


def test_a_handler_already_installed_keeps_receiving_the_rest():
    loop = asyncio.new_event_loop()
    try:
        earlier = []
        loop.set_exception_handler(lambda _loop, context: earlier.append(context))
        loop_noise.install(loop)
        context = {"message": "boom", "exception": ValueError("x")}
        loop.call_exception_handler(context)
        assert earlier == [context]
    finally:
        loop.close()
