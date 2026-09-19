"""Which started TDLib client serves which account, and closing them all.

Split from ``tdlib`` because it is a lifecycle question, not a protocol one: the
client class knows how to talk to TDLib, this knows which one is alive, whether
it is still usable, and what shutdown owes it.

Two facts shape everything here:

* **A client can die without this process doing anything.** The session is
  terminated from another device, or TDLib closes itself after an error, and the
  client moves to ``authorizationStateClosed`` or ``authorizationStateLoggingOut``
  while its ``_client_id`` is still set. Caching on that id alone handed the
  caller a corpse whose every call failed.
* **Closing is not optional.** TDLib writes secret-chat keys lazily, and a key
  lost on exit takes its chat's history with it - there is no way to re-derive
  one. So shutdown closes every client even when the first one refuses.
"""

# Annotations as strings, so naming `TDLibClient` below does not need it
# imported. `tdlib` re-exports this module from its own last lines, so a
# module-scope import of it here made THIS module unimportable first: the cycle
# resolved only if something happened to import `tdlib` before it.
from __future__ import annotations

import asyncio
import logging
import time
from typing import TYPE_CHECKING, List, Tuple

from telegram_mcp.safe_log import log_event

if TYPE_CHECKING:  # pragma: no cover - for readers and type checkers only
    from telegram_mcp.tdlib import TDLibClient

# States in which a client still holds a `_client_id` but can no longer serve.
# Kept as a name because it reads in the log lines and the tests, but it is no
# longer what decides usability - see `_is_usable`.
_DEAD_AUTH_STATES = frozenset(
    {
        "authorizationStateClosed",
        "authorizationStateClosing",
        "authorizationStateLoggingOut",
    }
)

# The one state in which a client can serve a secret-chat call.
_READY = "authorizationStateReady"

_by_account: dict[str, TDLibClient] = {}
_by_account_lock = asyncio.Lock()

# Set once shutdown starts. A new client against a database currently being
# flushed is never right, and the previous answer - clearing the registry before
# awaiting the closes - actively invited one.
_closing: bool = False

# How long every close gets, together. One client that will not finish must not
# consume the whole exit path, and the accounts after it still need their turn.
_CLOSE_ALL_BUDGET = 60.0

# account -> the Telethon client object this TDLib client was proved against.
# Object identity IS the generation: a re-login or a reload replaces the object,
# and that is exactly when the proof has to be redone.
_verified_against: dict[str, object] = {}

# A timeout cancels a waiter, not the native close. Keep that work owned and
# reuse it on subsequent shutdown calls instead of issuing duplicate closes.
_close_tasks: dict[int, asyncio.Future] = {}


def _is_usable(client: TDLibClient) -> bool:
    """Ready, and nothing else.

    This was "not in a small set of dead states", which is a different and much
    weaker claim: `authorizationStateWaitPassword`, `WaitCode` and any state
    this list has never heard of all passed it. A cached client sitting at
    WaitPassword was handed to a secret-chat tool, which then failed with a
    message about whichever call happened to come first.
    """
    return client._client_id is not None and client.authorization_state == _READY


def _start_close(account: str, client) -> asyncio.Future:
    """Own one close per client, including completion after a waiter's deadline."""
    key = id(client)
    task = _close_tasks.get(key)
    if task is not None:
        return task
    task = asyncio.ensure_future(client.close())
    _close_tasks[key] = task

    def completed(done):
        if _close_tasks.get(key) is done:
            _close_tasks.pop(key, None)
        # Retrieve failures even when the last waiter timed out or was cancelled.
        if done.cancelled():
            return
        error = done.exception()
        if error is not None:
            log_event(logging.WARNING, "a TDLib close failed; ownership retained", error=error)
            return
        # These callbacks run on the registry's loop, without an intervening
        # await. Never remove a replacement that has since acquired this label.
        if _by_account.get(account) is client:
            _by_account.pop(account, None)
            _verified_against.pop(account, None)

    task.add_done_callback(completed)
    return task


