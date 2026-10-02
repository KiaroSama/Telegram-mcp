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
import json
import logging
import os
import tempfile
from pathlib import Path
from typing import Dict, List, Tuple

from telethon_secret_chat import (
    PALETTE,
    FileStorage,
    MediaReference,
    SecretChatManager,
    key_visualization,
)
from telethon_secret_chat.errors import (
    ChatClosed,
    ChatNotReady,
    LayerUnsupported,
    MessageRejected,
    ParameterRejected,
    ResendUnsatisfiable,
    SecretChatError,
    SendPending,
    StorageRequired,
)
from telethon_secret_chat.schema import secret_tl

from telegram_mcp import secret_history, secret_media_refs
from telegram_mcp.owner_only import restrict_to_owner_strict
from telegram_mcp.safe_log import log_event
from telegram_mcp.settings import state_dir

# Re-exported, because this module is the ONLY one allowed to import the package
# and a test enforces that. Two callers need pieces of it - `secret_common` to tell
# a refusal from a defect, and the typing tool to name one protocol action - and
# letting either import it directly would turn "an upstream change is a one-file
# edit" into "grep and hope".
__all__ = [
    "ChatClosed",
    "ChatNotReady",
    "LayerUnsupported",
    "MessageRejected",
    "PALETTE",
    "ParameterRejected",
    "ResendUnsatisfiable",
    "SecretChatError",
    "SecretChatUnavailable",
    "SendPending",
    "StorageRequired",
    "close_all",
    "key_visualization",
    "erase_local",
    "media_source",
    "secret_manager",
    "secret_tl",
]


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

# Tasks, not callers, own startup, native shutdown and threaded acquisition.
# A cancelled tool call cannot abandon a subscription or a newly acquired lease.
_starts: Dict[str, asyncio.Task] = {}
_stops: Dict[int, asyncio.Task] = {}
_acquisitions: Dict[str, asyncio.Task] = {}
_START_SECONDS = 60.0
_CLEANUP_SECONDS = 5.0
_CLOSE_SECONDS = 30.0


def _check_current(account: str, client) -> None:
    if _closing:
        raise SecretChatUnavailable(account, "the server is shutting down")
    try:
        current = _telethon_client(account)
    except (KeyError, ValueError):
        current = None
    if current is not client:
        raise SecretChatUnavailable(account, "the account was reconfigured; retry the call")


def _release_store(account: str) -> None:
    lease = _store_locks.pop(account, None)
    if lease is not None:
        lease.release()


def _stop_owned(account: str, manager) -> asyncio.Task:
    """One stop per manager; retain failures and reap a successful late stop."""
    key = id(manager)
    existing = _stops.get(key)
    if existing is not None:
        return existing
    _verified_against.pop(account, None)
    task = asyncio.create_task(_stop(manager))
    _stops[key] = task

    def finished(done):
        if _stops.get(key) is done:
            _stops.pop(key, None)
        if done.cancelled():
            return
        error = done.exception()
        if error is not None:
            log_event(
                logging.ERROR, "secret manager closure failed; ownership retained", error=error
            )
            return
        if _by_account.get(account) is manager:
            _by_account.pop(account, None)
            _verified_against.pop(account, None)
            _release_store(account)

    task.add_done_callback(finished)
    return task


def _guard_event(account, client, manager, handler):
    """An old subscription cannot write under a label's new generation."""

    async def guarded(event):
        try:
            _check_current(account, client)
        except SecretChatUnavailable:
            return
        if _by_account.get(account) is not manager or id(manager) in _stops:
            return
        await handler(event)

    return guarded


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


#: account -> the OS lock this process holds on that account's key store.
_store_locks: Dict[str, object] = {}


def _store_lock_dir() -> Path:
    return state_dir() / "secret-chats"


def _store_identity(account: str) -> str:
    return f"secret-chat-store:{account}"


