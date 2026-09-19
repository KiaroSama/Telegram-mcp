"""Getting a connected Telethon client for an account, and keeping it connected.

One question, one module: given an account label - or none, in single-account mode -
hand back a client that is logged in, routed through whatever proxy the operator
configured, and actually reachable right now.

The pieces are here because they only make sense together. A session string names an
account; a session POOL exists because Telethon's `StringSession` holds no persistent
entity cache, so several concurrent clients need distinct slots rather than one shared
one. `@with_account` is the router that turns a tool's `account=` argument into a
client. `ensure_connected` is the part that distinguishes "the socket is open" from
"the server answers", which are not the same thing and only the second one matters.

**Patch this module, not `runtime`.** `runtime` re-exports these names for the star
imports every tool module uses, so rebinding `runtime._build_client` in a test sets a
second name and the code here keeps calling its own.
"""

import asyncio
import json
import logging
import os
import sys
from functools import wraps
from typing import Any, Optional

from telethon import TelegramClient
from telethon.sessions import StringSession

from telegram_mcp.aliases import normalise_account_label, restrict_to_owner
from telegram_mcp.safe_log import log_event, logger, safe_exception
from telegram_mcp.settings import (
    StartupMessage,
    ValidationError,
)

# Where log records go, and what they may contain, now lives next door: it is a
# different job from reaching Telegram, and this file was carrying both. The
# names are re-exported because `safe_log` and several tests import them from
# here, and moving code should not move anyone's import.
from telegram_mcp.log_setup import (  # noqa: F401  (re-exported)
    LOG_BACKUP_COUNT,
    LOG_MAX_BYTES,
    RedactingFilter,
    _make_file_handler,
    _OwnerOnlyRotatingFileHandler,
    _secret_env_values,
    console_handler,
    log_file_path,
    redact,
)

# Which of the pooled sessions this process claims, and the advisory locks that
# keep two clients off the same slot. Imported by name so `_build_accounts`
# below still resolves `_acquire_session` as a module global - which is what a
# test that substitutes it relies on.
from telegram_mcp.session_pool import _acquire_session, _parse_session_pool

# What a client must satisfy before this process serves from it. Startup had
# these checks and a reload did not, so the same server enforced two different
# rules depending on when an account appeared.
from telegram_mcp import admission as _admission
from telegram_mcp import account_lifecycle as _lifecycle

# Whether the socket in front of you still works, and bringing it back when it
# does not. A different question from which accounts exist, so it lives next
# door; re-exported because `__all__` publishes these and `runtime` star-imports
# this module.
from telegram_mcp.reconnect import (  # noqa: F401  (re-exported)
    _BURNED_SESSION_MESSAGE,
    _CONN_VERIFY_INTERVAL,
    _RECONNECT_LOCKS,
    _RECONNECT_TIMEOUT,
    _force_reconnect,
    _last_conn_verified,
    ensure_connected,
)

# What the configuration says, as opposed to what is connected. Pure file and
# environment reading, so it moved to its own module; re-exported because
# `runtime` star-imports this one and the tests patch these names here.
from telegram_mcp.account_config import (  # noqa: F401  (re-exported)
    _ACCOUNT_PREFIXES,
    _account_digest,
    _account_digest_bytes,
    _accounts_from_disk,
    _current_digests,
    _env_file,
    _env_fingerprint,
)

# One reading of the configuration, so the clients built, the fingerprint
# recorded and the digests marked active cannot describe different revisions.
from telegram_mcp.account_snapshot import Snapshot, read_snapshot

# Retiring a client outlives the synchronous call that starts it, so it owns a
# module of its own. Re-exported: `__all__` publishes these and `runtime`
# star-imports this file.
from telegram_mcp.retirement import (  # noqa: F401  (re-exported)
    _RETIRE_DRAIN_SECONDS,
    _retiring,
    drain_retirements,
    retire as _retire,
)

# Proxy configuration moved next door: turning TELEGRAM_PROXY_* into Telethon
# kwargs never touches a socket or a session, and this file was carrying both
# jobs. Re-exported for the same reason log_setup's names are - `runtime` star
# imports this module and `runner` imports parse_port from it.
from telegram_mcp.proxy import (  # noqa: F401  (re-exported)
    _PROXY_TYPES_ALL,
    _PROXY_TYPES_SOCKS_HTTP,
    _build_proxy_for_label,
    _get_proxy_env,
    parse_port,
)