async def _close_quietly(client: TDLibClient, why: str) -> None:
    """Retain failed/incomplete starts until their database owner actually closes."""
    account = client.account
    _by_account[account] = client
    _verified_against.pop(account, None)
    try:
        await asyncio.shield(_start_close(account, client))
    except asyncio.CancelledError:
        raise
    except Exception as error:
        log_event(logging.WARNING, f"failed to close a TDLib client after {why}", error=error)


async def secret_client(account: str) -> TDLibClient:
    """The account's started TDLib client.

    Raises `NotSignedIn` rather than returning a half-usable client: every
    secret-chat operation needs a real authorisation, and a client that is
    merely running would fail later with a message about whatever call happened
    to come first.
    """
    from telegram_mcp.tdlib import NotSignedIn, TDLibClient

    # Imported here, not at module scope: `connection` reaches this module
    # through `tdlib`, and naming it at the top closes the cycle.
    from telegram_mcp.connection import get_client

    if _closing:
        raise NotSignedIn(account, "the server is shutting down")

    # The Telethon half of the SAME label, which is what the database has to
    # agree with. Checking the database against its own recorded metadata only
    # proves it has not changed its mind about itself.
    telethon = get_client(account)

    async with _by_account_lock:
        # RE-READ inside the lock. Reading it above only says the shutdown had
        # not begun when this caller arrived; a caller that queued on the lock
        # passed that check and then started a native client against a database
        # `close_all` was already flushing.
        if _closing:
            raise NotSignedIn(account, "the server is shutting down")
        # And re-read the generation: taking the lock is a wait, and a reload
        # during it makes the client captured above the PREVIOUS account's.
        if get_client(account) is not telethon:
            raise NotSignedIn(
                account,
                "the account was reconfigured while its secret-chat client was being "
                "started, so this generation is no longer current; retry the call",
            )

        existing = _by_account.get(account)
        if existing is not None:
            if _is_usable(existing) and _verified_against.get(account) is telethon:
                return existing
            # Not merely stale - dead, or belonging to a generation that has
            # since been replaced. The close has to be CONFIRMED before a
            # replacement starts: a swallowed failure here meant a second native
            # client opening a database the first one may still have held, which
            # is the one thing this registry exists to prevent.
            try:
                await asyncio.shield(_start_close(account, existing))
            except asyncio.CancelledError:
                raise
            except Exception as error:
                raise NotSignedIn(
                    account,
                    "the previous secret-chat client for this account did not close, so "
                    "its database is still in use and a replacement was not started "
                    f"({type(error).__name__})",
                ) from error
            _by_account.pop(account, None)
            _verified_against.pop(account, None)

        if _closing or get_client(account) is not telethon:
            raise NotSignedIn(account, "the account was reconfigured or is shutting down")
        client = TDLibClient(account)
        # Reserve ownership before the first await. Failed starts must be visible
        # to shutdown and cannot be replaced while cleanup still owns the DB.
        _by_account[account] = client
        _verified_against.pop(account, None)
        try:
            state = await client.start()
        except BaseException:
            # Cancellation included: `start()` brings up a native client and
            # registers it with the reader thread, and abandoning that leaked
            # both for the life of the process.
            await _close_quietly(client, "a failed start")
            raise
        if state != "authorizationStateReady":
            await _close_quietly(client, "an incomplete authorisation")
            raise NotSignedIn(account, state)
        # AFTER the start, which is a real wait on a native library. The lease
        # this client was started for has to still be the current one, or what
        # gets published is the previous account's database under the new
        # account's name - and every secret-chat call then runs as the old owner.
        try:
            if _closing or get_client(account) is not telethon:
                raise NotSignedIn(
                    account,
                    "the account was reconfigured while its secret-chat client was being "
                    "started, so this generation is no longer current; retry the call",
                )
        except BaseException:
            await _close_quietly(client, "the account generation changed during the start")
            raise
        # Imported here, not at module scope: `tdlib_identity` reads
        # `database_dir_for` out of `tdlib`, which re-exports this module, and
        # naming it at the top closes the cycle.
        from telegram_mcp import tdlib_identity

        try:
            # Against the ACTIVE Telethon session, not against the database's own
            # recorded metadata. Comparing a database with the note beside it only
            # proves it has not changed its mind about itself: a label reused for
            # another account has an old database AND an old note, they agree
            # perfectly, and every secret-chat call runs as the previous owner.
            await tdlib_identity.verify_owner(account, client, telethon)
            if _closing or get_client(account) is not telethon:
                raise NotSignedIn(
                    account, "the account was reconfigured or shut down during identity verification"
                )
            if not _is_usable(client):
                raise NotSignedIn(account, "authorization changed during identity verification")
        except BaseException:
            await _close_quietly(client, "an identity that did not match")
            raise
        _by_account[account] = client
        _verified_against[account] = telethon
        return client