async def _claim_store(account: str) -> None:
    """Own the actual acquire even when its asynchronous waiter is cancelled."""
    if account in _store_locks:
        return
    from telegram_mcp.singleton import SessionLock, SessionLockError

    lease = SessionLock(_store_identity(account), lock_dir=_store_lock_dir())
    task = asyncio.create_task(
        asyncio.to_thread(lease.acquire, grace_seconds=2.0, poll_interval=0.2)
    )
    _acquisitions[account] = task
    try:
        await asyncio.shield(task)
    except BaseException as error:
        # Cancellation does not stop a thread. Its completion keeps a strong
        # reference to the lease and releases it without ever publishing it.
        def abandoned(done):
            if _acquisitions.get(account) is done:
                _acquisitions.pop(account, None)
            if not done.cancelled() and done.exception() is None:
                lease.release()

        task.add_done_callback(abandoned)
        if isinstance(error, SessionLockError):
            raise SecretChatUnavailable(
                account, "another telegram-mcp process holds this account's secret-chat keys"
            ) from None
        raise
    _acquisitions.pop(account, None)
    if _closing:
        lease.release()
        raise SecretChatUnavailable(account, "the server is shutting down")
    _store_locks[account] = lease


def _owner_path(account: str) -> Path:
    """Which Telegram account a label's key store was written for."""
    return state_dir() / "secret-chats" / f"{account}.owner.json"


async def _bind_store(account: str, client) -> None:
    """Compare a valid identity record, or atomically adopt a genuinely new one.

    Legacy stores with no binding are still adopted as documented. A PRESENT
    but corrupt/unreadable binding is not a legacy store and is never replaced.
    """
    me = await client.get_me(input_peer=True)
    user_id = getattr(me, "user_id", None) or getattr(me, "id", None)
    if type(user_id) is not int or user_id <= 0:
        raise SecretChatUnavailable(account, "Telegram did not prove a valid account identity")
    _check_current(account, client)
    path = _owner_path(account)
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        raw = None
    except OSError as error:
        raise SecretChatUnavailable(
            account, "the key-store identity record could not be read"
        ) from error
    if raw is not None:
        try:
            record = json.loads(raw)
            recorded = record["user_id"]
            if type(recorded) is not int or recorded <= 0:
                raise ValueError("invalid recorded identity")
        except (ValueError, TypeError, KeyError):
            raise SecretChatUnavailable(
                account, "the key-store identity record is invalid; its bytes were preserved"
            ) from None
        if recorded != user_id:
            raise SecretChatUnavailable(
                account,
                "this label's secret-chat keys belong to a different Telegram account "
                f"(user {recorded}, not {user_id}); stop the server and repair the label binding",
            )
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, suffix=".owner.tmp")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            # Restrict before writing identity data, including on Windows.
            if not restrict_to_owner_strict(temporary):
                raise OSError("Temporary secret state could not be made owner-only")
            json.dump({"user_id": user_id}, handle)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            Path(temporary).unlink(missing_ok=True)
        except OSError as error:
            log_event(logging.WARNING, "could not remove temporary identity record", error=error)
        raise


async def _start_manager(account: str, client):
    """Hold startup resources until publication or positively confirmed cleanup."""
    manager = None
    claimed = False
    try:
        async with asyncio.timeout(_START_SECONDS):
            _check_current(account, client)
            existing = _by_account.get(account)
            if existing is not None:
                await asyncio.shield(_stop_owned(account, existing))
                _check_current(account, client)
            await _claim_store(account)
            claimed = True
            _check_current(account, client)
            await _bind_store(account, client)
            _check_current(account, client)
            manager = SecretChatManager(client, _storage_for(account))
            # Reserve before start(): it can subscribe and then raise/cancel.
            _by_account[account] = manager
            _verified_against.pop(account, None)
            for event, handler in (
                ("ChatRequested", _accept_incoming(manager, account)),
                ("MessageReceived", _remember(account)),
                ("ChatClosed", _closed(account)),
                ("SendFailed", _withdrawn(account)),
                ("ServiceActionReceived", _service_action(account)),
            ):
                manager.on(event, _guard_event(account, client, manager, handler))
            await manager.start()
            _check_current(account, client)
            _verified_against[account] = client
            return manager
    except BaseException:
        if manager is not None:
            cleanup = _stop_owned(account, manager)
            # asyncio.wait never cancels the owned cleanup at its deadline.
            await asyncio.wait({cleanup}, timeout=_CLEANUP_SECONDS)
        elif claimed:
            _release_store(account)
        raise