# Session files and client construction moved next door. Re-exported
# because `__all__` still publishes several of these and `runtime`
# star-imports this module.
from telegram_mcp.session_files import (  # noqa: F401  (re-exported)
    SessionNotProtected,
    _SESSION_SIDECARS,
    _UNPROTECTED_SESSION_MESSAGE,
    _build_client,
    _close_unprotected,
    adopt_legacy_session,
    harden_env_file,
    harden_session_files,
    package_dir,
    script_dir,
    session_file_path,
)

# The installation, for the two things that still resolve against it: a session
# file an older install left beside main.py, and the historic `script_dir` name
# the tools re-export.

# ---------------------------------------------------------------------------
# Multi-account configuration
# ---------------------------------------------------------------------------


# --- File-based sessions -----------------------------------------------------
#
# A `.session` file IS the account. It is a SQLite database holding the auth
# key, and whoever can read it is logged in as that account with no password
# and no second factor -- Telethon's own docstring says as much. It was being
# created wherever the process happened to start, with whatever the umask gave
# it (0644 on a normal host) and, on Windows, readable by every account on the
# machine.

# SQLite writes alongside the database it opens. A `-journal` holds pages of
# the same file mid-write, and `-wal`/`-shm` hold them for as long as the
# connection lives, so restricting only the `.session` restricts nothing while
# a write is in flight.


class NoAccountsConfigured(StartupMessage):
    """Nothing at all is configured - as distinct from something configured wrongly.

    The two need different answers. At startup there is no account to serve and
    the process says so and stops. During a reload it means the file on disk is
    momentarily unusable, and the right move is to keep serving the generation
    already running rather than to exit; a half-written `.env` is a window the
    account manager genuinely opens every time it rewrites one.

    A `StartupMessage`, so the runner's readable-error path still prints it
    word for word instead of a traceback.
    """


def _discover_accounts(
    env: Optional[dict] = None, reuse: Optional[dict] = None
) -> dict[str, TelegramClient]:
    """Scan env vars to build account label -> TelegramClient mapping.

    Detection rules:
    - TELEGRAM_SESSION_STRING_<LABEL> / TELEGRAM_SESSION_NAME_<LABEL> -> multi-mode
    - TELEGRAM_SESSION_STRINGS (whitespace/comma/semicolon separated) -> a pool
      of interchangeable sessions for the default account; each process claims a
      free slot to avoid AuthKeyDuplicatedError (takes precedence for "default")
    - Unsuffixed TELEGRAM_SESSION_STRING / TELEGRAM_SESSION_NAME -> label "default"
    - If both suffixed and unsuffixed exist -> unsuffixed becomes "default"

    Two variables that name the SAME label are a configuration error, not a
    precedence question: the old loop simply let the later one win, so which
    account the server ran as depended on the order ``os.environ`` iterated in.
    A label that normalises to nothing is refused for the same reason.

    Each client is constructed via :func:`_build_client`, which applies any
    matching ``TELEGRAM_PROXY_*`` configuration (optionally per-label).

    ``reuse`` maps a label to a client the caller already owns and intends to
    keep. Validation still covers the WHOLE set - a duplicate label is an error
    whether or not that account changed - but nothing is constructed for a label
    being kept. A reload used to build every client and then drop the unchanged
    ones on the floor unclosed, so a `.env` touched ten times leaked ten clients
    per untouched account, each with whatever file handles and locks its session
    had taken.
    """
    reuse = reuse or {}
    environment = os.environ if env is None else env
    accounts: dict[str, TelegramClient] = {}
    try:
        return _build_accounts(environment, reuse, accounts)
    except BaseException:
        # Every client THIS call constructed, disposed. A failure part-way -
        # an unusable label further down the list, or a pool with no free slot -
        # left the ones already built holding their session files open with
        # nothing referencing them, once per reload.
        for label, client in accounts.items():
            if reuse.get(label) is not client:
                _retire(client)
        raise


