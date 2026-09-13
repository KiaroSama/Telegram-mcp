"""One reading of the account configuration, used for every decision made from it.

The file was read three times at startup, and the three answers were allowed to
be about three different revisions:

* ``_discover_accounts()`` built clients from ``os.environ``;
* ``_env_fingerprint()`` then re-read the file to record what had been seen;
* ``_current_digests(_accounts_from_disk())`` re-read it AGAIN to record what
  was active.

An edit landing between the first and the third - and the account manager
rewrites ``.env`` while the server runs, so the window is real rather than
theoretical - was recorded as the active revision with no client ever built for
it. The next reload then compared the new file against itself, found nothing to
do, and the old client went on serving under the new configuration's name.

A snapshot fixes the revision once: the file's bytes are read a single time, and
the fingerprint, the parsed values and the per-account digests all come from
THOSE bytes. Two answers from one snapshot cannot disagree, because there is
only one reading behind them.

Provenance is the other half, and it is decided in :mod:`telegram_mcp.settings`
before ``load_dotenv`` runs - see ``PROCESS_ACCOUNT_VARS`` there. It is applied
here, last, because ``load_dotenv()`` does not override: a variable the process
supplied is the one in force, whether or not the file also names it.
"""

from __future__ import annotations

import hashlib
import io
import os
from dataclasses import dataclass
from typing import Dict, Optional, Tuple

from telegram_mcp.settings import ACCOUNT_PREFIXES
from telegram_mcp.settings import PROCESS_ACCOUNT_VARS

# Read as a module global at call time, never captured into a local at import.
# That makes THIS name the seam a test redirects; binding settings' copy into a
# default argument or a closure would leave a redirected test reading the real
# process environment, which is how a fixture's value once reached live state.


@dataclass(frozen=True)
class Snapshot:
    """One revision of the configuration, and everything derived from it.

    Frozen because the point is that these three cannot drift apart: a caller
    that could replace ``env`` without replacing ``stamp`` would reintroduce
    exactly the disagreement this exists to remove.
    """

    env: Dict[str, str]
    stamp: Tuple[str, ...]
    digests: Dict[str, str]
    path: Optional[str]

    def describes_same_file_as(self, other: "Snapshot") -> bool:
        return self.stamp == other.stamp


def account_digest(value: str) -> str:
    """A session string is a full login; only ever its digest is kept."""
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def digests_of(env: Dict[str, str]) -> Dict[str, str]:
    return {
        key: account_digest(value)
        for key, value in env.items()
        if key.startswith(ACCOUNT_PREFIXES) and value
    }


def _read_file_bytes(path: Optional[str]) -> Optional[bytes]:
    if not path:
        return None
    try:
        with open(path, "rb") as handle:
            return handle.read()
    except OSError:
        return None


def read_snapshot(path: Optional[str] = None) -> Snapshot:
    """Read the configuration once and return everything decided from it.

    ``path`` defaults to the `.env` this process reads. A missing or unreadable
    file is not an error here: the process may legitimately run on real
    environment variables alone, and the empty stamp says so.
    """
    from telegram_mcp.account_config import _env_file

    if path is None:
        path = _env_file()
    raw = _read_file_bytes(path)

    # The fingerprint hashes the SAME bytes the values were parsed from. Stat
    # metadata was the previous answer and it was wrong twice over: a re-login
    # rewrites `.env` with a session string of the same length, moving neither
    # size nor - on a coarse filesystem - mtime; and a separate stat is a second
    # reading that can describe a different revision than the parse.
    stamp: Tuple[str, ...] = () if raw is None else (hashlib.sha256(raw).hexdigest(),)

    on_disk: Dict[str, str] = {}
    if raw is not None:
        from dotenv import dotenv_values

        text = raw.decode("utf-8", "replace")
        on_disk = {k: v for k, v in dotenv_values(stream=io.StringIO(text)).items() if v}

    env = {k: v for k, v in os.environ.items() if not k.startswith(ACCOUNT_PREFIXES)}
    # File first, process last. `load_dotenv()` does not override, so a variable
    # the process supplied is the one the running clients were built from; the
    # file supplies the rest. A key removed from the file and absent from the
    # process genuinely disappears, which is what makes an account deletable.
    env.update({k: v for k, v in on_disk.items() if k.startswith(ACCOUNT_PREFIXES)})
    env.update(PROCESS_ACCOUNT_VARS)

    return Snapshot(env=env, stamp=stamp, digests=digests_of(env), path=path)


__all__ = ["Snapshot", "account_digest", "digests_of", "read_snapshot"]
