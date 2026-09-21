"""What the server was given: environment configuration, and the error for bad input.

Deliberately the bottom of the import graph. Everything above it - the client layer,
the alias store, file-path security, the tools - needs some of this, and none of it
needs them, so putting it anywhere higher creates a cycle.

`ValidationError` lives here because that is what it mostly reports: seven of its nine
raise sites are configuration the operator got wrong (a proxy type that does not exist,
a host without a port), where the right behaviour is to fail at startup rather than at
the first call. The two remaining sites use it for a caller-supplied ID, which is a
different kind of wrong wearing the same name - worth separating one day, but changing
it now would change what callers catch.
"""

import os
from pathlib import Path
from typing import Optional

from dotenv import load_dotenv

# Which environment variables name an account. Here rather than in
# `account_config` because provenance has to be captured on the line below this
# one, and this module is the bottom of the import graph.
ACCOUNT_PREFIXES = ("TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME")

# BEFORE `load_dotenv`, and that ordering is the whole point.
#
# Provenance is "did the PROCESS supply this, or the file?", and it is decidable
# exactly once: the moment dotenv runs, the two are merged beyond separating.
# Deciding it afterwards by asking which keys are absent from the file gets the
# overlapping case backwards - a key set in BOTH was read as file-managed, so a
# process value of `external-B` served every call while the reload view reported
# the file's `file-A`, and deleting that line from the file dropped the genuine
# external account as well.
#
# `load_dotenv()` does not override, so a key present here is the one in force.
PROCESS_ACCOUNT_VARS: dict = {
    key: value for key, value in os.environ.items() if key.startswith(ACCOUNT_PREFIXES) and value
}

# Same line, same reason, for the allow-list. `file_roots` re-reads the file so a
# folder can be allowed without a restart, and it must not overwrite a value the
# PROCESS supplied - which after `load_dotenv` is indistinguishable from the
# file's own. `None` here means the file owns the key.
PROCESS_FILE_ROOTS: Optional[str] = os.environ.get("TELEGRAM_FILE_ROOTS") or None

# Loaded HERE, not by whoever imports this. These values are read at import time, and
# this module now sits at the bottom of the import graph - `main.py` reaches it through
# `file_roots` before `runtime` has run a line, so relying on `runtime` to have called
# `load_dotenv()` first meant the server could not start from a `.env` at all. It is
# idempotent, and `runtime` still calls it for its own remaining reads.
load_dotenv()


class ValidationError(Exception):
    """Custom exception for validation errors."""

    pass


class StartupMessage(RuntimeError):
    """A startup failure whose message this package wrote, word for word.

    The startup path prints these verbatim, because a server that will not
    start has to say what to change and an operator who cannot read it has
    nothing else to go on. That allowance used to be granted to `RuntimeError`
    as a whole, which also covered any RuntimeError a dependency happened to
    raise during startup - carrying whatever the failing call was given.

    Subclasses RuntimeError so existing `except RuntimeError` handlers keep
    catching it.
    """


def _require_credential(name: str) -> str:
    """Read a mandatory credential, or say which one is missing and where it comes from.

    Missing credentials are the most common way this server fails for a new operator,
    and the bare reads this replaced failed as `int(None)` - a TypeError naming no
    variable, no file and no next step. Same wording as
    `session_string_generator.py`, which already got this right: name the variable,
    name the file, name where the value comes from. It raises rather than prints
    because this is a library module, and on the stdio transport stdout is the MCP
    protocol channel.
    """
    value = os.getenv(name)
    if not value:
        raise ValidationError(
            f"{name} is not set. Put it in the .env file next to the server. "
            "Get both TELEGRAM_API_ID and TELEGRAM_API_HASH from "
            "https://my.telegram.org/apps."
        )
    return value


_RAW_TELEGRAM_API_ID = _require_credential("TELEGRAM_API_ID")
try:
    TELEGRAM_API_ID = int(_RAW_TELEGRAM_API_ID)