def _build_accounts(environment, reuse, accounts):
    """The construction itself. Separated so its failure has a single handler."""

    prefix_str = "TELEGRAM_SESSION_STRING_"
    prefix_name = "TELEGRAM_SESSION_NAME_"

    # Collect first, decide second: every conflict is then visible at once and
    # the answer cannot depend on iteration order.
    declared: dict[str, list[tuple[str, str, str]]] = {}
    for key, value in environment.items():
        if key.startswith(prefix_str) and value:
            suffix, kind = key[len(prefix_str) :], "string"
        elif key.startswith(prefix_name) and value:
            suffix, kind = key[len(prefix_name) :], "name"
        else:
            continue
        # The SAME rule the account manager and session generator apply when
        # they WRITE a label. Reading with a weaker one (a bare strip+lower) let
        # `TELEGRAM_SESSION_STRING_WORK-2` register as `work-2`, a name no tool
        # could ever produce - so the account existed and nothing could address
        # it. Canonicalising here also makes `WORK-2` and `WORK_2` the same
        # account, which is what the collision check below then refuses.
        try:
            label = normalise_account_label(suffix).lower()
        except ValueError as error:
            raise ValidationError(
                f"'{key}' does not name a usable account: {error} Use "
                f"'{prefix_str}<LABEL>' with a label, or the unsuffixed variable "
                "for the default account."
            ) from error
        declared.setdefault(label, []).append((key, kind, value))

    for label, sources in sorted(declared.items()):
        if len(sources) > 1:
            names = ", ".join(sorted(key for key, _, _ in sources))
            raise ValidationError(
                f"Account '{label}' is defined more than once ({names}). These "
                "resolve to one account after normalisation - spaces and hyphens "
                "both become underscores and case is folded - and which one wins "
                "would depend on environment order. Keep exactly one."
            )
        if label in reuse:
            accounts[label] = reuse[label]
            continue
        _key, kind, value = sources[0]
        session = StringSession(value) if kind == "string" else value
        accounts[label] = _build_client(session, label)

    # Backward-compatible unsuffixed variables. A pool (TELEGRAM_SESSION_STRINGS)
    # takes precedence for the default account and claims a free session slot.
    session_pool = _parse_session_pool(environment)
    session_string = environment.get("TELEGRAM_SESSION_STRING")
    session_name = environment.get("TELEGRAM_SESSION_NAME")

    if "default" not in accounts:
        if "default" in reuse:
            # Before the pool branch on purpose: claiming a slot for an account
            # that is not being rebuilt is how a rebuild took a second one.
            accounts["default"] = reuse["default"]
        elif session_pool:
            accounts["default"] = _build_client(
                StringSession(_acquire_session(session_pool)), "default"
            )
        elif session_string:
            accounts["default"] = _build_client(StringSession(session_string), "default")
        elif session_name:
            accounts["default"] = _build_client(session_name, "default")

    if not accounts:
        # RAISED, not `sys.exit`. `SystemExit` derives from `BaseException`, so
        # `refresh_accounts`'s `except Exception` never saw it: a `.env` caught
        # mid-rewrite - and the account manager backs up and rewrites, so that
        # window is real - took the whole running server down from inside a
        # routine hot-reload check. Startup still exits, just below; a reload
        # keeps the generation it already has.
        raise NoAccountsConfigured(
            "No Telegram session configured. "
            "Set TELEGRAM_SESSION_STRING or TELEGRAM_SESSION_STRING_<LABEL> in .env"
        )

    return accounts


# ONE read, and everything below is decided from it. Three separate reads let
# an edit that landed between them be recorded as the active revision with no
# client ever built for it - after which the next reload compared the new file
# against itself, found nothing to do, and the old client went on serving.
_boot: Snapshot = read_snapshot()

try:
    clients: dict[str, TelegramClient] = _discover_accounts(_boot.env)
except NoAccountsConfigured as _no_accounts:
    # Startup with nothing configured cannot proceed, and says so in one line
    # rather than a traceback - the behaviour this replaced, kept verbatim.
    print(f"Error: {_no_accounts}", file=sys.stderr)
    sys.exit(1)


