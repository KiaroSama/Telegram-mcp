"""Who owns a TDLib database directory, and the receipt that says a client let go.

This is ADR 0001 one layer down. There a session lease was keyed by a LABEL,
which is reusable, and five defects came out of that single ambiguity. The
native database had the same shape: the only thing standing between two
processes and one TDLib database was ``closed=True``, a boolean the caller
passed in, backed by a module-level dict keyed by label. Neither says anything
about another process, and a label-keyed receipt is a value one attempt can
leave behind for the next one to spend.

So ownership is keyed by the thing that owns it:

* **The lease is the canonical database PATH**, and nothing else - not the
  label, which two accounts can wear in turn, and not the account, which is
  what the database is supposed to prove. It is an OS-level lock, so it is
  answerable to another process; a boolean is not.
* **The receipt is the ATTEMPT**, so ``closed_confirmed`` is a statement about
  the client this sequence actually closed rather than about whatever last
  used the name.

The lock is a plain open file handle, exactly as ``SessionLock`` is, and both
``flock`` and ``msvcrt.locking`` release when the handle closes. So ``_held``
below is the strong reference and that is its whole job: a lease nothing refers
to is a database the operating system has already handed back.

Nothing here waits. A TDLib database another process has open is not something
to sit on a deadline for - the honest answer is that this one may not touch it,
and the caller is told which path and which lock file say so.
"""

from __future__ import annotations

import hashlib
import logging
import os
from pathlib import Path
from typing import IO, Dict, Optional

from telegram_mcp.safe_log import log_event
from telegram_mcp.singleton import DEFAULT_LOCK_DIR, release_lock, try_lock_exclusive

__all__ = [
    "DatabaseBusy",
    "LoginAttempt",
    "database_identity",
    "hold",
    "lock_file",
    "owner_of",
    "release",
]


# Where the lock files live. A module attribute rather than a constant because a
# test has to point a child process at the same directory, and a child cannot
# see a monkeypatch.
lock_dir: Path = DEFAULT_LOCK_DIR


class DatabaseBusy(RuntimeError):
    """Something else owns this TDLib database, so nothing was opened or moved."""


def database_identity(database_dir) -> str:
    """The canonical path a lease is keyed by.

    ``realpath`` so a symlinked state directory resolves to the one database it
    names, ``normcase`` so a case-differing Windows spelling of the same
    directory is the same lease. ``abspath`` alone let both spellings buy their
    own lock, which is the label mistake wearing a different hat.
    """
    return os.path.normcase(os.path.realpath(str(database_dir)))


def lock_file(database_dir) -> Path:
    """The file whose OS lock IS this database's lease."""
    digest = hashlib.sha256(database_identity(database_dir).encode("utf-8")).hexdigest()[:16]
    lock_dir.mkdir(parents=True, exist_ok=True)
    return lock_dir / f"tdlib-{digest}.lock"


class _Held:
    """One database's lease. The handle is the lock; the owner is who may end it."""

    __slots__ = ("handle", "owner", "path")

    def __init__(self, handle: IO, owner: object, path: Path):
        self.handle = handle
        self.owner = owner
        self.path = path


_held: Dict[str, _Held] = {}


def owner_of(database_dir) -> Optional[object]:
    """Who holds this database's lease in THIS process, if anyone.

    Says nothing about another process - that question is only answerable by
    trying to take the lock, which is what :func:`hold` does.
    """
    held = _held.get(database_identity(database_dir))
    return None if held is None else held.owner


def hold(database_dir, owner: object) -> None:
    """Take the lease for this database, or refuse.

    Reentrant for the SAME owner and for nobody else. A login sequence holds one
    lease across its client, its quarantine and its one retry, and the clients it
    makes ask for that same lease under the sequence's own token; a second owner
    asking for the same path is the competing ownership this exists to refuse.
    """
    identity = database_identity(database_dir)
    held = _held.get(identity)
    if held is not None:
        if held.owner is owner:
            return
        raise DatabaseBusy(
            f"the TDLib database at {database_dir} is already owned inside this process. "
            "One database is one authorisation, and a second holder would open it beside "
            "the first. Nothing was opened."
        )
    path = lock_file(database_dir)
    handle = open(path, "a+")
    if not try_lock_exclusive(handle):
        handle.close()
        raise DatabaseBusy(
            f"another process already has the TDLib database at {database_dir} open "
            f"(lock held: {path}). Nothing was opened or moved: two processes writing one "
            "database corrupts secret-chat keys that cannot be re-derived. Stop the other "
            "process - or wait for it to exit, which releases this lock on its own - and "
            "retry."
        )
    _held[identity] = _Held(handle=handle, owner=owner, path=path)


def release(database_dir, owner: object) -> bool:
    """Give the lease back, and report whether this owner was the one holding it.

    ``False`` rather than an exception for a caller who does not own it, because
    both non-owners are ordinary: a client whose lease belongs to the login
    sequence around it, and a client that never took one.
    """
    identity = database_identity(database_dir)
    held = _held.get(identity)
    if held is None or held.owner is not owner:
        return False
    del _held[identity]
    release_lock(held.handle)
    held.handle.close()
    return True


class LoginAttempt:
    """One login sequence for one label: its database lease and its closure receipt.

    Both used to be keyed by the label. The receipt in particular was a
    module-level ``{label: bool}``, so a value some earlier attempt left behind
    was what authorised this one's quarantine - and a quarantine is a rename of
    a database whose keys cannot be re-derived.
    """

    def __init__(self, label: str, database_dir):
        self.label = label
        self.database_dir = Path(database_dir)
        # Set by `close` below for the client THIS attempt ran, and by nothing
        # else. False until something has actually watched a client reach
        # `authorizationStateClosed`.
        self.closed_confirmed = False

    async def close(self, client) -> bool:
        """Close this attempt's client and record whether it confirmed.

        Recorded rather than swallowed, and never raised: the caller is usually
        reporting a different failure already, and a close that could not be
        confirmed must not mask it - but it must also not be mistaken for one
        that was.
        """
        try:
            await client.close()
        except Exception as error:
            log_event(logging.WARNING, "a TDLib client did not confirm it closed", error=error)
            self.closed_confirmed = False
        else:
            self.closed_confirmed = True
        return self.closed_confirmed