except ValueError:
    # Truncated: this is an operator-supplied string, and an error message is a
    # place things get logged.
    raise ValidationError(
        "TELEGRAM_API_ID must be a number, but .env has "
        f"{_RAW_TELEGRAM_API_ID[:40]!r}. Copy the numeric App api_id from "
        "https://my.telegram.org/apps."
    ) from None

TELEGRAM_API_HASH = _require_credential("TELEGRAM_API_HASH")


def state_dir() -> Path:
    """Where runtime state goes: never the install directory.

    The install directory may be read-only, is often a git checkout, and inherits
    whatever that directory grants. The alias store already used this location;
    the error log and file-based Telethon sessions join it so there is one place
    to lock down rather than three. `start-mcp.ps1` computes the same path.
    """
    base = os.getenv("XDG_STATE_HOME") or Path.home() / ".local" / "state"
    return Path(base) / "telegram-mcp"


def stranded_state_dir() -> Optional[Path]:
    """A previous state directory that still holds data the new one does not.

    Deployments that pointed `XDG_STATE_HOME` somewhere new - the container
    image now sets it to `/data/state`, under the mounted volume - leave
    whatever was in the old location exactly where it was. That is the right
    thing to do with it: a secret-chat key store is not re-creatable, because
    the keys cannot be re-derived, so nothing here moves or deletes one.

    What must not happen is the server quietly signing in afresh beside it and
    the operator never learning the old one existed. Returning the path is how
    the startup path can say so.
    """
    current = state_dir()
    legacy = Path.home() / ".local" / "state" / "telegram-mcp"
    if legacy == current:
        return None
    left_behind = _account_state_in(legacy) - _account_state_in(current)
    return legacy if left_behind else None


def _account_state_in(root: Path) -> set:
    """What a directory holds that BELONGS to an account, named piece by piece.

    Two things this is not. It is not "the directory is non-empty": this
    server's own logging creates `mcp_errors.log` in the state directory at
    import time, before startup ever asks, so the new location was never empty
    and the warning could not fire on the deployment shape it exists for. A log
    is re-created every run and nothing is lost with it.

    And it is not a single yes/no for the whole directory, because a PARTIAL
    migration is the expensive case: the session file copied across and the
    key store left behind reads as "the new location is in use" while the
    secret-chat keys sit in the old one. Naming each piece is what lets the
    caller subtract one set from the other.

    What counts is what cannot be re-created: a secret-chat key store per
    account (its keys cannot be re-derived), a Telethon session (it IS the
    login), and the alias store.
    """
    found = set()
    try:
        if not root.is_dir():
            return found
        for database in (root / "secret-chats").glob("*"):
            if database.is_dir() and any(database.iterdir()):
                found.add(f"secret-chats:{database.name}")
        for entry in root.iterdir():
            if entry.suffix == ".session":
                found.add(f"session:{entry.stem}")
            elif entry.suffix == ".json":
                found.add(f"store:{entry.name}")
    except OSError:
        # Unreadable is not evidence of absence, but it is also not something
        # this can report a path for. The startup note is advisory; a permission
        # problem here belongs to whatever actually tries to use the directory.
        return set()
    return found


def parse_bool_env(value: Optional[str], default: bool) -> bool:
    """A permissive truthy read: unset means `default`, and only the obvious words win.

    Anything unrecognised is False rather than an error, because a malformed switch
    must not stop the server from starting. One parser, four subsystems - the proxy
    layer, the alias store, file-path security and the event feed - so that
    `TELEGRAM_EVENT_FEED=on` cannot mean different things in different places.
    """
    if value is None:
        return default
    return value.strip().lower() in {"1", "true", "yes", "on"}


# Historic private name: `runtime` exported it under the leading underscore and three
# subsystems still import it that way.
_parse_bool_env = parse_bool_env

__all__ = [
    "TELEGRAM_API_HASH",
    "TELEGRAM_API_ID",
    "ValidationError",
    "parse_bool_env",
    "state_dir",
    "_parse_bool_env",
]