# The revision this process has SEEN, and the revision that is ACTIVE. Both come
# from `_boot`, so they describe the configuration the clients above were built
# from and nothing else. They are separate names because a revision can be seen
# and refused: the stamp moves so a broken file is not re-parsed on every call,
# while the digests stay on the generation still serving.
_env_stamp: tuple = _boot.stamp
_env_digests: dict = _boot.digests

# The last revision that was read and refused, for the status tools to report. A
# reload that quietly did nothing is indistinguishable from one that worked.
# The refused-revision record moved to `account_lifecycle`, which owns the
# reload transaction and can be asked about it from anywhere. Re-exported
# here because that is where callers have always looked.
last_rejection = _lifecycle.last_rejection


# Told after every change to `clients`. A registry of callbacks rather than a
# direct call, because the modules that care - `tools.events` above all - reach
# this one through `runtime`'s star import, so the dependency runs one way only.
# Registering a callback is how the other direction gets expressed without an
# import cycle.
_registry_listeners: list = []


def on_clients_changed(callback) -> None:
    """Run ``callback(added, removed)`` after every change to ``clients``."""
    if callback not in _registry_listeners:
        _registry_listeners.append(callback)


def _notify_clients_changed(added: set, removed: set) -> None:
    if not (added or removed):
        return
    for callback in list(_registry_listeners):
        try:
            callback(added, removed)
        except Exception as error:
            log_event(
                logging.ERROR,
                "a client-registry listener failed",
                error=error,
                added=len(added),
                removed=len(removed),
            )


