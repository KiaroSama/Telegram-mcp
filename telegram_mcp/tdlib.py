"""Secret chats, which Telethon cannot do at all.

Telethon never implemented MTProto 2.0 end-to-end encryption. The raw TL
requests are in the schema -- ``messages.requestEncryption``,
``messages.sendEncrypted`` and the rest -- but nothing drives them: there is no
Diffie-Hellman exchange, no key store, no secret-chat layer negotiation, and no
``create_secret_chat`` on the client. The project was archived in February 2026
without one, so this is not a gap that will close. Writing the exchange and the
message encryption by hand is not something a tool server should own.

So secret chats come from Telegram's own code instead: TDLib, the library the
official clients are built on, driven through its JSON interface. It performs
the cryptography itself, and it is current -- 1.8.67 speaks layer 229, where the
installed Telethon speaks 227.

Two costs, stated here because neither can be hidden from whoever runs this:

* TDLib cannot read a Telethon session file. There is no import path for an
  existing authorisation. An account used for secret chats logs in once more
  through ``scripts/secret_chat_login.py``, and that login is a new device in
  the account's session list.
* The binary is an optional dependency. Without it every other tool in this
  server keeps working and only the secret-chat tools report why they cannot.

Everything else in the server stays on Telethon. This module exists to run
secret chats and nothing more.
"""

from __future__ import annotations

import asyncio
import itertools
import json
import logging
import time
from pathlib import Path
from typing import Any, Optional

from telegram_mcp.safe_log import log_event
from telegram_mcp.settings import TELEGRAM_API_HASH, TELEGRAM_API_ID, state_dir

# The process-global half - the shared binary handle and the single receive
# thread that routes events back to these clients. Imported by name so the
# call sites below read the same as they always did, and so a test that
# patches `tdlib._tdjson` still reaches the client that uses it.
from telegram_mcp.tdlib_runtime import (
    TDLibUnavailable,
    _clients,
    _clients_lock,
    _ensure_reader,
    _quieten,
    _tdjson,
    stop_reader,
    tdjson_status,
)

__all__ = [
    "complete_login",
    "NotSignedIn",
    "account_label",
    "authorise_from_telethon",
    "TDLibClient",
    "TDLibError",
    "TDLibUnavailable",
    "close_all",
    "database_dir_for",
    "secret_client",
    "tdjson_status",
]


class NotSignedIn(RuntimeError):
    """The account has a Telethon session but no TDLib login.

    Its own type because the fix is specific and nothing else in the server
    needs it: this is not a permission problem, a network problem, or a wrong
    account name, and answering with any of those would send the caller looking
    in the wrong place.
    """

    def __init__(self, account: str, state: Optional[str]):
        super().__init__(
            f"Account {account!r} is signed in to Telethon but not to Telegram's "
            f"secret-chat library (state: {state}). TDLib keeps its own "
            f"authorisation and cannot import a Telethon session - but this account's "
            f"existing login can authorise it, so this asks for no code and nothing to "
            f"scan. Run once:"
            + chr(10)
            + "    Manage-Accounts.ps1 -> option 2, adding this same account again"
            + chr(10)
            + f"    or, without a fresh scan: python scripts/secret_chat_login.py {account}"
        )
        self.account = account
        self.state = state


# How long one client gets to reach `authorizationStateClosed`. Generous: what
# is being waited for is a database checkpoint whose loss takes secret-chat
# history with it. Bounded all the same - shutdown cannot hang on it.
_CLOSE_TIMEOUT = 20.0


class TDLibError(RuntimeError):
    """An `error` object from TDLib, carrying Telegram's own code and text."""

    def __init__(self, code: int, message: str):
        super().__init__(f"{message} (TDLib code {code})")
        self.code = code
        self.message = message


def account_label(account: Optional[str]) -> str:
    """The account label a TDLib database is stored under.

    `get_client` resolves `None` to the sole client but hands back the client,
    not its name, and TDLib is addressed by name. Lives here rather than in a
    tool module because this is the module that turns a label into a database
    directory, and two copies of the rule would be two chances to disagree.

    The import is deferred: `connection` builds real clients at import time, and
    this module must stay importable without them.
    """
    from telegram_mcp.connection import clients

    if account is None:
        if len(clients) == 1:
            return next(iter(clients))
        raise ValueError(f"Account is required. Available accounts: {', '.join(clients)}")
    label = account.lower()
    if label not in clients:
        raise ValueError(f"Unknown account '{account}'. Available accounts: {', '.join(clients)}")
    return label