async def secret_manager(account: str) -> SecretChatManager:
    """Return one fully started manager for the current client generation.

    Startup is single-flight and owned independently of its waiters. The final
    proof is repeated after every initialization wait, including shared waits.
    """
    client = _telethon_client(account)
    _check_current(account, client)
    async with _lock:
        _check_current(account, client)
        existing = _by_account.get(account)
        if (
            existing is not None
            and _verified_against.get(account) is client
            and id(existing) not in _stops
            and not getattr(existing, "_stopping", False)
            and getattr(existing, "_running", True)
        ):
            return existing
        task = _starts.get(account)
        if task is None:
            task = asyncio.create_task(_start_manager(account, client))
            _starts[account] = task

            def finished(done):
                if _starts.get(account) is done:
                    _starts.pop(account, None)
                if not done.cancelled():
                    error = done.exception()
                    if error is not None:
                        log_event(logging.WARNING, "secret manager startup refused", error=error)

            task.add_done_callback(finished)
    manager = await asyncio.shield(task)
    _check_current(account, client)
    if _verified_against.get(account) is not client or _by_account.get(account) is not manager:
        raise SecretChatUnavailable(account, "the manager generation changed; retry the call")
    return manager


def _accept_incoming(manager: SecretChatManager, account: str):
    """Answer an incoming secret-chat request, because no tool can.

    The previous backend completed the handshake inside itself: an invitation arrived
    and became a usable chat with nothing asked of this server. The package hands the
    decision back instead, which is the better design for a library and a CAPABILITY
    LOSS here - the published tool surface is frozen, so there is no
    `accept_secret_chat` for a caller to reach for, and an unanswered request would sit
    at `pending` until it expired with every tool correctly refusing to send into it.

    Accepting restores exactly what the operator had. It is also what the account's
    other clients do, and it commits nothing: a chat that is accepted and never used
    costs one key, and `close_secret_chat` ends it.

    A failure here is logged and dropped rather than raised. This runs inside the
    package's own update dispatch, where an exception would take down the subscription
    that every OTHER chat on this account also depends on, to punish one bad
    invitation.
    """

    async def _handler(event):
        try:
            await manager.accept(event.chat_id)
        except Exception as error:
            log_event(
                logging.WARNING,
                "could not accept an incoming secret chat; it stays pending",
                account=account,
                chat_id=getattr(event, "chat_id", None),
                error=error,
            )

    return _handler


def _remember(account: str):
    """Write every arrived message into this server's own durable history.

    The package keeps arrivals in memory, which dies with the process, and keeps no
    record of what this side SENT because nothing arrives for it. `read_secret_messages`
    published both directions across restarts for the whole life of the previous
    backend, so :mod:`telegram_mcp.secret_history` holds them and this is where the
    incoming half is caught - once, at the seam, rather than at each reading tool.

    Failures are logged, never raised: this runs inside the package's update dispatch,
    and a full disk must not tear down the subscription that decrypts every other chat.
    """

    async def _handler(event):
        try:
            secret_history.record_received(account, event)
        except Exception as error:
            log_event(
                logging.WARNING,
                "could not record a received secret message",
                account=account,
                error=error,
            )
        try:
            # The file's key, kept so the file outlives this process (owner's
            # decision, 2026-09-29; see secret_media_refs).
            reference = MediaReference.from_message(event)
            if reference is not None:
                secret_media_refs.remember(
                    account,
                    event.chat_id,
                    event.random_id,
                    reference.to_dict(),
                    getattr(event, "ttl", 0) or 0,
                )
        except Exception as error:
            log_event(
                logging.WARNING,
                "could not keep a received secret file's reference",
                account=account,
                error=error,
            )

    return _handler


def _closed(account: str):
    """Drop closed-chat keys, and honor a peer's history-deleted event."""

    async def _handler(event):
        if getattr(event, "history_deleted", False):
            erase_local(account, event.chat_id)
            return
        try:
            secret_media_refs.drop_chat(account, event.chat_id)
        except Exception as error:
            log_event(
                logging.WARNING,
                "could not drop a closed secret chat's file references",
                account=account,
                error=error,
            )

    return _handler


