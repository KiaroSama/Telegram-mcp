"""Application entrypoints for the Telegram MCP server."""

from telegram_mcp.install_guard import UnsafeInstallationError, assert_safe_distribution

try:
    assert_safe_distribution()
except UnsafeInstallationError as exc:
    raise SystemExit(str(exc)) from None

from telethon.errors import AuthKeyDuplicatedError

from telegram_mcp import runtime as _runtime
from telegram_mcp import admission as _admission
from telegram_mcp.connection import _BURNED_SESSION_MESSAGE, harden_env_file, parse_port
from telegram_mcp.paging import bounded_number
from telegram_mcp.safe_log import safe_exception
from telegram_mcp.settings import StartupMessage, state_dir, stranded_state_dir
from telegram_mcp.runtime import *
from telegram_mcp.singleton import (
    DEFAULT_GRACE_SECONDS,
    SessionLock,
    SessionLockError,
    session_identity,
)
import telegram_mcp.tools  # noqa: F401 - registers MCP tools via decorators

# The session locks this process holds, shared with the reload path so an
# account added while the server runs is admitted the same way one present at
# boot is. Released in _main's finally block, so a lock never outlives the
# process. `_session_locks` stays as the name the tests and this module use.
# `reject_duplicate_sessions` moved there too, so the reload path runs the same
# check; both keep the names this module and its tests already use.
from telegram_mcp.admission import reject_duplicate_sessions as _reject_duplicate_sessions
from telegram_mcp.admission import session_locks as _session_locks

# Closing a client this startup attempt built but will not serve from. The
# lease release below is tied to it, so the lock never goes before the socket.
from telegram_mcp.retirement import retire as _retire

# Every transport this server can actually run. Anything else is a typo.
_TRANSPORTS = ("stdio", "http", "sse")

# Leaving because another instance already holds the session is not a failure of
# this server, and it is the single most common way it is started wrongly. It
# left through the same `exit 1` as a crash, so the launcher's last word was
# "uv exited with code 1" - blaming the package manager for a deliberate,
# correct refusal and sending the operator to debug the wrong thing. Its own
# code lets the launcher say what actually happened. Below 126, which a shell
# reserves for "could not execute".
EXIT_SESSION_HELD = 75


def _lock_grace_seconds() -> float:
    """The configured lock grace period, or a loud error for anything unusable.

    ``nan`` used to sail straight through: every ``time.monotonic() >= nan``
    comparison is false, so the "wait briefly, then give up" loop waited for
    ever. A malformed value silently became the default, which hid the typo.
    """
    raw = os.getenv("TELEGRAM_LOCK_GRACE_SECONDS")
    if not raw:
        return DEFAULT_GRACE_SECONDS
    # The same rule the lock itself applies, from the same table: a local
    # finite/non-negative test accepted 1e18, which is an unbounded wait written
    # in digits and would have stalled startup rather than failing it.
    span = bounded_number(raw, "lock_grace_seconds")
    if span.error:
        raise ValidationError(f"TELEGRAM_LOCK_GRACE_SECONDS: {span.error}")
    return span.value