def database_dir_for(account: str) -> Path:
    """Where one account's TDLib database lives.

    Under `state_dir()` with the Telethon sessions rather than beside the code:
    the install directory may be read-only and is often a git checkout, and one
    place to lock down beats two.
    """
    return state_dir() / "tdlib" / account


# Request correlation: TDLib echoes `@extra` back, so a client can have
# several requests in flight and still match each answer to its future.
_extra_ids = itertools.count(1)


class TDLibClient:
    """One account's TDLib client.

    Requests are correlated by the `@extra` field TDLib echoes back, so several
    can be in flight at once. Updates -- which arrive unsolicited and carry no
    `@extra` -- go to a queue instead of a future.
    """

    def __init__(self, account: str, database_dir: Optional[Path] = None):
        self.account = account
        self.database_dir = Path(database_dir) if database_dir else database_dir_for(account)
        self.authorization_state: Optional[str] = None
        # `authorizationStateWaitOtherDeviceConfirmation` carries a `tg://login`
        # link and nothing else. Keeping only the state name would throw away the
        # one field that state exists to deliver.
        self.authorization_link: Optional[str] = None
        self.updates: asyncio.Queue = asyncio.Queue(maxsize=1000)
        self._td = _tdjson()
        self._client_id: Optional[int] = None
        # Set when TDLib reports `authorizationStateClosed`, which is the
        # documented completion signal for `close` - the request's own reply
        # only says it arrived. Created lazily: there may be no loop yet.
        self._closed: Optional[asyncio.Event] = None
        self._pending: dict[str, asyncio.Future] = {}
        self._loop: Optional[asyncio.AbstractEventLoop] = None
        self._state_changed: Optional[asyncio.Event] = None

    # -- lifecycle ---------------------------------------------------------

    async def start(self) -> str:
        """Bring the client up and return the authorisation state it settles in.

        `authorizationStateReady` means a previous login is still good.
        Anything else is a step `scripts/secret_chat_login.py` has to complete;
        this returns it rather than prompting, because a tool server has nowhere
        to prompt.
        """
        _quieten(self._td)
        self._loop = asyncio.get_running_loop()
        self._state_changed = asyncio.Event()
        self._client_id = self._td.td_create_client_id()
        with _clients_lock:
            _clients[self._client_id] = self
        _ensure_reader()

        # Nothing happens until the client is poked; TDLib answers the first
        # request with its authorisation state.
        self._send({"@type": "getOption", "name": "version"})
        return await self._settle()

    async def _settle(self, timeout: float = 30.0, ignore: frozenset = frozenset()) -> str:
        """Wait for an authorisation state that needs someone else to act.

        The intermediate states pass by on their own -- `WaitTdlibParameters` is
        answered here, `Ready` is the end -- so waiting for "not changing any
        more" would either hang or return a state that is about to be replaced.

        `ignore` is for waiting to LEAVE a state that is otherwise settled. It
        exists because this returns the current state immediately when that
        state already qualifies: after a login token is accepted the client sits
        in `WaitOtherDeviceConfirmation`, which counts as settled, so a plain
        call answers "still waiting" instantly instead of waiting for Telegram
        to push the acceptance through. Measured, not theorised - that is
        exactly how the first live run failed.
        """
        settled = {
            "authorizationStateReady",
            "authorizationStateWaitPhoneNumber",
            "authorizationStateWaitCode",
            "authorizationStateWaitPassword",
            "authorizationStateWaitEmailAddress",
            "authorizationStateWaitEmailCode",
            "authorizationStateWaitRegistration",
            "authorizationStateWaitOtherDeviceConfirmation",
            "authorizationStateClosed",
        } - set(ignore)
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            if self.authorization_state in settled:
                return self.authorization_state
            remaining = deadline - asyncio.get_running_loop().time()
            if remaining <= 0:
                raise TimeoutError(
                    f"TDLib did not reach a usable authorisation state within {timeout:.0f}s "
                    f"(last state: {self.authorization_state})"
                )
            self._state_changed.clear()
            try:
                await asyncio.wait_for(self._state_changed.wait(), timeout=remaining)
            except asyncio.TimeoutError:
                continue

    async def close(self, timeout: float = _CLOSE_TIMEOUT) -> None:
        """Shut the client down and WAIT until TDLib says it is closed.

        Skipping this risks losing the secret-chat keys written since the last
        flush, which cannot be re-derived -- the messages they decrypt are gone
        with them. Which is why waiting for the right signal matters: the answer
        to the `close` REQUEST is an acknowledgement that the request arrived.
        TDLib's own documentation names `authorizationStateClosed` as the moment
        the work is finished, and that is what this waits for.

        The old version took the request's `ok` for completion, suppressed every
        failure and unregistered on the spot - so a close that had not happened
        was reported as one, the dispatch entry went while the native client was
        still checkpointing, and the caller went on to move the database aside.

        Raises on timeout or refusal, and keeps its registration when it does:
        the caller has to be able to tell "closed" from "asked to close".
        """
        if self._client_id is None:
            return
        closed = self._closed_event()
        # ONE monotonic deadline over both halves. Spending `timeout` on the
        # request and then `timeout` again on the Closed event meant a caller
        # that asked for ten seconds could wait twenty - so a shutdown budget
        # bought half of what it said, and the accounts behind this one lost
        # their turn.
        deadline = time.monotonic() + timeout
        try:
            await self.request({"@type": "close"}, timeout=max(deadline - time.monotonic(), 0.0))
        except (TDLibError, TimeoutError):
            # The request itself failed. TDLib may still be closing, so the
            # wait below is still the question worth asking.
            pass
        try:
            await asyncio.wait_for(closed.wait(), timeout=max(deadline - time.monotonic(), 0.0))
        except (asyncio.TimeoutError, TimeoutError) as error:
            # NOT unregistered. A client whose close cannot be confirmed is
            # still holding its database, and dropping the handle here is how
            # the next caller came to start a second one against it.
            raise TimeoutError(
                f"TDLib did not reach authorizationStateClosed within {timeout:.0f}s "
                f"for account {self.account!r}; its database is still in use."
            ) from error
        with _clients_lock:
            _clients.pop(self._client_id, None)
        self._client_id = None
        self._settle_pending(
            RuntimeError(f"the TDLib client for {self.account!r} closed while this was in flight")
        )

    def _closed_event(self) -> "asyncio.Event":
        """The event `_on_authorization` sets when TDLib reports Closed."""
        if self._closed is None:
            self._closed = asyncio.Event()
        if self.authorization_state == "authorizationStateClosed":
            self._closed.set()
        return self._closed

    def _settle_pending(self, error: BaseException) -> None:
        """Fail every request still waiting, rather than leaving it forever.

        A closed client can never answer, so a pending future is a caller that
        waits out its own timeout for a reply that was never coming.
        """
        for future in list(self._pending.values()):
            if not future.done():
                future.set_exception(error)
        self._pending.clear()

    # -- request / response -----------------------------------------------

    def _send(self, obj: dict) -> None:
        self._td.td_send(self._client_id, json.dumps(obj).encode())

    async def request(self, obj: dict, timeout: float = 30.0) -> dict:
        """Send one request and wait for its answer.

        Raises `TDLibError` when TDLib answers with an `error`, so a caller
        never has to check whether a dict is a result or a failure.
        """
        if self._client_id is None:
            raise RuntimeError("TDLib client is not started")
        extra = str(next(_extra_ids))
        future = self._loop.create_future()
        self._pending[extra] = future
        try:
            # INSIDE the try. `_send` serialises to JSON and calls into the
            # native library, and either can raise - an unencodable argument, a
            # library error - at which point the entry registered a line above
            # was orphaned for the life of a client that lives as long as the
            # server, because only the await was ever guarded.
            self._send({**obj, "@extra": extra})
            result = await asyncio.wait_for(future, timeout=timeout)
        except asyncio.TimeoutError:
            raise TimeoutError(f"TDLib did not answer {obj['@type']} within {timeout:.0f}s")
        finally:
            # `finally`, not the timeout branch alone. `_handle_on_loop` removes an
            # entry only when a reply arrives, and for a call nobody is waiting for
            # one never does - so a CANCELLED request (an MCP client that hung up,
            # a sibling in a gather that failed) left its future here for the life
            # of a client that lives as long as the server. Same leak the timeout
            # branch was already written to prevent, through the other door.
            abandoned = self._pending.pop(extra, None)
            # A future nobody will ever resolve: the send failed, so no reply
            # carrying this `@extra` is coming. Left pending it warns at
            # interpreter exit and hides the real error behind the noise.
            if abandoned is not None and not abandoned.done():
                abandoned.cancel()
        if result.get("@type") == "error":
            raise TDLibError(result.get("code", 0), result.get("message", "unknown error"))
        return result

    # -- inbound -----------------------------------------------------------

    def _handle(self, event: dict) -> None:
        """Called on the reader thread. Everything it touches is hopped onto the
        client's own event loop, because futures and queues are not thread-safe.
        """
        loop = self._loop
        if loop is None or loop.is_closed():
            return
        loop.call_soon_threadsafe(self._handle_on_loop, event)

    def _handle_on_loop(self, event: dict) -> None:
        extra = event.get("@extra")
        if extra is not None:
            future = self._pending.pop(extra, None)
            if future is not None and not future.done():
                future.set_result(event)
            return

        if event.get("@type") == "updateAuthorizationState":
            self._on_authorization(event["authorization_state"])
            return

        try:
            self.updates.put_nowait(event)
        except asyncio.QueueFull:
            # Dropping the oldest keeps the newest, which is what a caller
            # polling for "did my message arrive" actually wants.
            try:
                self.updates.get_nowait()
                self.updates.put_nowait(event)
            except (asyncio.QueueEmpty, asyncio.QueueFull):  # pragma: no cover
                pass

    def _on_authorization(self, state: dict) -> None:
        self.authorization_state = state["@type"]
        self.authorization_link = state.get("link")
        if self.authorization_state == "authorizationStateWaitTdlibParameters":
            self._send(self._parameters())
        if self.authorization_state == "authorizationStateClosed":
            # The completion signal, per TDLib's own documentation. `close`
            # answering `ok` only says the request arrived.
            if self._closed is not None:
                self._closed.set()
            # Closed can arrive UNSOLICITED - the session terminated from another
            # device, or TDLib closing itself after an error - and a client that
            # is closed can never answer. Every request still in flight used to
            # sit until its own timeout expired, one by one, reporting a network
            # stall for something that had already ended.
            self._settle_pending(
                RuntimeError(
                    f"the TDLib client for {self.account!r} reported it closed while this "
                    "request was in flight"
                )
            )
        if self._state_changed is not None:
            self._state_changed.set()

    def _parameters(self) -> dict:
        self.database_dir.mkdir(parents=True, exist_ok=True)
        return {
            "@type": "setTdlibParameters",
            "use_test_dc": False,
            "database_directory": str(self.database_dir),
            "files_directory": str(self.database_dir / "files"),
            "database_encryption_key": "",
            "use_file_database": True,
            "use_chat_info_database": True,
            "use_message_database": True,
            # The whole point. TDLib will not accept or create an encrypted chat
            # with this off, and the failure is a silent absence of updates
            # rather than an error.
            "use_secret_chats": True,
            "api_id": TELEGRAM_API_ID,
            "api_hash": TELEGRAM_API_HASH,
            "system_language_code": "en",
            "device_model": "telegram-mcp",
            "system_version": "",
            "application_version": "1.0",
        }

    # -- convenience -------------------------------------------------------

    async def drain_updates(self, of_type: Optional[set] = None) -> list[dict[str, Any]]:
        """Everything queued since the last drain, oldest first.

        Non-blocking on purpose: a tool call has to return, and "nothing new"
        is a real answer.
        """
        out = []
        while True:
            try:
                event = self.updates.get_nowait()
            except asyncio.QueueEmpty:
                return out
            if of_type is None or event.get("@type") in of_type:
                out.append(event)