def _went(result) -> bool:
    """Whether one `_close_one` answer means the client CONFIRMED it closed."""
    return not isinstance(result, BaseException) and result[1] is None


async def close_all(budget: float = _CLOSE_ALL_BUDGET) -> List[Tuple[str, Exception]]:
    """Shut every started client down, flushing its database.

    Returns the accounts whose close failed, so shutdown can say so. Every
    client is attempted within one total budget: a `for` loop over
    `await close()` abandoned the rest after the first refusal, which is the
    shape that loses exactly the keys this function exists to flush.

    Two things this does NOT do any more:

    * **Clear the registry first.** Taking and clearing under the lock left a
      window in which `secret_client` saw an empty registry and started a SECOND
      native client against a database the first was still checkpointing. The
      latch below closes that instead, and an entry is dropped only once its
      client has confirmed it closed.
    * **Let a cancellation abandon the remainder.** Clients whose close never
      ran stay registered and stay owned, so a later attempt - or the report
      this returns - still knows about them.
    """
    global _closing
    _closing = True
    failures: List[Tuple[str, Exception]] = []
    # From HERE, not from after the lock. The registry lock can be held by a
    # `secret_client` still starting, and that wait used to be free: the budget
    # began once it was granted, so a shutdown given ten seconds could take
    # thirty.
    deadline = time.monotonic() + budget
    try:
        try:
            left = deadline - time.monotonic()
            await asyncio.wait_for(_by_account_lock.acquire(), timeout=max(left, 0.0))
        except (asyncio.TimeoutError, TimeoutError):
            # The budget is gone before anything could be read. Say so rather
            # than closing nothing silently.
            return [(name, TimeoutError("the shutdown budget ran out")) for name in _by_account]
        try:
            pending = list(_by_account.items())
        finally:
            _by_account_lock.release()

        # CONCURRENT, bounded by the same deadline. Closing sequentially let the
        # first stalled client spend the whole budget, so the accounts behind it
        # were never even ASKED - and a TDLib client that is never asked to close
        # is exactly the unflushed secret-chat keys this function exists for.
        async def _close_one(account, client):
            left = deadline - time.monotonic()
            if left <= 0:
                return account, TimeoutError("the shutdown budget ran out before this account")
            try:
                await asyncio.wait_for(
                    asyncio.shield(_start_close(account, client)), timeout=left
                )
            except Exception as error:
                return account, error
            return account, None

        results = await asyncio.gather(
            *(_close_one(account, client) for account, client in pending),
            return_exceptions=True,
        )
        for (account, client), result in zip(pending, results):
            error = result if isinstance(result, BaseException) else result[1]
            if error is None:
                continue
            # RETAINED on failure: a client that did not confirm it closed
            # still holds its database, and forgetting it here is how the
            # next start came to run against one mid-checkpoint.
            failures.append((account, error))
            log_event(
                logging.ERROR,
                "a TDLib client did not close cleanly; its unflushed keys may be lost",
                error=error,
            )

        closed = {account for (account, _c), result in zip(pending, results) if _went(result)}
        # No second, unbudgeted lock acquisition after waiting for closes.
        # Identity-checked, synchronous updates cannot interleave on this loop.
        for account, client in pending:
            if account in closed and _by_account.get(account) is client:
                _by_account.pop(account, None)
                _verified_against.pop(account, None)
    finally:
        # The latch stays SET. Once shutdown has begun, a new native client
        # against a database being flushed is never the right answer.
        pass
    return failures


__all__ = [
    "_CLOSE_ALL_BUDGET",
    "_DEAD_AUTH_STATES",
    "_READY",
    "_verified_against",
    "_by_account",
    "_by_account_lock",
    "_is_usable",
    "close_all",
    "secret_client",
]