async def _connect_authorized_client(label, client) -> None:
    # First, prevent our own duplicate-spawn case outright: an exclusive
    # per-session lock means a second instance of this server never even
    # attempts to connect while another instance already holds the same
    # session (see telegram_mcp/singleton.py for why and how).
    grace = _lock_grace_seconds()
    await _admission.claim_session(
        label,
        client,
        grace_seconds=grace,
        # Only fires when the lock is actually held by someone else. Twenty
        # seconds is the default grace, and all twenty used to pass without a
        # word: a second instance printed "Starting N Telegram client(s)", went
        # silent, and died. Measured at 24.6s end to end on this machine.
        on_wait=lambda seconds: startup_note(
            f"[{label}] Another process still holds this account's Telegram session. "
            f"Waiting up to {seconds:.0f}s for it to let go - this is the running "
            "instance shutting down, or an instance you did not mean to leave running."
        ),
    )

    # No retry. Telegram invalidates an auth key used from two places at once
    # permanently -- connection.py has said so on the reconnect path all along
    # -- so the four attempts here only spent 2+4+8 seconds re-asking for a key
    # that can never come back, and then reported the raw Telethon error
    # instead of the sentence that says how to recover.
    #
    # ONE budget over connect AND the authorization check, because neither has a
    # deadline of its own and either can sit on an unreachable DC indefinitely.
    # A startup that never finishes and never says why is the worst of the three
    # outcomes; this makes it the one that cannot happen.
    try:
        async with asyncio.timeout(_CONNECT_PHASE_SECONDS):
            await _connect_and_check(label, client)
    except (asyncio.TimeoutError, TimeoutError) as exc:
        _admission.forget(label, closing=_retire(client), client=client)
        raise StartupMessage(
            f"[{label}] Telegram did not answer within {_CONNECT_PHASE_SECONDS:.0f}s while "
            "connecting and checking the session. The session lock was released, so a "
            "retry will not queue behind this attempt. Check network reachability and "
            "any TELEGRAM_PROXY_* settings."
        ) from exc


async def _connect_and_check(label, client) -> None:
    """Connect and prove the session is usable. Bounded by its caller."""
    try:
        await client.connect()
    except AuthKeyDuplicatedError as exc:
        try:
            await client.disconnect()
        except Exception:
            pass
        # Nothing is connected on this session, so nothing should still be
        # holding its lock: a retry after fixing the config must not queue
        # behind a lock this failed attempt left standing.
        _admission.forget(label, client=client)
        raise StartupMessage(f"[{label}] {_BURNED_SESSION_MESSAGE}") from exc

    if await client.is_user_authorized():
        return

    raise StartupMessage(
        f"Telegram client '{label}' is not authorized. Interactive phone login "
        "is disabled for the MCP server because it runs over stdio. Generate a "
        "session string with `uv run session_string_generator.py`, then set "
        "TELEGRAM_SESSION_STRING or TELEGRAM_SESSION_STRING_<LABEL> in .env. "
        "For existing file sessions, run the login outside the MCP server first."
    )


def _binds_beyond_this_machine(host: str) -> bool:
    """Whether ``host`` accepts connections from anywhere but this machine.

    Unparseable names answer True. A hostname here is almost always a deliberate
    public bind, and guessing "probably local" about an address that decides who
    can reach a Telegram account is the wrong direction to be wrong in.
    """
    import ipaddress

    candidate = (host or "").strip().strip("[]")
    if candidate.lower() in {"localhost", ""}:
        return False
    try:
        address = ipaddress.ip_address(candidate)
    except ValueError:
        return True
    # 0.0.0.0 and :: are not "unspecified" in any harmless sense here - they mean
    # every interface the machine has.
    return not address.is_loopback


def _refuse_unauthenticated_remote_bind(host: str) -> None:
    """Stop a remote bind that nothing authenticates.

    `MCP_ALLOWED_HOSTS` and the DNS-rebinding protection it enables are not
    authentication. They check which name a request arrived under, which stops a
    browser on the operator's own machine being tricked into calling this server
    - it asks nothing about WHO is calling. Bound to a routable address without
    something in front that does, every tool here is available to anyone who can
    reach the port: read any conversation, send as the account, delete history.

    This server implements no authentication of its own, and that is deliberate -
    a hand-rolled token scheme that no real client has exercised would read as
    protection without being any. So the safe configurations are the two where
    something else is doing the work, and both have to be stated explicitly.
    """
    if not _binds_beyond_this_machine(host):
        return
    if _parse_bool_env(os.getenv("MCP_TRUSTED_PROXY_AUTH"), False):
        return
    if _parse_bool_env(os.getenv("MCP_ALLOW_UNAUTHENTICATED_REMOTE"), False):
        startup_note(
            f"WARNING: serving on {host} with no authentication, because "
            "MCP_ALLOW_UNAUTHENTICATED_REMOTE is set. Anyone who can reach this "
            "port controls the configured Telegram account(s)."
        )
        return

    raise ValidationError(
        f"Refusing to serve on {host}: that address is reachable from outside this "
        "machine and nothing here authenticates callers. Every tool on this server "
        "acts as your Telegram account.\n"
        "  - Keep it local (the default): unset MCP_HOST, or set it to 127.0.0.1.\n"
        "  - Behind a reverse proxy that authenticates requests: set "
        "MCP_TRUSTED_PROXY_AUTH=1 to state that it does.\n"
        "  - Deliberately open, on a trusted private network: set "
        "MCP_ALLOW_UNAUTHENTICATED_REMOTE=1.\n"
        "MCP_ALLOWED_HOSTS is not an answer here - it checks which name a request "
        "used, never who sent it."
    )