# --------------------------------------------------------------------------
# One started client per account, because starting one is expensive: it opens a
# database, reconnects, and re-fetches state. A tool call must not pay that.
# --------------------------------------------------------------------------

# --------------------------------------------------------------------------
# Authorising TDLib from the Telethon login that already exists.
#
# This is the difference between "one more code" and none at all. Telegram's
# QR-login flow lets a NEW client publish a login token and an ALREADY
# AUTHORISED client accept it - that is how the official desktop app links a
# device. Both halves are reachable here: TDLib asks with
# `requestQrCodeAuthentication` and answers with a `tg://login?token=...` link,
# and Telethon accepts it with `auth.acceptLoginToken`.
#
# So an account already signed in to Telethon can authorise TDLib without the
# person entering anything. It is still a second device on the account -- that
# part is the protocol and cannot be removed -- but it is no longer a second
# code.
# --------------------------------------------------------------------------


# The account -> client registry moved next door: which client is alive and
# whether it can still serve is a lifecycle question, not a protocol one.
# Imported BELOW the class rather than at the top, because that module imports
# `TDLibClient` from here - the name has to exist before it is loaded.
from telegram_mcp.tdlib_registry import (  # noqa: E402,F401  (re-exported)
    _by_account,
    _by_account_lock,
    close_all,
    secret_client,
)


