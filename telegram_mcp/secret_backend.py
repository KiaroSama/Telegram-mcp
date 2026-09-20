"""The one place this server reaches the secret-chat implementation.

Nineteen call sites across five tool modules used to reach TDLib through
`tdlib_registry.secret_client`. They reach `secret_manager` here instead, and this
module is the ONLY one that imports `telethon_secret_chat` — a test enforces that,
because the package is maintained in its own repository and updated independently, so
an upstream signature change has to be a one-file edit rather than a search.

**Why the manager rides the caller's client.** TDLib keeps its own authorization and
cannot import a Telethon one, so every account that used secret chats showed the
operator two devices. The manager is constructed over the Telethon client this server
already holds for that account, whose lease `claim_session` already owns. It opens no
socket and authorises nothing, which is Principle I discharged by construction rather
than by a check.

The caching rules below are not decoration. They are what the registry this replaces
learned: a backend cached against a label rather than against the CLIENT hands a caller
a manager wired to a generation nobody uses any more, and a backend started while
shutdown is flushing races the flush for key material that cannot be recovered.
"""

import asyncio
from typing import Dict

from telethon_secret_chat import FileStorage, SecretChatManager

from telegram_mcp.settings import state_dir

__all__ = ["SecretChatUnavailable", "close_all", "secret_manager"]


class SecretChatUnavailable(RuntimeError):
    """A secret-chat backend cannot be served right now, and why.

    Its own type because the fixes are specific and nothing else here needs them:
    this is not a permission problem, a network problem or a wrong account name, and
    answering with any of those sends the caller looking in the wrong place.
    """

    def __init__(self, account: str, reason: str):
        super().__init__(f"Secret chats are unavailable for {account!r}: {reason}.")
        self.account = account
        self.reason = reason


#: account -> its started manager.
_by_account: Dict[str, SecretChatManager] = {}

#: account -> the client its manager was built over. A manager is reused only while
#: this is still the account's current client; see `secret_manager`.
_verified_against: Dict[str, object] = {}

#: Set for the life of the process once shutdown starts. Never cleared: a server that
#: has begun flushing key material does not go back to serving.
_closing = False

_lock = asyncio.Lock()


def _telethon_client(account: str):
    """The account's current client generation.

    Indirection on purpose: `connection` reaches this module through the tool layer,
    so importing it at module scope closes a cycle, and a test needs one place to
    point somewhere else.
    """
    from telegram_mcp.connection import get_client

    return get_client(account)


def _storage_for(account: str) -> FileStorage:
    """Where this account's key material lives.

    Under the server's state directory, never the install directory, and one
    directory per account so two accounts in one process cannot open each other's.
    The package requires storage explicitly and has no default, which is the right
    call — a library that quietly writes key material writes it somewhere the
    operator did not protect.
    """
    path = state_dir() / "secret-chats" / account
    path.parent.mkdir(parents=True, exist_ok=True)
    return FileStorage(path)


async def secret_manager(account: str) -> SecretChatManager:
    """The account's started secret-chat manager.

    Raises rather than returning a half-usable object: every secret-chat operation
    needs a real authorization, and a manager that is merely constructed would fail
    later with a message about whichever call happened to come first.

    Lazy. An account that never opens a secret chat never builds one.
    """
    if _closing:
        raise SecretChatUnavailable(account, "the server is shutting down")

    client = _telethon_client(account)

    async with _lock:
        # RE-READ both inside the lock. The checks above only describe the moment
        # this caller arrived; a caller that queued on the lock passed them and then
        # would act on state that changed while it waited.
        if _closing:
            raise SecretChatUnavailable(account, "the server is shutting down")
        if _telethon_client(account) is not client:
            raise SecretChatUnavailable(
                account,
                "the account was reconfigured while its secret-chat backend was being "
                "started, so this generation is no longer current; retry the call",
            )

        existing = _by_account.get(account)
        if existing is not None and _verified_against.get(account) is client:
            return existing

        if existing is not None:
            # A manager from a generation that has since been replaced. Stop it
            # before building its successor, so the old subscription is gone and its
            # chats are flushed; two managers over one account would mean two key
            # stores for one conversation.
            await _stop(existing)

        manager = SecretChatManager(client, _storage_for(account))
        await manager.start()
        _by_account[account] = manager
        _verified_against[account] = client
        return manager


async def _stop(manager: SecretChatManager) -> None:
    """Stop one manager, letting a failure surface rather than be swallowed.

    `stop()` flushes each chat. A swallowed failure here is lost key material, which
    no restart brings back, so it is reported rather than absorbed.
    """
    await manager.stop()


async def close_all() -> None:
    """Stop every manager, flushing key material. Idempotent.

    Shutdown can be reached twice — once from a signal handler and once from the
    runner's own path — so the second call must be a no-op rather than a failure.
    """
    global _closing
    _closing = True

    async with _lock:
        accounts = list(_by_account)
        for account in accounts:
            manager = _by_account.pop(account, None)
            _verified_against.pop(account, None)
            if manager is not None:
                await _stop(manager)