def _transport_security():
    """MCP_ALLOWED_HOSTS/MCP_ALLOWED_ORIGINS as DNS-rebinding protection, or None.

    Needed when the server sits behind a reverse proxy on a public domain rather
    than being reached only over 127.0.0.1/localhost.

    Returned rather than assigned: under mcp 1.x this was `mcp.settings.
    transport_security`, but 2.x dropped host/port/transport_security from
    `settings` and made them parameters of the `run_*_async` calls. Handing the
    value back keeps the "no hosts configured means no override" decision here,
    where the environment is read.
    """
    raw_hosts = os.getenv("MCP_ALLOWED_HOSTS", "")
    allowed_hosts = [h.strip() for h in raw_hosts.split(",") if h.strip()]
    if not allowed_hosts:
        return None

    from mcp.server.transport_security import TransportSecuritySettings

    raw_origins = os.getenv("MCP_ALLOWED_ORIGINS", "")
    allowed_origins = [o.strip() for o in raw_origins.split(",") if o.strip()]

    return TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=allowed_hosts,
        allowed_origins=allowed_origins,
    )


def _stateless_http() -> bool:
    """Whether the HTTP transport skips session ids. Default: no, it issues them.

    Stateless was the original choice, so ONE long-lived process could hold the
    shared Telethon session and still survive a restart: with no session id there
    is nothing to invalidate, and a client's next call goes straight through
    instead of being answered "No valid session ID provided".

    That turned out to be the problem, not the feature. A client reads
    `tools/list` when it connects and caches it. A stateless restart tells it
    NOTHING, so it keeps validating calls against the schema it first saw - a
    widened `inspect_sticker_set` and a newly added `edit_quick_reply` were both
    refused by a client while the server accepted the identical calls, and no
    number of restarts helped, because a stateless restart has nothing to say.

    Measured on this server, same build, one restart each:

      stateless=True   no session id issued; after the restart the old
                       (absent) id still answers 200 with the full tool list
      stateless=False  session id issued; after the restart it answers
                       404 "Session not found"

    That 404 is the whole point. It is the only thing in the protocol that makes
    a restart observable, and the spec has the client re-initialise when it sees
    one - which refetches the tools. The price is one rejected call per restart,
    which is strictly better than a client that is silently wrong.

    `MCP_STATELESS_HTTP=true` restores the old shape. Only an explicit,
    recognised truth does: this is deliberately NOT `parse_bool_env`, whose
    documented contract is that anything unrecognised is False - right for the
    four switches that default to off, and exactly wrong here, where
    `MCP_STATELESS_HTTP=ture` would silently change the transport.
    """
    raw = os.getenv("MCP_STATELESS_HTTP")
    if raw is None:
        return False
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _recoverable_sessions(app):
    """Drop a session id from `initialize`, so an expired session has a way back.

    `_stateless_http` below chose stateful HTTP for its 404: it is the one signal
    that tells a client the server restarted and its cached tool list is stale,
    "and the spec has the client re-initialise when it sees one". That sentence
    was not true of this server. The SDK's session manager reaches its
    new-session branch only when the header is ABSENT
    (`mcp/server/streamable_http_manager.py`, the `request_mcp_session_id is None`
    test), so an `initialize` carrying a stale id got the same 404 as any other
    call and the client could never obtain a fresh one.

    The client half makes it permanent rather than merely annoying: a 404 WITH a
    session id becomes an ordinary JSON-RPC error - "Session terminated" - and the
    transport is NOT torn down. The harness therefore keeps reporting the
    connector as connected while every call fails identically. Measured on
    2026-09-20, after four restarts in one session: `status: connected,
    tool_count: 227` beside a `get_me` that answered `Session terminated`, with no
    reconnect available because re-dialling is offered for connectors and this is
    a plain server.

    `initialize` is the method that CREATES a session, so an id on it is
    meaningless by definition and dropping it costs nothing. Every other method
    keeps its id and still earns the 404 - the signal survives intact; only the
    way back is reopened.

    Written as raw ASGI rather than Starlette middleware because the body has to
    be read to see the method and then replayed unchanged to the app underneath;
    a request that is not JSON, or not `initialize`, passes through untouched.
    """
    import json as _json

    async def wrapped(scope, receive, send):
        if scope.get("type") != "http" or scope.get("method") != "POST":
            await app(scope, receive, send)
            return
        if not any(key == b"mcp-session-id" for key, _ in scope.get("headers", [])):
            await app(scope, receive, send)
            return

        body, more = b"", True
        while more:
            message = await receive()
            body += message.get("body", b"")
            more = message.get("more_body", False)

        try:
            parsed = _json.loads(body)
        except Exception:
            # Not JSON, or not valid JSON. Not this layer's business to judge:
            # a transport guard that rejected a request would be a worse defect
            # than the one it exists to fix.
            parsed = None

        calls = parsed if isinstance(parsed, list) else [parsed]
        initializing = any(
            isinstance(one, dict) and one.get("method") == "initialize" for one in calls
        )
        if initializing:
            scope = dict(scope)
            scope["headers"] = [
                (key, value) for key, value in scope["headers"] if key != b"mcp-session-id"
            ]

        replayed = False

        async def replay():
            nonlocal replayed
            if replayed:
                return await receive()
            replayed = True
            return {"type": "http.request", "body": body, "more_body": False}

        await app(scope, replay, send)

    return wrapped