def login_token(link: str) -> bytes:
    """The raw token inside a `tg://login?token=...` link.

    Telegram encodes it base64url WITHOUT padding, which `b64decode` rejects,
    so the padding is restored rather than the error being caught and guessed
    at.
    """
    import base64
    from urllib.parse import parse_qs, urlparse

    values = parse_qs(urlparse(link).query).get("token") or []
    if not values:
        raise ValueError(f"No login token in {link!r}")
    encoded = values[0]
    return base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4))


async def authorise_from_telethon(client: "TDLibClient", telethon_client) -> str:
    """Sign TDLib in using the account's existing Telethon authorisation.

    Returns the authorisation state it reaches. `authorizationStateReady` is a
    complete success. `authorizationStateWaitPassword` means the account has
    two-step verification and Telegram wants it even for a linked device -- the
    caller has to collect that, and it is a password rather than a code.

    Raises `TDLibError` if Telegram refuses the token, which is the honest
    outcome to report: the fallback is the ordinary phone-and-code login.
    """
    from telethon.tl import functions

    if client.authorization_state != "authorizationStateWaitPhoneNumber":
        raise RuntimeError(
            "TDLib is not waiting for a login "
            f"(state: {client.authorization_state}); nothing to authorise."
        )

    # `other_user_ids` is for adding an account beside ones already signed in to
    # THIS TDLib database. There are none: one database per account here.
    await client.request({"@type": "requestQrCodeAuthentication", "other_user_ids": []})
    state = await client._settle()
    if state != "authorizationStateWaitOtherDeviceConfirmation":
        raise RuntimeError(f"TDLib did not offer a login token (state: {state}).")

    token = login_token(client.authorization_link or "")
    await telethon_client(functions.auth.AcceptLoginTokenRequest(token=token))

    # Telegram pushes the acceptance through as a NEW authorisation state, so
    # the wait has to be for leaving this one rather than for reaching any
    # settled one - it is already in a settled one.
    return await client._settle(
        ignore=frozenset({"authorizationStateWaitOtherDeviceConfirmation"})
    )


