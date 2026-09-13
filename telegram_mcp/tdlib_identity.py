"""Which Telegram user a TDLib database belongs to, and what to do when it is not this one.

A label is a name in ``.env``. A TDLib database is a signed-in Telegram
account. Nothing tied the two together, and they come apart in an ordinary way:
an account is removed, the label is reused for a different person, and the
database directory - which outlives ``.env`` - is still there under the old
name. ``_attempt_login`` saw ``authorizationStateReady``, asked nothing further,
and handed back a client signed in as the PREVIOUS user. Every secret-chat call
made through it then ran as them.

So the database is bound to an identity, and the binding is checked before the
client is used for anything:

* ``getMe.id`` from TDLib against ``get_me().id`` from the Telethon session that
  names the same label. Two answers about the same account, from the two halves
  that are supposed to be it.
* A mismatch REFUSES. It does not repair, choose, or delete: the operator is the
  only one who knows which of the two accounts they meant.
* ``owner.json`` beside the database records the id once it has been proved.
  That is the migration for databases that predate this: the first successful
  verification writes the file, nothing is guessed from a filename, and nothing
  is deleted for lacking it.

The other half is what happens to a database Telegram no longer honours. It used
to be ``shutil.rmtree(..., ignore_errors=True)`` - no backup, no ownership
check, no confirmation the client had closed, and a locked directory reported as
a success. A real temporary fixture lost its bytes to it during the audit. Here
a dead database is QUARANTINED: renamed aside, intact, recoverable, and only
when the rename actually succeeds.
"""

from __future__ import annotations

import json
import logging
import os
import time
from pathlib import Path
from typing import Optional

from telegram_mcp.owner_only import restrict_to_owner_strict, verify_owner_only
from telegram_mcp.safe_log import log_event
from telegram_mcp.tdlib import database_dir_for

# One quarantine per login attempt, and no second one on the retry. "Log in
# again" against a dead authorisation costs a real login every time, and a loop
# that keeps quarantining and retrying is a loop that keeps spending them.
MAX_RECOVERY_ATTEMPTS = 1

_IDENTITY_FILE = "owner.json"


class IdentityMismatch(RuntimeError):
    """The TDLib database belongs to a different Telegram user than this label.

    Deliberately not a subclass of anything the login path treats as recoverable:
    there is no automatic repair for this, because both accounts are real and
    only the operator knows which one the label is supposed to name.
    """


class QuarantineFailed(RuntimeError):
    """The database could not be moved aside, so nothing was done to it."""


def identity_path(label: str) -> Path:
    return database_dir_for(label) / _IDENTITY_FILE


def read_identity(label: str) -> Optional[int]:
    """The user id this database was last proved to belong to, if it is recorded.

    ``None`` covers both "no file" and "a file that says nothing usable". Neither
    is evidence of a mismatch - a database created before this existed has no
    file and is perfectly good - so neither refuses anything on its own.
    """
    try:
        raw = identity_path(label).read_text(encoding="utf-8")
    except OSError:
        return None
    try:
        user_id = json.loads(raw).get("user_id")
    except (ValueError, AttributeError):
        return None
    return user_id if isinstance(user_id, int) and not isinstance(user_id, bool) else None


def record_identity(label: str, user_id: int) -> None:
    """Write the binding, atomically and owner-only.

    Best effort by design: a database that works must not become unusable
    because a note beside it could not be written. The verification itself is
    what protects the caller; this only saves doing it from scratch.
    """
    path = identity_path(label)
    temporary = path.with_name(path.name + ".tmp")
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary.write_text(
            json.dumps({"user_id": user_id, "label": label, "recorded_at": time.time()}),
            encoding="utf-8",
        )
        _restrict(temporary)
        os.replace(temporary, path)
    except OSError as error:
        log_event(logging.WARNING, "could not record the TDLib database owner", error=error)
        try:
            temporary.unlink()
        except OSError:
            pass