def erase_local(account: str, chat_id: int, message_ids=None) -> dict:
    """Try BOTH stores and report partial cleanup without misreporting a send.

    These are two atomic files, not one transaction. Failure in one never skips
    the other, and callers may only claim complete local erasure if both agree.
    """
    ids = None if message_ids is None else tuple(message_ids)
    result = {"history": False, "media": False, "messages_removed_here": None}
    try:
        result["messages_removed_here"] = (
            secret_history.clear(account, chat_id)
            if ids is None
            else secret_history.forget(account, chat_id, ids)
        )
        result["history"] = True
    except Exception as error:
        log_event(logging.ERROR, "could not erase secret message history", error=error)
    try:
        if ids is None:
            secret_media_refs.drop_chat(account, chat_id)
        else:
            secret_media_refs.forget(account, chat_id, ids)
        result["media"] = True
    except Exception as error:
        log_event(logging.ERROR, "could not erase secret message file references", error=error)
    return result


def _service_action(account: str):
    """The library clears its memory; the application must clear its own files."""

    async def handler(event):
        action = event.action
        if isinstance(action, secret_tl.DecryptedMessageActionDeleteMessages):
            erase_local(account, event.chat_id, action.random_ids)
        elif isinstance(action, secret_tl.DecryptedMessageActionFlushHistory):
            erase_local(account, event.chat_id)

    return handler


def _withdrawn(account: str):
    """Log a message Telegram rejected for good; the package already withdrew it."""

    async def _handler(event):
        log_event(
            logging.WARNING,
            "a sent secret message was rejected by Telegram and withdrawn",
            account=account,
            chat_id=getattr(event, "chat_id", None),
            message_id=getattr(event, "random_id", None),
            cause=getattr(event, "cause", None),
        )

    return _handler


def media_source(manager, account: str, chat_id: int, message_id: int):
    """``(source, ttl)`` for a received message: the live one, else its stored file.

    The live message wins while this process still holds it; after a restart the
    stored ``MediaReference`` stands in. ``(None, 0)`` when neither exists.
    """
    for message in reversed(manager.read_history(chat_id, 10_000)):
        if message.random_id == int(message_id):
            return message, int(getattr(message, "ttl", 0) or 0)
    stored = secret_media_refs.load(account, chat_id, message_id)
    if stored is None:
        return None, 0
    return MediaReference.from_dict(stored["reference"]), int(stored.get("ttl") or 0)


async def _stop(manager: SecretChatManager) -> None:
    """Stop one manager, letting a failure surface rather than be swallowed.

    `stop()` flushes each chat. A swallowed failure here is lost key material, which
    no restart brings back, so it is reported rather than absorbed.
    """
    await manager.stop()


async def close_all(budget: float = _CLOSE_SECONDS) -> List[Tuple[str, BaseException]]:
    """Request every closure, retain failed owners, and spend one total budget.

    Cancellation/timeout of this observer never cancels the owned stop tasks.
    Repeated calls retry failed stops and share those still in progress.
    """
    global _closing
    _closing = True
    pending = set()
    for task in list(_starts.values()):
        if not task.done():
            task.cancel()
            pending.add(task)
    attempts = {}
    for account, manager in list(_by_account.items()):
        # A cancelled initializer owns cleanup of its partial subscription; a
        # concurrent native stop/start would itself corrupt lifecycle state.
        if account in _starts and not _starts[account].done():
            continue
        task = _stop_owned(account, manager)
        attempts[account] = task
        pending.add(task)
    pending.update(t for t in _acquisitions.values() if not t.done())
    # A stop already in flight (an old manager still flushing) is waited for too:
    # returning before it finishes lets the runner disconnect under it (review).
    pending.update(t for t in _stops.values() if not t.done())
    if pending:
        await asyncio.wait(pending, timeout=max(0.0, budget))
    failures = []
    accounts = set(_by_account) | set(_starts) | set(_acquisitions)
    for account in sorted(accounts):
        task = attempts.get(account)
        error = None
        if task is not None and task.done() and not task.cancelled():
            error = task.exception()
        if account in _by_account or account in _store_locks or account in _acquisitions:
            failures.append(
                (account, error or TimeoutError("secret backend closure is unconfirmed"))
            )
    return failures