def refresh_accounts() -> list:
    """Pick up accounts added, removed or re-logged-in since startup.

    Adding an account used to need a server restart, and the failure when you
    forgot was not "unknown account" - it was `AuthKeyUnregisteredError` from
    the session this process was still holding, which reads like Telegram
    revoking the login rather than like stale state here. Re-logging in an
    existing account produced the same thing.

    Cost on the common path is reading a few kilobytes and hashing them. The
    file is only PARSED into clients, and clients only rebuilt, when that hash
    actually moves.

    Returns the labels that changed, so a caller can say what happened.
    """
    # `_env_digests` is no longer assigned here: `record_activated` owns it, so
    # that what is recorded as active is decided in one place.
    global _env_stamp

    # ONE read. Fingerprinting the file and then parsing it again is two
    # readings of something being rewritten underneath, and the pair could
    # describe different revisions - the stamp recording an edit whose accounts
    # were never built, after which the next call saw nothing left to do.
    try:
        snapshot = read_snapshot()
    except Exception as error:
        # A half-written `.env` - the account manager backs up and rewrites, so
        # there IS a window - must not take the running server down. The next
        # call reads again.
        #
        # RECORDED, not only swallowed. A revision the reader refuses - a
        # malformed line, undecodable bytes, a file that exists and cannot be
        # opened - now stops here rather than arriving as a partial
        # configuration, and an operator who edited the file has to be able to
        # ask why nothing happened. The stamp deliberately does NOT move: the
        # bytes were never understood, so there is nothing to mark as seen.
        _lifecycle.record_rejection(None, f"{type(error).__name__}: {error}")
        return []

    if snapshot.stamp == _env_stamp:
        return []
    stamp, env, digests = snapshot.stamp, snapshot.env, snapshot.digests

    if digests == _env_digests:
        # The file moved but no account did - a comment, an unrelated setting.
        _env_stamp = stamp
        return []

    keep = {label: client for label, client in clients.items() if not _replaced(label, digests)}
    rebuilt = None
    try:
        rebuilt = _discover_accounts(env, reuse=keep)
        # The same check startup runs, before anything is published: two labels
        # sharing one session is one auth key used twice, which Telegram answers
        # by invalidating it for both. A reload could publish exactly that.
        _admission.reject_duplicate_sessions(rebuilt)
    except Exception as error:
        # Every client this attempt CONSTRUCTED is closed before returning. The
        # duplicate check runs after construction, so a rejection here used to
        # leave real clients - sockets, and an open SQLite handle on a file
        # session - built and owned by nobody at all.
        if rebuilt:
            for label, client in rebuilt.items():
                if keep.get(label) is not client:
                    _lifecycle.dispose(label, client, "the reload was rejected")
        # A `.env` that no longer describes a valid account set - a duplicate
        # label, an unusable one, or none at all - leaves the WORKING clients in
        # place. Refusing to serve because a file on disk went wrong would be
        # worse than serving what already works. Said out loud, because a reload
        # that quietly did nothing is indistinguishable from one that worked.
        log_event(
            logging.WARNING,
            "account reload rejected; keeping the running accounts",
            error=error,
            accounts=len(clients),
        )
        # SEEN, not applied: the stamp moves so a broken file is not re-parsed on
        # every call, while the active digests stay on the generation still
        # serving. The rejection is recorded rather than only logged, because a
        # reload that quietly did nothing looks exactly like one that worked.
        _env_stamp = stamp
        _lifecycle.record_rejection(stamp, f"{type(error).__name__}: {error}")
        return []

    before = dict(clients)
    changed = sorted(set(rebuilt) ^ set(clients)) + sorted(
        label for label in set(rebuilt) & set(clients) if _replaced(label, digests)
    )
    for label in set(clients) - set(rebuilt):
        # The disconnect is handed to admission so the lease is released when the
        # socket is actually down. Releasing first is the window another process
        # needs to claim a session this one is still connected to.
        removed = clients.pop(label)
        _admission.forget(label, closing=_retire(removed), client=removed)
    # STAGED, not published. A replacement is only what its label means once it
    # holds the session lease, has connected and has proved the session is
    # authorized - all of which can fail, and all of which used to happen after
    # the working client had already been retired and replaced. Until then the
    # previous client keeps answering.
    staged = []
    for label, client in rebuilt.items():
        if label in clients and not _replaced(label, digests):
            continue
        previous = clients.get(label)
        if previous is None:
            # Nothing is serving this label, so there is nothing to protect:
            # publish it and let admission gate what is served FROM it.
            clients[label] = client
        staged.append(_lifecycle.Staged(label=label, client=client, previous=previous))

    # The STAMP moves now: this revision has been read, and re-parsing it on
    # every call would be work with no answer attached. The DIGESTS do not, past
    # the accounts that are not part of the transaction - they say what is
    # SERVING, and nothing staged is serving yet.
    _env_stamp = stamp
    # Everything except a REPLACEMENT. A replacement's digest waits because its
    # predecessor is the one still serving, and recording the candidate's is what
    # made a failed one look already applied. Everything else - untouched
    # accounts, and a brand-new one, which has no predecessor and was published
    # into the registry above - is serving now, and a digest left unrecorded
    # makes the next reload believe it changed and rebuild it.
    replacing = {one.label for one in staged if one.previous is not None}
    record_activated(clients, digests, set(clients) - replacing)
    _lifecycle.clear_rejection()

    # A pure label move first, and it is not a transaction: the SAME session and
    # the SAME client under a new name already hold the lease, and releasing and
    # re-taking it would be a gap another process could use for a change that
    # never touched the session. A fresh `claim_session` would also block on this
    # process's own lock.
    for entry in list(staged):
        moved = next(
            (
                old
                for old, lease in _admission._active.items()
                if lease.client is entry.client and old != entry.label
            ),
            None,
        )
        if moved is not None and _admission.transfer_lease(moved, entry.label, entry.client):
            clients[entry.label] = entry.client
            staged.remove(entry)

    # Identity, not label: a re-login keeps the label and replaces the object, and
    # a listener that only watched labels left the new client with no handler.
    added = {label: cl for label, cl in clients.items() if before.get(label) is not cl}
    for label in added:
        if label not in _admission.session_locks:
            # No lock was ever taken for this label, so there is nothing to release;
            # this only clears a stale pending entry. Named anyway, so it cannot
            # reach a lease that a different client took under the same name.
            _admission.forget(label, client=clients.get(label))
    still_pending = {
        label: client for label, client in added.items() if label not in _admission.session_locks
    }
    # Started NOW rather than on the first async API call, and it CONNECTS. A
    # server that only waits for incoming updates never makes an API call, so a
    # hot-added account used to sit published with a lease and no socket - which
    # is not an account that receives anything. The transaction takes the lease,
    # connects, proves the session is authorized, and only then swaps a
    # replacement in; a failure leaves the previous client serving.
    _admission.mark_awaiting_admission(still_pending)
    settling = _lifecycle.begin(staged, clients)
    if settling is not None:
        # Recorded when the transaction settles, from what REACHED the registry.
        # With no loop to settle on, nothing is recorded, which is the same safe
        # direction the staging itself takes.
        settling.add_done_callback(
            lambda _t: record_activated(clients, digests, _lifecycle.activated_labels())
        )
    _notify_clients_changed(set(added), set(before) - set(clients))
    return sorted(set(changed))