_SESSION_RECOVERY = "_telegram_mcp_session_recovery"


def _install_session_recovery(server) -> None:
    """Wrap `server`'s app builder so every app it builds carries the recovery.

    The builder rather than the runner: `run_streamable_http_async` reaches
    `streamable_http_app` through `self`, so shadowing it with an instance
    attribute reaches the served app without taking over the call that
    `tests/test_runtime.py` patches to keep `_serve` off a real port.

    Idempotent by marker, because a second install would wrap the wrapper and
    read the body twice.

    A server with no builder is left alone rather than refused. The recovery only
    means anything once a real app exists, and `_serve`'s own tests drive it with
    a double that serves nothing - requiring the attribute there would make this
    helper decide what a test may stand in for.
    """
    builder = getattr(server, "streamable_http_app", None)
    if builder is None or getattr(builder, _SESSION_RECOVERY, False):
        return

    def build(*args, **kwargs):
        return _recoverable_sessions(builder(*args, **kwargs))

    setattr(build, _SESSION_RECOVERY, True)
    server.streamable_http_app = build


async def _serve(transport: str) -> None:
    """Run the MCP server on the selected transport.

    HTTP transports let one long-lived process hold a single shared Telegram
    connection while multiple local MCP clients connect over HTTP, instead of
    each client spawning its own Telethon session (which Telegram
    throttles/flags). "http" is streamable HTTP — the current MCP transport
    that Claude Code (`--transport http`) and Codex (`--url`) speak natively;
    "sse" is kept for clients that only support the legacy SSE transport.

    An unrecognised name is refused rather than quietly falling back to stdio:
    `MCP_TRANSPORT=htpp` used to produce a server that reported a healthy start
    and an HTTP client that could never reach it.
    """
    if transport not in _TRANSPORTS:
        raise ValidationError(
            f"Invalid MCP_TRANSPORT {transport!r}. Expected one of: "
            f"{', '.join(sorted(_TRANSPORTS))}."
        )
    if transport in ("http", "sse"):
        host = os.getenv("MCP_HOST", "127.0.0.1")
        # Before the port is opened, not after: a refusal that arrives once the
        # socket is already listening has already been too late.
        _refuse_unauthenticated_remote_bind(host)
        port = parse_port(os.getenv("MCP_PORT", "8765"), "MCP_PORT")
        # 2.x takes these per-call. Only pass transport_security when the
        # environment actually configured one: the parameter's own default is
        # what applies otherwise, and forcing None over it would be a silent
        # downgrade of whatever protection the SDK enables by default.
        options = {"host": host, "port": port}
        security = _transport_security()
        if security is not None:
            options["transport_security"] = security
        if transport == "http":
            # `stateless_http` moved here from the server constructor in 2.x.
            #
            # `session_idle_timeout=None` turns OFF the SDK's 30-minute reaper,
            # and it is not a tuning choice - it is what keeps the 404 honest.
            # A stateful session whose id is reaped answers 404 "Session not
            # found", which is the exact code `_stateless_http` above chose
            # statefulness FOR: the one signal that tells a client the server
            # restarted and its cached tool list is stale. Left on, that signal
            # also fires at a server that never went anywhere, half an hour
            # after a client last spoke - a client that reads it correctly loses
            # a call, one that does not reports "Session terminated" and stops.
            # An agent waiting on a person is quiet for far longer than that.
            #
            # What the reaper also did was collect a session a client abandoned
            # without its DELETE; those now persist. `max_sessions` is left at
            # the SDK's 10 000, so that leak has a ceiling and reaches it after
            # more reconnects than this server will see - and 503 at a known
            # bound beats a 404 at half an hour.
            #
            # `_recoverable_sessions` is installed by wrapping the APP BUILDER,
            # not by replacing this call. `run_streamable_http_async` reaches the
            # builder through `self`, so an instance attribute shadows it and the
            # wrapper reaches the served app either way - while `_serve` keeps
            # calling the one method `tests/test_runtime.py` monkeypatches to stop
            # it binding a real port. Replacing the call instead was tried first
            # and hung the suite: the patch no longer applied, `_serve` bound the
            # port for real, and the run sat there with no output and almost no
            # CPU until it was killed at 22 minutes.
            _install_session_recovery(mcp)
            await mcp.run_streamable_http_async(
                stateless_http=_stateless_http(), session_idle_timeout=None, **options
            )
        else:
            await mcp.run_sse_async(**options)
    else:
        # Use the asynchronous entrypoint instead of mcp.run()
        await mcp.run_stdio_async()


