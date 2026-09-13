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
from typing import TYPE_CHECKING, List, Tuple

from telegram_mcp.safe_log import log_event

if TYPE_CHECKING:  # pragma: no cover - for readers and type checkers only
    from telegram_mcp.tdlib import TDLibClient

# States in which a client still holds a `_client_id` but can no longer serve.
_DEAD_AUTH_STATES = frozenset(
    {
        "authorizationStateClosed",
        "authorizationStateClosing",
        "authorizationStateLoggingOut",
    }
)

_by_account: dict[str, TDLibClient] = {}
_by_account_lock = asyncio.Lock()


def _is_usable(client: TDLibClient) -> bool:
    return client._client_id is not None and client.authorization_state not in _DEAD_AUTH_STATES


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

    async with _by_account_lock:
        existing = _by_account.get(account)
        if existing is not None:
            if _is_usable(existing):
                return existing
            # Not merely stale - dead. Dropped first so a failure to close it
            # cannot leave the corpse in the cache for the next caller.
            _by_account.pop(account, None)
            await _close_quietly(existing, "it was found closed or logging out")

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
            # Against the binding recorded when this database was last proved,
            # since there is no Telethon session in hand here. A label reused for
            # another account reaches this path without ever going through the
            # login tool, and every secret-chat call made through the client
            # would run as the previous owner.
            await tdlib_identity.verify_owner(account, client, None)
        except BaseException:
            await _close_quietly(client, "an identity that did not match")
            raise
        _by_account[account] = client
        return client


async def close_all() -> List[Tuple[str, Exception]]:
    """Shut every started client down, flushing its database.

    Returns the accounts whose close failed, so shutdown can say so. Every
    client is attempted: a `for` loop over `await close()` abandoned the rest
    after the first refusal and never cleared the registry, which is the shape
    that loses exactly the keys this function exists to flush.
    """
    async with _by_account_lock:
        # Taken and cleared under the lock, so a caller arriving mid-shutdown
        # builds a fresh client rather than waiting on one being closed.
        closing = list(_by_account.items())
        _by_account.clear()

    failures: List[Tuple[str, Exception]] = []
    for account, client in closing:
        try:
            await client.close()
        except Exception as error:
            failures.append((account, error))
            log_event(
                logging.ERROR,
                "a TDLib client did not close cleanly; its unflushed keys may be lost",
                error=error,
            )
    return failures


__all__ = [
    "_DEAD_AUTH_STATES",
    "_by_account",
    "_by_account_lock",
    "_is_usable",
    "close_all",
    "secret_client",
]