def _account_label_of(key: str) -> Optional[str]:
    """The account label an environment variable configures, or ``None``.

    The same mapping :func:`_discover_accounts` applies, because :func:`_replaced`
    has to select the same variables it does. It selected them by
    ``key.upper().endswith(label.upper())``, which is wrong in both directions:
    ``TELEGRAM_SESSION_STRING_NETWORK`` ends with ``WORK``, so re-logging in
    `network` retired the live `work` client too - and the DEFAULT account's
    variables carry no suffix at all, so nothing ever matched ``default`` and its
    re-login was noticed, recorded as seen, and then silently ignored. That left
    the server holding the session Telegram had just invalidated, which is the
    exact AuthKeyUnregisteredError this module exists to prevent.
    """
    for prefix in _ACCOUNT_PREFIXES:
        if not key.startswith(prefix):
            continue
        suffix = key[len(prefix) :]
        # "" is TELEGRAM_SESSION_STRING/NAME; "S" is only the TELEGRAM_SESSION_STRINGS
        # pool, which has no NAME counterpart. `_discover_accounts` reads all of
        # these as the default account, and anything else as no account at all.
        if suffix == "" or (suffix == "S" and prefix.endswith("STRING")):
            return "default"
        if suffix.startswith("_"):
            try:
                return normalise_account_label(suffix[1:]).lower()
            except ValueError:
                return None
        return None
    return None


def record_activated(clients: dict, digests: dict, labels: set) -> None:
    """Move the active digest for the given labels, and for nobody else.

    `_replaced` compares against `_env_digests`, so whatever sits there is what
    this server believes is serving. Setting it to the whole desired revision
    before the transaction ran meant a candidate that failed to connect was
    recorded as in force: the old client kept answering, the next reload compared
    the file against itself, found nothing to do, and the account never retried.
    """
    global _env_digests
    moved = dict(_env_digests)
    for key, value in digests.items():
        if _account_label_of(key) in labels:
            moved[key] = value
    # A label that has gone from the configuration entirely takes its digest with
    # it; one that is merely not activated yet keeps the digest it is serving on.
    for key in list(moved):
        if _account_label_of(key) not in clients and key not in digests:
            moved.pop(key, None)
    _env_digests = moved


def _replaced(label: str, digests: dict) -> bool:
    """Whether this label's session value differs from the one in use."""
    before = {k: v for k, v in _env_digests.items() if _account_label_of(k) == label}
    after = {k: v for k, v in digests.items() if _account_label_of(k) == label}
    return before != after


def get_client(account: str = None) -> TelegramClient:
    """Resolve account label to TelegramClient."""
    refresh_accounts()
    if account is None:
        if len(clients) == 1:
            return next(iter(clients.values()))
        raise ValueError(f"Account is required. Available accounts: {', '.join(clients.keys())}")
    label = account.lower()
    if label not in clients:
        raise ValueError(
            f"Unknown account '{account}'. Available accounts: {', '.join(clients.keys())}"
        )
    return clients[label]


def is_multi_mode() -> bool:
    """Return True when more than one account is configured."""
    return len(clients) > 1