# Startup failures this package raises itself, whose message is a fixed sentence
# telling the operator what to change. Everything else -- Telethon, sqlite, the
# OS -- carries whatever the failing call was given, and is reduced to a type,
# a length and a digest.
#
# The narrowness is the point: a server that will not start prints one line, and
# an operator who cannot read it has nothing else to go on. `StartupMessage` is
# the type this package raises for exactly those sentences. It used to be plain
# `RuntimeError`, which also covered any RuntimeError a dependency raised during
# startup - and those carry whatever the failing call was given.
_READABLE_STARTUP_ERRORS = (ValidationError, SessionLockError, StartupMessage)


def _startup_text(error: BaseException) -> str:
    if isinstance(error, _READABLE_STARTUP_ERRORS):
        return str(error)
    return safe_exception(error)


# The launcher persists the server's own diagnostics and withholds everything
# else on stderr, because a Telethon warning naming a chat or a third-party
# traceback with locals in it was never composed with persistence in mind. The
# lines below WERE: every one is a literal this file writes, and any exception in
# them goes through `_startup_text`/`safe_exception` first, so it carries a shape
# and a digest rather than what the failing call was given.
#
# They cannot go through the logger instead - it sits at ERROR, so the whole
# startup narrative would vanish - and they cannot be recognised by shape,
# because a bare print is indistinguishable from anyone else's. The marker is
# what makes the promise checkable from outside: start-mcp.ps1 allowlists this
# exact prefix, and nothing else can claim it by accident.
STARTUP_MARKER = "[telegram-mcp]"