async def complete_login(label: str, telethon_client, password=None, ask_password=None) -> str:
    """Take an account's TDLib half from wherever it is to signed in.

    ``label`` is already resolved -- this deliberately does NOT call
    ``account_label``. That would import ``telegram_mcp.connection``, which reads
    the accounts once at import time, and the session generator calls this
    moments after writing a NEW account to ``.env``: the environment was read
    before that line existed, so the freshly created account looked unconfigured
    and the whole step died with "No Telegram session configured" on an account
    that had just been saved successfully. The caller knows its own label; asking
    a cached view to confirm it was the bug.

    One implementation for both callers, because they differ only in where the
    two-step password comes from:

    * ``session_string_generator.py`` has just watched the owner type it for the
      Telethon login, so it passes it straight through and the owner is asked
      **nothing**. Asking a second time for a password Telegram already accepted
      seconds earlier is not caution, it is a defect -- and every extra attempt
      is one more against the account's own limits.
    * ``scripts/secret_chat_login.py`` is repairing an account signed in long
      ago and has no password to reuse, so it supplies ``ask_password``. It
      resolves the label through ``account_label`` first, where a typed name
      genuinely does need checking.

    Starts from the client's CURRENT state rather than assuming a fresh one: a
    database left at ``WaitPassword`` by an interrupted run must not publish a
    second login token, which is what re-running the old flow did.

    Returns the state reached. ``authorizationStateReady`` is the only success.
    The password is never logged, stored, or passed on a command line.
    """
    from telegram_mcp import tdlib_identity as identity

    try:
        return await _attempt_login(label, telethon_client, password, ask_password)
    except identity.IdentityMismatch:
        # Two real accounts and no way to tell which was meant. Never recovered
        # from automatically: the database is the evidence, and the old recovery
        # path would have deleted it.
        raise
    except (TDLibError, RuntimeError) as error:
        if not _authorisation_is_dead(error):
            raise
        # The database holds an authorisation Telegram no longer honours. That
        # happens every time an account is removed and added again: the TDLib
        # database outlives `.env`, so the new session inherits the old one's
        # dead auth key and every attempt fails with AUTH_KEY_UNREGISTERED --
        # which no amount of logging in again can fix, and each attempt costs a
        # real login.
        #
        # Moved aside, not deleted. The bytes may hold secret-chat keys that
        # cannot be re-derived, and the diagnosis above is a guess about
        # Telegram's answer rather than a fact about the file. `rmtree` with
        # `ignore_errors=True` was worse than either: a directory still held
        # open reported success while deleting nothing at all.
        kept_at = identity.quarantine_database(
            label, why=str(error), closed=_close_confirmed.get(label, False)
        )
        try:
            # ONE retry. A dead authorisation that survives a fresh database is
            # not a stale database, and quarantining again would spend another
            # real login to learn the same thing.
            return await _attempt_login(label, telethon_client, password, ask_password)
        except (TDLibError, RuntimeError) as second:
            if _authorisation_is_dead(second):
                raise RuntimeError(
                    f"Telegram still refuses the authorisation for account '{label}' after a "
                    f"fresh database was started. The previous one was not deleted - it is at "
                    f"{kept_at}. Sign the account in again from the Telegram app, then retry; "
                    "nothing here will keep requesting authorisations on its own."
                ) from second
            raise


