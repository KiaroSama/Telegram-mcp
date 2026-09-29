"""Move everything the server stored under an account label to a new label (spec 025).

A label is more than its `.env` line. The secret-chat key store and its sidecars are
files named after it, and five JSON stores key rows by it: ghost mode, the proxy pool,
the approval records, the always-approvals and the contact aliases. `Manage-Accounts.ps1`
renames the `.env` line; this moves the rest, while the server is stopped.

All or nothing: every target is checked free before anything moves, and a failure part
way puts back what already moved. Labels are compared lower-cased, as every store keys
them. Run as ``python -m telegram_mcp.account_rename <state-dir> <old> <new>``; exit 0
done (or nothing to move), 2 refused, 1 failed.
"""

import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Callable, List

from telegram_mcp.owner_only import restrict_to_owner_strict

__all__ = ["RenameRefused", "main", "rename"]

_SIDECARS = ("{}", "{}.owner.json", "{}-history.json", "{}-media.json")
_ALIAS_SEPARATOR = "\n"  # alias_store._ACCOUNT_SEPARATOR


class RenameRefused(Exception):
    """The new label already has something stored under it; nothing was changed."""


def _read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def _write_json(path: Path, data) -> None:
    handle, temporary = tempfile.mkstemp(dir=str(path.parent), suffix=".tmp")
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(data, fh, indent=1, sort_keys=True, ensure_ascii=False)
        restrict_to_owner_strict(Path(temporary))
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def _move_dict_keys(*names: str) -> Callable:
    def move(data, old, new):
        for name in names:
            table = data.get(name)
            if isinstance(table, dict) and old in table:
                if new in table:
                    raise RenameRefused(f"'{new}' already has {name} entries")
                table[new] = table.pop(old)
        return data

    return move


def _move_grants(data, old, new):
    grants = data.get("grants", [])
    if any(g and g[0] == old for g in grants) and any(g and g[0] == new for g in grants):
        raise RenameRefused(f"'{new}' already has always-approvals")
    data["grants"] = [[new, *g[1:]] if g and g[0] == old else g for g in grants]
    return data


def _move_aliases(data, old, new):
    old_prefix, new_prefix = old + _ALIAS_SEPARATOR, new + _ALIAS_SEPARATOR
    if any(k.lower().startswith(old_prefix) for k in data) and any(
        k.lower().startswith(new_prefix) for k in data
    ):
        raise RenameRefused(f"'{new}' already has contact aliases")
    return {
        (new_prefix + k[len(old_prefix) :] if k.lower().startswith(old_prefix) else k): v
        for k, v in data.items()
    }


_STORES = {
    "ghost.json": _move_dict_keys("accounts", "chats"),
    "proxy-pool.json": _move_dict_keys("accounts", "routes"),
    "approval-messages.json": _move_dict_keys("accounts"),
    "always-approvals.json": _move_grants,
    "aliases.json": _move_aliases,
}


def rename(state: Path, old: str, new: str) -> List[str]:
    """Move ``old``'s stored state to ``new``; return what moved, e.g. ``secret-chats/old``."""
    state, old, new = Path(state), old.lower(), new.lower()
    chats = state / "secret-chats"
    files = [(chats / p.format(old), chats / p.format(new)) for p in _SIDECARS]
    files = [(src, dst) for src, dst in files if src.exists()]
    for _src, dst in files:
        if dst.exists():
            raise RenameRefused(f"{dst.name} already exists in {chats}; nothing was changed")

    # Every store transformed in memory first, so a refusal changes nothing on disk.
    rewrites = []
    for name, move in _STORES.items():
        path = state / name
        if not path.exists():
            continue
        before = _read_json(path)
        try:
            after = move(json.loads(json.dumps(before)), old, new)
        except RenameRefused as refusal:
            raise RenameRefused(f"{name}: {refusal}; nothing was changed") from None
        if after != before:
            rewrites.append((path, before, after))

    moved, undo = [], []
    try:
        for src, dst in files:
            os.replace(src, dst)
            undo.append(lambda s=src, d=dst: os.replace(d, s))
            moved.append(f"secret-chats/{src.name}")
        for path, before, after in rewrites:
            _write_json(path, after)
            undo.append(lambda p=path, b=before: _write_json(p, b))
            moved.append(path.name)
    except BaseException:
        for step in reversed(undo):
            step()
        raise
    return moved


def main(argv=None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 3:
        print(
            "usage: python -m telegram_mcp.account_rename <state-dir> <old> <new>", file=sys.stderr
        )
        return 1
    try:
        moved = rename(Path(args[0]), args[1], args[2])
    except RenameRefused as refusal:
        print(f"Refused: {refusal}", file=sys.stderr)
        return 2
    except Exception as error:  # reported to the launcher, which stops the rename
        print(f"Failed: {type(error).__name__}: {error}", file=sys.stderr)
        return 1
    print("\n".join(moved) if moved else "nothing stored under that label")
    return 0


if __name__ == "__main__":
    sys.exit(main())
