"""Temporary ordinary-chat typing (upstream20601a2, Apache-2.0 adaptation)."""

import math

from telegram_mcp.runtime import *
from telegram_mcp.safe_log import log_event
from telegram_mcp import preflight

__all__ = ["send_typing"]
_ACTIVE = set()


def _arguments_problem(arguments):
    seconds = arguments.get("seconds", 5.0)
    if (
        isinstance(seconds, bool)
        or not isinstance(seconds, (int, float))
        or not math.isfinite(seconds)
        or not 0.5 <= seconds <= 45
    ):
        return "seconds must be finite and between 0.5 and 45."
    topic = arguments.get("topic_id")
    if topic is not None and (type(topic) is not int or topic < 1):
        return "topic_id must be a positive integer."
    return None


preflight.RULES["send_typing"] = _arguments_problem


@mcp.tool(
    annotations=ToolAnnotations(
        title="Send Typing",
        readOnlyHint=False,
        destructiveHint=True,
        idempotentHint=False,
        openWorldHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def send_typing(
    chat_id: Union[int, str],
    seconds: float = 5.0,
    topic_id: Optional[int] = None,
    account: str = None,
) -> str:
    """Show temporary typing for 0.5..45 seconds, then clear it.

    This is visible to the chat, not a read receipt. Waits for the duration.
    A concurrent call in the same chat/topic refuses. Cancellation attempts a
    bounded clear; Telegram also expires the indicator automatically.
    Owner approval is always required by the existing destructive safety net.
    """
    problem = _arguments_problem({"seconds": seconds, "topic_id": topic_id})
    if problem:
        return problem
    key = None
    try:
        cl = get_client(account)
        peer = await resolve_input_entity(chat_id, cl)
        peer_key = "self" if isinstance(peer, types.InputPeerSelf) else utils.get_peer_id(peer)
        key = (id(cl), peer_key, topic_id)
        if key in _ACTIVE:
            return "A typing operation is already active in this chat/topic."
        _ACTIVE.add(key)
        cleared = False
        try:

            async def show():
                ticks = math.ceil(seconds / 4)
                for _ in range(ticks):
                    accepted = await cl(
                        functions.messages.SetTypingRequest(
                            peer=peer, action=types.SendMessageTypingAction(), top_msg_id=topic_id
                        ),
                        flood_sleep_threshold=0,
                    )
                    if not accepted:
                        raise ValueError("Telegram did not accept the typing request.")
                    await asyncio.sleep(seconds / ticks)

            await asyncio.wait_for(show(), timeout=48)
        finally:
            try:
                cleared = bool(
                    await asyncio.wait_for(
                        cl(
                            functions.messages.SetTypingRequest(
                                peer=peer,
                                action=types.SendMessageCancelAction(),
                                top_msg_id=topic_id,
                            ),
                            flood_sleep_threshold=0,
                        ),
                        timeout=3,
                    )
                )
            except Exception:
                log_event(logging.WARNING, "Typing cleanup was not confirmed")
            finally:
                _ACTIVE.discard(key)
        return (
            "Typing requests accepted; indicator cleared."
            if cleared
            else "Typing requests accepted; clearing was not confirmed."
        )
    except Exception as error:
        return log_and_format_error("send_typing", error)