def _authorisation_is_dead(error: Exception) -> bool:
    """Whether the stored authorisation is unusable rather than merely refused.

    Matched on the specific names, not on the 401 code: a client that has simply
    not signed in yet also answers 401, and wiping its database for that would
    destroy a login in progress.
    """
    text = str(error).upper()
    return any(
        name in text for name in ("AUTH_KEY_UNREGISTERED", "SESSION_REVOKED", "SESSION_EXPIRED")
    )


# label -> whether the last `_attempt_login` saw its client reach Closed. Read
# by the recovery path, which may not move a database aside on any weaker
# evidence: a rename succeeding is a Windows accident, not a closure check.
_close_confirmed: dict = {}


async def _closed_cleanly(client) -> bool:
    try:
        await client.close()
    except Exception as error:
        log_event(
            logging.WARNING,
            "a TDLib client did not confirm it closed",
            error=error,
        )
        return False
    return True


async def _attempt_login(label, telethon_client, password, ask_password) -> str:
    from telegram_mcp import tdlib_identity as identity

    client = TDLibClient(label)
    try:
        state = await client.start()
        if state == "authorizationStateReady":
            # Before the caller does ANYTHING with this client. A ready database
            # under a reused label is signed in as the previous owner, and every
            # call made through it would run as them.
            await identity.verify_owner(label, client, telethon_client)
            return state

        if state == "authorizationStateWaitPhoneNumber":
            state = await authorise_from_telethon(client, telethon_client)

        if state == "authorizationStateWaitPassword":
            secret = (
                password if password is not None else (ask_password() if ask_password else None)
            )
            if not secret:
                return state
            await client.request({"@type": "checkAuthenticationPassword", "password": secret})
            state = await client._settle()

        if state == "authorizationStateReady":
            # The other way a reused label signs in as the wrong person: a
            # database an interrupted run left at WaitPassword belongs to
            # WHOEVER started that run, and finishing it here completes THEIR
            # login. Checked on every path that reaches Ready, not only the one
            # that was already there.
            await identity.verify_owner(label, client, telethon_client)
        return state
    finally:
        # Recorded rather than swallowed. The recovery path below may want to
        # move this database aside, and it may only do that once something has
        # CONFIRMED the client let go of it. A close that raised - and now it
        # can, because it waits for the documented completion signal - must not
        # mask the failure being reported either.
        _close_confirmed[label] = await _closed_cleanly(client)
