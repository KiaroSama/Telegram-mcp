"""Removing messages, and the budget that removal runs under.

Split from ``messages.py`` along the seam the tests already used - there has
been a ``tests/test_message_deletion.py`` for as long as there have been these
tools. Deleting is not the tail end of sending: it is irreversible, ``revoke``
decides whether the other party keeps their copy, and ``delete_chat_history``
is the only tool here that loops.

That loop is why the two budget constants travel with it. A history is deleted
in passes, bounded by BOTH a pass ceiling and a wall-clock deadline - the
deadline applies to the RPC as well as to the loop around it, because a call
that never returned used to outlive the budget entirely.

The rendering helpers stay in ``messages``: ``message_view``, ``inspection``,
``scheduled`` and ``messages_read`` all import them from there.
"""

from telegram_mcp.runtime import *
from telegram_mcp.entities import build_send_entities
import random

from telethon import utils as telethon_utils
from telethon.tl.types import InputReplyToMessage

from telegram_mcp.forum import topic_reply_to, topic_reply_to_request
from telegram_mcp.sent import sent_message_ids

__all__ = [
    "delete_chat_history",
    "delete_message",
    "delete_messages_bulk",
]


# `messages.deleteMessages` caps one call at 100 ids.
_DELETE_CHUNK = 100


async def _ids_that_live_here(cl, entity, message_ids):
    """Split `message_ids` into (here, missing, elsewhere) for a NON-channel chat.

    Outside a channel the request carries no peer at all - Telethon's own
    documentation warns that these ids are account-global - so an id copied from
    another conversation deletes a message THERE while the caller names this
    chat, irreversibly and with no error. The only way to tell them apart is to
    look the ids up first and compare the peer each message reports against the
    one the caller asked for.
    """
    here, missing, elsewhere = [], [], []
    wanted = telethon_utils.get_peer_id(entity)
    for start in range(0, len(message_ids), _DELETE_CHUNK):
        batch = message_ids[start : start + _DELETE_CHUNK]
        found = await cl.get_messages(entity, ids=batch)
        for asked, message in zip(batch, found):
            if message is None:
                missing.append(asked)
            elif telethon_utils.get_peer_id(message.peer_id) != wanted:
                elsewhere.append(asked)
            else:
                here.append(asked)
    return here, missing, elsewhere


def _wrong_chat_refusal(elsewhere, chat_id):
    shown = ", ".join(str(i) for i in elsewhere[:10])
    more = "" if len(elsewhere) <= 10 else f" (and {len(elsewhere) - 10} more)"
    return (
        f"Refusing to delete: {len(elsewhere)} of the ids given are not in chat "
        f"{chat_id} - {shown}{more}. Outside a channel Telegram treats a message "
        "id as account-global, so deleting these would remove messages from a "
        "different conversation and could not be undone. Re-read the ids from "
        "this chat."
    )


_DELETE_HISTORY_MAX_PASSES = 20