def with_account(readonly=False):
    """Decorator that adds multi-account support to MCP tools.

    - In single-mode: always uses the sole client, no output tagging.
    - In multi-mode with explicit account: uses that account's client.
    - In multi-mode without account + readonly: fans out to all accounts
      concurrently and returns one JSON object, ``{"accounts": {label: result}}``.
      A failing account appears as ``{"error": "<Type>: <message>"}`` beside the
      others rather than discarding them.
    - In multi-mode without account + NOT readonly: returns an error.

    The wrapped function must accept ``account: str = None`` and use
    ``get_client(account)`` internally to obtain the TelegramClient.
    """

    def decorator(fn):
        @wraps(fn)
        async def wrapper(*args, **kwargs):
            account = kwargs.get("account")

            # BEFORE the mode decision, not after. `is_multi_mode()` reads the
            # registry, and the registry only moves when something refreshes it -
            # so a second account added while the server ran was invisible here,
            # and a write with no `account` was refused for being single-mode
            # right up until some other call happened to refresh first.
            refresh_accounts()

            # Explicit account OR single-mode -> call once
            if account is not None or not is_multi_mode():
                return await fn(*args, **kwargs)

            # account is None AND multi-mode
            if not readonly:
                labels = ", ".join(clients.keys())
                return f"Error: 'account' is required. Available accounts: {labels}"

            # Read-only fan-out to all accounts concurrently
            async def _call_for(label):
                kw = dict(kwargs)
                kw["account"] = label
                return await fn(*args, **kw)

            # return_exceptions: without it the first failing account propagates out of
            # gather and out of this wrapper, discarding every other account's already
            # completed result — one expired session turned a five-account query into a
            # single error string.
            #
            # labels is materialised once and reused for the zip, so results cannot be
            # mis-paired if `clients` is rebound mid-await. The old code carried the
            # label inside the returned tuple, which return_exceptions makes impossible
            # for the failing branch.
            labels = list(clients)
            outcomes = await asyncio.gather(
                *(_call_for(label) for label in labels), return_exceptions=True
            )

            # One envelope instead of "\n\n".join(f"[{label}]\n{result}"). Every tool
            # returns JSON from format_tool_result, and welding those strings together
            # produced something no caller could parse. Values are decoded where they
            # are JSON and kept verbatim where a tool answers in prose ("No messages
            # found."), so both kinds survive.
            #
            # BaseException, not Exception: gather(return_exceptions=True) returns
            # whatever was raised, and CancelledError is a BaseException.
            accounts: dict[str, Any] = {}
            for label, outcome in zip(labels, outcomes):
                if isinstance(outcome, BaseException):
                    accounts[label] = {"error": f"{type(outcome).__name__}: {outcome}"}
                    continue
                try:
                    accounts[label] = json.loads(outcome)
                except (TypeError, ValueError):
                    accounts[label] = outcome
            # ensure_ascii=False matches format_tool_result, so non-ASCII chat titles are
            # not escaped twice; default=str is a net for a non-string, non-JSON value —
            # this wrapper must never raise.
            return json.dumps({"accounts": accounts}, ensure_ascii=False, default=str)

        # The routing contract, readable without unwrapping: a registry test can
        # check it against the tool annotation, which is how save_disappearing_media
        # was found declaring readOnlyHint=False while routing as read-only.
        wrapper.__telegram_readonly__ = readonly
        return wrapper

    return decorator


__all__ = [
    "on_clients_changed",
    "_RETIRE_DRAIN_SECONDS",
    "drain_retirements",
    "_BURNED_SESSION_MESSAGE",
    "_CONN_VERIFY_INTERVAL",
    "_PROXY_TYPES_ALL",
    "_PROXY_TYPES_SOCKS_HTTP",
    "_RECONNECT_LOCKS",
    "_RECONNECT_TIMEOUT",
    "_UNPROTECTED_SESSION_MESSAGE",
    "_acquire_session",
    "_build_client",
    "_build_proxy_for_label",
    "_discover_accounts",
    "_force_reconnect",
    "_get_proxy_env",
    "_last_conn_verified",
    "_parse_session_pool",
    "SessionNotProtected",
    "NoAccountsConfigured",
    "adopt_legacy_session",
    "clients",
    "console_handler",
    "ensure_connected",
    "get_client",
    "harden_env_file",
    "harden_session_files",
    "is_multi_mode",
    "log_event",
    "log_file_path",
    "logger",
    "package_dir",
    "restrict_to_owner",
    "session_file_path",
    "safe_exception",
    "script_dir",
    "with_account",
]
