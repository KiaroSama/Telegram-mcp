"""Which file the configuration comes from, and the names the rest of the package uses.

The reading itself moved to :mod:`telegram_mcp.account_snapshot`, because the
three separate reads this module used to perform were allowed to describe three
different revisions of the same file. What is left here is the question that
genuinely stands alone - WHICH file - plus the names `connection` re-exports and
the tests already patch, each delegating to the one snapshot so there is a
single reading behind every answer.

Provenance is decided earlier still, in :mod:`telegram_mcp.settings`, on the
line before ``load_dotenv`` runs. It cannot be decided here: by the time this
module is imported the process environment and the file have been merged, and
asking afterwards which keys are missing from the file answers a different
question - one that got the overlapping case backwards.
"""

from typing import Optional

from telegram_mcp.account_snapshot import account_digest as _snapshot_digest
from telegram_mcp.account_snapshot import digests_of, read_snapshot
from telegram_mcp.settings import ACCOUNT_PREFIXES


def _env_file() -> Optional[str]:
    """The `.env` this process reads, or None when it runs on real env vars."""
    try:
        from dotenv import find_dotenv

        return find_dotenv(usecwd=True) or None
    except Exception:
        return None


def _env_fingerprint(path: Optional[str]) -> tuple:
    """A digest of the file's CONTENT, not its metadata.

    This was `(st_mtime_ns, st_size)` and that was wrong. Replacing one session
    string with another of the same length changes neither: the account manager
    rewrites `.env` wholesale, so a re-login is exactly a same-size rewrite, and
    on a filesystem whose timestamp resolution is coarser than the gap between
    the two writes the mtime does not move either. CI caught it on a Windows
    runner where the local machine never had.

    Kept as a name of its own because the reload's first question is only "has
    the file changed at all", and answering it must not cost a parse. It hashes
    the same bytes :func:`read_snapshot` would.
    """
    from telegram_mcp.account_snapshot import _read_file_bytes

    raw = _read_file_bytes(path)
    return () if raw is None else (_account_digest_bytes(raw),)


def _account_digest_bytes(data: bytes) -> str:
    import hashlib

    return hashlib.sha256(data).hexdigest()


def _account_digest(value: str) -> str:
    """A session string is a full login; only ever its digest is kept."""
    return _snapshot_digest(value)


# The historic spelling, kept because `connection` re-exports it and `__all__`
# publishes it.
_ACCOUNT_PREFIXES = ACCOUNT_PREFIXES


def _accounts_from_disk() -> dict:
    """The environment as it would be if this process had started right now."""
    return read_snapshot().env


def _current_digests(env: dict) -> dict:
    return digests_of(env)


__all__ = [
    "_ACCOUNT_PREFIXES",
    "_account_digest",
    "_account_digest_bytes",
    "_accounts_from_disk",
    "_current_digests",
    "_env_file",
    "_env_fingerprint",
]