def startup_note(text: str) -> None:
    """One startup diagnostic, marked so the launcher may keep it."""
    print(f"{STARTUP_MARKER} {text}", file=sys.stderr)


async def _main() -> None:
    try:
        # The door `release_all()` closes at the bottom of this function. Stated
        # at the start of serving rather than assumed, because the boundary is
        # sticky on purpose: a shutdown must not be undone by a slow acquire.
        _admission.begin_serving()
        labels = ", ".join(clients.keys())
        _reject_duplicate_sessions(clients)
        # Said before anything signs in. A deployment whose state directory moved -
        # the container image now points XDG_STATE_HOME at the mounted volume -
        # would otherwise start beside an existing key store and never mention
        # it, and those secret-chat keys cannot be re-derived once the old
        # container is gone.
        stranded = stranded_state_dir()
        if stranded is not None:
            startup_note(
                f"State from an earlier location is still at {stranded} and this process "
                f"is using {state_dir()}. Nothing has been moved or deleted. If that older "
                "directory holds secret-chat keys, copy it across BEFORE replacing "
                "this container - a lost key store takes its chats' history with it."
            )

        startup_note(f"Starting {len(clients)} Telegram client(s) ({labels})...")
        # OWNED siblings. A bare `gather` propagates the first exception while
        # the others are still running and unawaited, so cleanup began beside
        # live connects - one of which could still take a session lock after
        # this function had decided to give up. `return_exceptions=True` waits
        # for every sibling to settle; the first real failure is raised after.
        outcomes = await asyncio.gather(
            *(_connect_authorized_client(label, cl) for label, cl in clients.items()),
            return_exceptions=True,
        )
        for outcome in outcomes:
            if isinstance(outcome, BaseException):
                raise outcome

        # Warm entity caches — StringSession has no persistent cache,
        # so fetch all dialogs once per client to populate them.
        # Runs in background: blocking startup on this (e.g. under a
        # GetDialogsRequest flood wait) makes MCP clients time out, and
        # resolve_entity() re-warms the cache on miss anyway.
        startup_note("Warming entity caches (background)...")

        async def _warm_caches() -> None:
            # Through the managed registry, NOT `cl.get_dialogs()` directly. The
            # direct call is a task nothing owns: `drain_warms()` could not see
            # it, `cancel_warm()` could not stop it when its client was retired,
            # and shutdown had no way to wait for it. The registry bounds each
            # warm and records why a cache is cold.
            from telegram_mcp.dialog_warm import warm_dialogs_once

            try:
                await asyncio.gather(*(warm_dialogs_once(cl) for cl in clients.values()))
                startup_note("Entity caches warmed.")
            except Exception as warm_exc:
                # stderr may be persisted by the launcher, so this says what
                # failed and where, never what the failing call was given.
                startup_note(f"Entity cache warm failed: {safe_exception(warm_exc)}")

        # Held deliberately: asyncio keeps only a weak reference to a running task,
        # so dropping this name can let the cache warm-up be collected mid-flight.
        warm_task = asyncio.create_task(_warm_caches())  # noqa: F841

        transport = os.getenv("MCP_TRANSPORT", "stdio").lower()
        startup_note(f"Telegram client(s) started ({labels}). Running MCP server ({transport})...")
        await _serve(transport)
    except Exception as e:
        startup_note(f"Error starting client: {_startup_text(e)}")
        if isinstance(e, sqlite3.OperationalError) and "database is locked" in str(e):
            startup_note("Database lock detected. Please ensure no other instances are running.")
        elif isinstance(e, SessionLockError):
            startup_note(
                "Another instance of this MCP server already holds this Telegram "
                "session (e.g. the client restarted the connector without the old "
                "process exiting yet). This instance is exiting instead of "
                "connecting a second time, which would risk Telegram invalidating "
                "the session for both. Retry once the other instance is gone."
            )
            sys.exit(EXIT_SESSION_HELD)
        sys.exit(1)
    finally:
        # STOP NEW WORK FIRST. The incoming-event consumer writes to the feed and
        # calls into clients, so tearing the clients down underneath it produced
        # errors from a component that was merely still running. Nothing below
        # waited for it either - it was left to whatever cancelled the loop.
        try:
            from telegram_mcp.tools import feed_lifecycle as _feed

            if _feed.feed_enabled():
                stopped = await asyncio.wait_for(_feed._stop_current(), timeout=_FEED_STOP_SECONDS)
                if not stopped:
                    startup_note(
                        "The incoming-event consumer did not stop within "
                        f"{_FEED_STOP_SECONDS:.0f}s; continuing with shutdown."
                    )
        except (asyncio.TimeoutError, TimeoutError):
            startup_note(
                f"The incoming-event consumer did not stop within {_FEED_STOP_SECONDS:.0f}s; "
                "continuing with shutdown."
            )
        except Exception:
            pass
        # The secret-chat managers FIRST, and the ordering is the whole point.
        # They ride the Telethon clients rather than holding a connection of their
        # own, so a client torn down underneath one leaves it flushing key material
        # through a socket that is already gone - and a key lost on exit takes its
        # chat's history with it, with no way to re-derive one. The previous
        # backend held its own connection and was therefore flushed AFTER the
        # disconnect; moving to one that does not makes that ordering wrong.
        try:
            from telegram_mcp.secret_backend import close_all as _close_secret

            unflushed = await asyncio.wait_for(_close_secret(), timeout=_SECRET_CLOSE_SECONDS)
            for account, error in unflushed:
                startup_note(
                    f"[{account}] the secret-chat backend did not close cleanly "
                    f"({_startup_text(error)}); keys written since its last flush may be lost."
                )
        except (asyncio.TimeoutError, TimeoutError):
            startup_note(
                "The secret-chat backend did not finish closing within "
                f"{_SECRET_CLOSE_SECONDS:.0f}s; exiting anyway. Keys written since its last "
                "flush may be lost."
            )
        except Exception as exc:
            startup_note(f"Closing the secret-chat backend failed: {_startup_text(exc)}")

        # BOUNDED, and that is the whole point of the deadline. This gather was
        # unbounded, so a single client whose disconnect never returned held
        # shutdown here forever - and everything below it simply never ran. One
        # stalled cleanup must not be able to suppress the others.
        try:
            await asyncio.wait_for(
                asyncio.gather(
                    *(cl.disconnect() for cl in clients.values()), return_exceptions=True
                ),
                timeout=_DISCONNECT_ALL_SECONDS,
            )
        except (asyncio.TimeoutError, TimeoutError):
            startup_note(
                f"Some accounts did not disconnect within {_DISCONNECT_ALL_SECONDS:.0f}s; "
                "continuing with shutdown."
            )
        except Exception:
            pass

        # A client REPLACED while the server ran is not in `clients` any more:
        # `refresh_accounts` dropped it and its disconnect is still in flight.
        # Releasing its session lock now is what lets a second connection claim
        # one session, which Telegram answers by burning it for both. Bounded,
        # because exit must not wait on a socket that will never close.
        try:
            still_closing = await drain_retirements()
            if still_closing:
                startup_note(
                    f"{still_closing} retired client(s) did not finish disconnecting "
                    f"within {_RETIRE_DRAIN_SECONDS:.0f}s; releasing their session locks "
                    "anyway so this process can exit."
                )
        except Exception as exc:
            startup_note(f"Waiting for retired clients failed: {_startup_text(exc)}")

        # A lease whose release is waiting on a socket to close. Draining before
        # the sweep below means those releases happen in the right order rather
        # than being cancelled by the loop shutting down underneath them.
        # Cancelled before the locks go: a warm still running holds the client
        # it is warming and goes on calling into it after the socket is closed.
        try:
            from telegram_mcp.dialog_warm import drain_warms

            still_warming = await drain_warms()
            if still_warming:
                startup_note(f"{still_warming} dialog warm(s) did not stop when asked.")
        except Exception as exc:
            startup_note(f"Stopping dialog warms failed: {_startup_text(exc)}")

        # A reload's admission still in flight. It owns a staged client and may
        # be about to take a lease, so exiting past it leaves both to whatever
        # the loop does on its way down.
        try:
            from telegram_mcp.account_lifecycle import drain as _drain_admissions

            unsettled = await asyncio.wait_for(_drain_admissions(), timeout=_ADMIT_DRAIN_SECONDS)
            if unsettled:
                startup_note(
                    f"{unsettled} account admission(s) had not settled within "
                    f"{_ADMIT_DRAIN_SECONDS:.0f}s; their staged clients are being closed."
                )
        except (asyncio.TimeoutError, TimeoutError):
            startup_note("Account admissions did not settle before exit.")
        except Exception as exc:
            startup_note(f"Waiting for account admissions failed: {_startup_text(exc)}")

        try:
            unreleased = await _admission.drain_releases()
            if unreleased:
                startup_note(
                    f"{unreleased} session lease release(s) were still waiting on a socket; "
                    "they are being kept rather than released, because a socket that never "
                    "confirmed it closed may still be connected."
                )
        except Exception as exc:
            startup_note(f"Waiting for session lease releases failed: {_startup_text(exc)}")

        # Named before the locks go, because this is the one thing shutdown
        # cannot put right: a session whose socket never confirmed it closed
        # keeps its lease deliberately, and the next start of this server will
        # refuse to connect it. Saying which accounts, and why, is the difference
        # between a recoverable state and a mystery.
        for account, why in sorted(_admission.unreleased_leases.items()):
            startup_note(
                f"[{account}] its session lease was NOT released: {why}. Nothing else "
                "may connect that session until this process has fully exited."
            )
        _admission.release_all()


