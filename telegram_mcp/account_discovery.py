"""Which accounts the configuration declares, built as clients.

Split out of ``connection`` (spec 029, size ceiling). This is the reading of
``TELEGRAM_SESSION_STRING_<LABEL>`` / ``TELEGRAM_SESSION_NAME_<LABEL>`` and the
unsuffixed/pool default: label normalisation, refusing one account defined twice,
and saying so when nothing is configured at all. Connecting, admitting and
reloading stay in ``connection`` and ``account_lifecycle``.
"""

from telegram_mcp.aliases import normalise_account_label
from telegram_mcp.session_pool import _parse_session_pool
from telegram_mcp.settings import StartupMessage, ValidationError

__all__ = ["NoAccountsConfigured", "build_accounts"]


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


def build_accounts(environment, reuse, accounts, *, string_session, build_client, acquire_session):
    """Fill ``accounts`` (label -> client) from ``environment``; reuse ``reuse``'s clients.

    The three callables come from the caller - ``connection`` passes its own
    ``StringSession``, ``_build_client`` and ``_acquire_session`` at call time, which
    is what tests patch there. ``accounts`` is filled in place so a caller that has
    to dispose of a partial result on failure can see what was built.
    """

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
        session = string_session(value) if kind == "string" else value
        accounts[label] = build_client(session, label)

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
            accounts["default"] = build_client(
                string_session(acquire_session(session_pool)), "default"
            )
        elif session_string:
            accounts["default"] = build_client(string_session(session_string), "default")
        elif session_name:
            accounts["default"] = build_client(session_name, "default")

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