def _restrict(path: Path) -> None:
    """Owner-only: the file names a real account id."""
    if os.name != "nt":
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass
        return
    if not verify_owner_only(path):
        restrict_to_owner_strict(path)


async def telegram_user_id(tdlib_client) -> int:
    """Who TDLib says this client is signed in as."""
    return int((await tdlib_client.request({"@type": "getMe"}))["id"])


async def verify_owner(label: str, tdlib_client, telethon_client) -> int:
    """Prove the database and the session are the same Telegram account.

    Returns the user id on success, having recorded it. Raises
    ``IdentityMismatch`` otherwise, WITHOUT touching the database: a mismatch is
    a question for the operator, and deleting the evidence is the one response
    that makes it unanswerable.

    ``telethon_client`` of ``None`` means there is nothing to compare against -
    a repair tool run with no session to hand - and the recorded binding, if
    any, is used instead. A database with neither is left alone rather than
    accused.
    """
    theirs = await telegram_user_id(tdlib_client)

    if telethon_client is None:
        recorded = read_identity(label)
        if recorded is not None and recorded != theirs:
            raise IdentityMismatch(_mismatch_message(label, recorded, theirs, "recorded earlier"))
        return theirs

    mine = int((await telethon_client.get_me()).id)
    if mine != theirs:
        raise IdentityMismatch(_mismatch_message(label, mine, theirs, "the configured session"))
    record_identity(label, theirs)
    return theirs


def _mismatch_message(label: str, expected: int, found: int, whose: str) -> str:
    return (
        f"The TDLib database for account '{label}' is signed in as Telegram user {found}, "
        f"but {whose} for that label is user {expected}. This happens when a label is "
        "reused for a different account: the database outlives .env, so the new session "
        "inherits the previous user's TDLib login. Nothing has been changed. Either point "
        f"the label back at user {found}, or move {database_dir_for(label)} aside yourself "
        "and run the secret-chat login again to build a fresh database for this account."
    )


def quarantine_path(label: str, now: Optional[float] = None) -> Path:
    stamp = time.strftime("%Y%m%d-%H%M%S", time.gmtime(time.time() if now is None else now))
    return database_dir_for(label).with_name(f"{label}.quarantined-{stamp}")


def quarantine_database(label: str, why: str) -> Path:
    """Move a dead database aside. Never delete it.

    ``shutil.rmtree(..., ignore_errors=True)`` was three separate mistakes in one
    call: the bytes were gone with no copy, a directory still held open reported
    success while deleting nothing, and there was no way to get back the
    secret-chat keys if the diagnosis had been wrong. A rename has none of those
    properties. It also cannot succeed while a handle is open on Windows, which
    is exactly the confirmation that the client really closed.

    Raises ``QuarantineFailed`` rather than returning quietly: the caller is
    about to start a fresh login on the strength of this having happened.
    """
    source = database_dir_for(label)
    if not source.exists():
        raise QuarantineFailed(
            f"there is no TDLib database at {source} to move aside, so the failure "
            "was not a stale one"
        )
    target = quarantine_path(label)
    # A second quarantine inside the same second would otherwise land on the
    # first; the counter keeps both rather than one overwriting the other.
    suffix = 1
    while target.exists():
        target = target.with_name(f"{target.name}-{suffix}")
        suffix += 1
    try:
        os.replace(source, target)
    except OSError as error:
        raise QuarantineFailed(
            f"could not move {source} aside: {error}. Nothing was deleted. On Windows this "
            "usually means the TDLib client is still open on it; close the server and try "
            "again."
        ) from error
    log_event(
        logging.WARNING,
        "TDLib database quarantined",
        account=label,
        reason=why,
        kept_at=str(target),
    )
    return target


__all__ = [
    "IdentityMismatch",
    "MAX_RECOVERY_ATTEMPTS",
    "QuarantineFailed",
    "identity_path",
    "quarantine_database",
    "quarantine_path",
    "read_identity",
    "record_identity",
    "telegram_user_id",
    "verify_owner",
]