# How long one account gets to connect AND prove its session, together. Neither
# call bounds itself, so without this a single unreachable DC held startup open
# with no message and no exit.
_CONNECT_PHASE_SECONDS = 60.0


# How long shutdown waits for every account to disconnect before moving on. The
# session locks are released after it, and a lock left held is an account this
# server cannot start again, so this cannot hold the exit path open indefinitely.
_DISCONNECT_ALL_SECONDS = 15.0

# How long shutdown waits for the incoming-event consumer to stop before it
# carries on. Short: it is asked to stop FIRST so the clients under it are not
# torn away mid-write, and a consumer that will not stop must not hold exit.
_FEED_STOP_SECONDS: float = 5.0

# How long shutdown waits for a reload's admission to settle. One budget for
# all of them: each is already bounded by ADMIT_PHASE_SECONDS, and exit must
# not wait out several of those in series.
_ADMIT_DRAIN_SECONDS: float = 20.0


# How long shutdown waits for the secret-chat backend to flush and close.
# Generous, because the cost of cutting it short is unrecoverable: the keys that
# decrypt a secret chat's history. Bounded all the same - exit must not hang
# forever.
_SECRET_CLOSE_SECONDS: float = 30.0


def main() -> None:
    # Startup, not import: this touches a file's permissions, and a library that
    # does that merely by being imported is a surprise. `.env` holds the API hash
    # and, in the single-account setup, a session string; the documented
    # `cp .env.example .env` leaves it 0644 under a normal umask.
    harden_env_file()
    _configure_allowed_roots_from_cli(sys.argv[1:])
    _runtime._apply_exposed_tools_mode()
    asyncio.run(_main())


if __name__ == "__main__":
    main()
