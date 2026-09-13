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


def _is_usable(client: TDLibClient) -> bool:
    """Ready, and nothing else.

    This was "not in a small set of dead states", which is a different and much
    weaker claim: `authorizationStateWaitPassword`, `WaitCode` and any state
    this list has never heard of all passed it. A cached client sitting at
    WaitPassword was handed to a secret-chat tool, which then failed with a
    message about whichever call happened to come first.
    """
    return client._client_id is not None and client.authorization_state == _READY


async def _close_quietly(client: TDLibClient, why: str) -> None:
    """Close a client whose failure is already being reported."""
    try:
        await client.close()
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
        existing = _by_account.get(account)
        if existing is not None:
            if _is_usable(existing) and _verified_against.get(account) is telethon:
                return existing
            # Not merely stale - dead, or belonging to a generation that has
            # since been replaced. Dropped first so a failure to close it cannot
            # leave the corpse in the cache for the next caller.
            _by_account.pop(account, None)
            _verified_against.pop(account, None)
            await _close_quietly(existing, "it was unusable or from a replaced generation")

        client = TDLibClient(account)
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
        except BaseException:
            await _close_quietly(client, "an identity that did not match")
            raise
        _by_account[account] = client
        _verified_against[account] = telethon
        return client


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
    try:
        async with _by_account_lock:
            pending = list(_by_account.items())

        deadline = time.monotonic() + budget
        for account, client in pending:
            left = deadline - time.monotonic()
            if left <= 0:
                failures.append(
                    (account, TimeoutError("the shutdown budget ran out before this account"))
                )
                continue
            try:
                await asyncio.wait_for(asyncio.shield(client.close()), timeout=left)
            except Exception as error:
                # RETAINED on failure: a client that did not confirm it closed
                # still holds its database, and forgetting it here is how the
                # next start came to run against one mid-checkpoint.
                failures.append((account, error))
                log_event(
                    logging.ERROR,
                    "a TDLib client did not close cleanly; its unflushed keys may be lost",
                    error=error,
                )
                continue
            async with _by_account_lock:
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