_DELETE_HISTORY_DEADLINE_SECONDS = 60.0


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Message", openWorldHint=True, destructiveHint=True, idempotentHint=True
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def delete_message(
    chat_id: Union[int, str], message_id: int, revoke: bool = False, account: str = None
) -> str:
    """
    Delete a message by ID, from this account's view unless told otherwise.

    Args:
        chat_id: Chat ID or username.
        message_id: The message to delete.
        revoke: Pass True to delete the message for EVERYONE in the chat, wherever
            Telegram still permits it. The default removes it from this account's
            view only, because that is the one of the two that can be lived with
            if it was the wrong message. A channel has no per-account copy, so
            there `revoke=False` is REFUSED rather than quietly widened - pass
            True to mean it.
    """
    try:
        cl = get_client(account)
        entity = await resolve_entity(chat_id, cl)
        if isinstance(entity, Channel) and not revoke:
            # Refused BEFORE the call, not silently widened. `revoke=False` asks
            # for a deletion only this account sees; a channel has no such thing,
            # and going ahead would remove the post for every subscriber while
            # the caller believed they were tidying their own view.
            return (
                f"Refusing to delete message {message_id} from chat {chat_id}: "
                "revoke=False asks for a deletion only you see, and a channel has "
                "no per-account copy - removing it there removes it for every "
                "subscriber. Pass revoke=True to say that is what you mean."
            )
        # Telethon's friendly method defaults to revoke=True, and so did this,
        # which made the least alarming-sounding call the most destructive one on
        # offer: an agent tidying its own view took the message out of the
        # recipient's chat as well. Reaching the other party is now something the
        # caller asks for.
        await cl.delete_messages(entity, message_id, revoke=revoke)
        # Report what Telegram DID, not what the flag asked for. A channel or
        # supergroup keeps no per-account copy, so `channels.deleteMessages`
        # removes the message for every subscriber whatever `revoke` says - and
        # echoing the flag meant a channel post that was already gone for
        # everyone was reported as "deleted for you only". The caller then has
        # every reason to believe it is still up, which is the one thing this
        # sentence exists to settle.
        if isinstance(entity, Channel):
            scope = "for everyone (a channel keeps no per-account copy)"
        else:
            scope = "for both parties" if revoke else "for you only"
        return f"Message {message_id} deleted {scope}."
    except Exception as e:
        return log_and_format_error("delete_message", e, chat_id=chat_id, message_id=message_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Chat History",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def delete_chat_history(
    chat_id: Union[int, str], max_id: int = 0, revoke: bool = False, account: str = None
) -> str:
    """
    Clear the full message history of a chat.

    Args:
        chat_id: Chat ID or username.
        max_id: Delete messages up to this ID; 0 deletes all messages (default).
        revoke: If True, delete for both parties (default False = only for you).
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)

        # messages.affectedHistory.offset is a continuation signal: a positive
        # value means the method has to be repeated with the same parameters
        # until it answers zero. One call and a "cleared" report was a claim the
        # server had not made -- it had said the opposite.
        deleted = 0
        offset = None
        passes = 0
        deadline = time.monotonic() + _DELETE_HISTORY_DEADLINE_SECONDS
        stalled = False
        timed_out = False
        while passes < _DELETE_HISTORY_MAX_PASSES:
            budget_left = deadline - time.monotonic()
            if budget_left <= 0:
                break
            try:
                # The budget bounds the CALL, not merely the gap between calls.
                # Checked only afterwards, a single request that never returned
                # sat past the deadline for as long as Telegram felt like, and
                # only cancellation from outside ever ended it. wait_for cancels
                # the request it is waiting on, so nothing is left running.
                result = await asyncio.wait_for(
                    cl(
                        functions.messages.DeleteHistoryRequest(
                            peer=entity, max_id=max_id, revoke=revoke
                        )
                    ),
                    timeout=budget_left,
                )
            except (asyncio.TimeoutError, TimeoutError):
                timed_out = True
                break
            passes += 1
            deleted += getattr(result, "pts_count", 0) or 0
            remaining = getattr(result, "offset", 0) or 0
            if remaining <= 0:
                offset = 0
                break
            # A remainder that does not shrink is not progress. Repeating the
            # identical request against it is an unbounded spin, so it stops here
            # and says what is left rather than looping on hope.
            if offset is not None and remaining >= offset:
                offset = remaining
                stalled = True
                break
            offset = remaining

        scope = "for both parties" if revoke else "for you"
        if offset == 0:
            return f"Chat {chat_id} history cleared {scope}: {deleted} messages deleted."
        if stalled:
            reason = "the server stopped reporting progress"
        elif timed_out:
            reason = (
                f"a delete call did not answer inside the "
                f"{_DELETE_HISTORY_DEADLINE_SECONDS:g}s budget and was abandoned"
            )
        else:
            reason = f"the {passes}-pass/{_DELETE_HISTORY_DEADLINE_SECONDS:g}s budget ran out"
        # An unanswered first call leaves no offset to quote; saying "unknown" is
        # the honest form of "Telegram never told us".
        left = "unknown" if offset is None else offset
        return (
            f"Chat {chat_id} history deletion is INCOMPLETE {scope}: {deleted} messages "
            f"deleted over {passes} pass(es), and Telegram still reports offset={left} "
            f"left because {reason}. Run delete_chat_history again to continue."
        )
    except telethon.errors.rpcerrorlist.ChatAdminRequiredError:
        return "Cannot delete chat history: admin privileges are required."
    except Exception as e:
        return log_and_format_error(
            "delete_chat_history",
            e,
            chat_id=chat_id,
            max_id=max_id,
            revoke=revoke,
        )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Messages Bulk",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def delete_messages_bulk(
    chat_id: Union[int, str],
    message_ids: List[int],
    revoke: bool = True,
    account: str = None,
) -> str:
    """
    Delete multiple messages in a single call.

    Args:
        chat_id: Chat ID or username.
        message_ids: List of message IDs to delete.
        revoke: If True, delete for both parties (default True). Ignored for channels.

    Outside a channel Telegram treats a message id as account-global, not scoped
    to a chat: `messages.DeleteMessagesRequest` carries no peer field at all. So
    the ids are checked against this chat BEFORE anything is deleted, and any
    that live in another conversation make the whole call refuse.
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        entity = await resolve_entity(chat_id, cl)
        # Order-preserving dedupe: the same id twice is one deletion, and a
        # duplicate would otherwise inflate every number reported below.
        wanted = list(dict.fromkeys(message_ids))
        notes = []

        if isinstance(entity, Channel):
            # A channel request carries the peer, so an id from elsewhere simply
            # is not found here - there is nothing to guard against.
            targets = wanted
        else:
            targets, missing, elsewhere = await _ids_that_live_here(cl, entity, wanted)
            if elsewhere:
                return _wrong_chat_refusal(elsewhere, chat_id)
            if missing:
                notes.append(f"{len(missing)} id(s) were already gone or never existed here")
            if not targets:
                return (
                    f"Nothing to delete in chat {chat_id}: none of the "
                    f"{len(wanted)} id(s) given is a message in this chat."
                )

        sent = 0
        for start in range(0, len(targets), _DELETE_CHUNK):
            batch = targets[start : start + _DELETE_CHUNK]
            if isinstance(entity, Channel):
                await cl(functions.channels.DeleteMessagesRequest(channel=entity, id=batch))
            else:
                await cl(functions.messages.DeleteMessagesRequest(id=batch, revoke=revoke))
            sent += len(batch)

        # NOT `pts_count`. That field counts the update events the deletion
        # produced, which is not a count of messages removed - reporting it as
        # one produced sentences like "Deleted 7 of 2 messages". Telegram does
        # not say how many it removed, so no number is claimed that it did not
        # give: what is reported is what was ASKED for and accepted.
        report = f"Deletion accepted for {sent} message(s) in chat {chat_id}."
        return report if not notes else report + " " + "; ".join(notes) + "."
    except telethon.errors.rpcerrorlist.MessageIdInvalidError:
        return "Cannot delete messages: one or more message IDs are invalid."
    except telethon.errors.rpcerrorlist.ChatAdminRequiredError:
        return "Cannot delete messages: admin privileges are required."
    except Exception as e:
        return log_and_format_error(
            "delete_messages_bulk",
            e,
            chat_id=chat_id,
            message_ids=message_ids,
            revoke=revoke,
        )
