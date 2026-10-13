#!/usr/bin/env python3
"""Allowlisted guidance-only archives with client-specific root manifests."""

import argparse
import json
from pathlib import Path
import stat
import sys
import zipfile

SKILLS = {
    "telegram-bot-reconstruction": "behavior-contract.md",
    "telegram-chat-migration": "migration-ledger.md",
    "telegram-backup-restore": "verification.md",
    "telegram-access-audit": "audit-matrix.md",
    "telegram-format-reconstruction": "fidelity.md",
}
FILES = {
    "plugin.json",
    "README.md",
    ".claude-plugin/plugin.json",
    ".claude-plugin/marketplace.json",
}
for name, reference in SKILLS.items():
    FILES.update({f"skills/{name}/SKILL.md", f"skills/{name}/references/{reference}"})
MAX_FILE_BYTES = 1_000_000


def _plain(path):
    info = path.lstat()
    if path.is_symlink() or getattr(info, "st_file_attributes", 0) & 0x400:
        raise ValueError("Linked or reparse package paths are not allowed.")
    if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
        raise ValueError("Only regular package paths are allowed.")


def build(source, output, client, repository):
    """Create a new ZIP after inspecting the entire explicit payload."""
    if client not in {"portable", "antigravity"}:
        raise ValueError("Unknown client flavor.")
    source, output, repository = Path(source), Path(output), Path(repository)
    for path in (source, *source.parents):
        _plain(path)
    dirs = {"."}
    for name in FILES:
        dirs.update(p.as_posix() for p in Path(name).parents)
    found = set()
    for path in source.rglob("*"):
        _plain(path)
        name = path.relative_to(source).as_posix()
        if path.is_dir():
            if name not in dirs:
                raise ValueError("Unexpected package directory.")
        elif name not in FILES:
            raise ValueError("Unexpected package file.")
        else:
            found.add(name)
    if found != FILES:
        raise ValueError("Required package files are missing.")
    payload = {}
    for name in sorted(FILES):
        path = source / name
        if path.stat().st_size > MAX_FILE_BYTES:
            raise ValueError("Package file exceeds byte limit.")
        data = path.read_bytes()
        data.decode("utf-8")
        payload[name] = data
    manifest = json.loads(payload["plugin.json"])
    if manifest.get("name") != "telegram-operator":
        raise ValueError("Unexpected plugin identity.")
    if client == "antigravity":
        payload["plugin.json"] = (
            json.dumps(
                {"name": manifest["name"], "description": manifest["description"]}, indent=2
            )
            + "\n"
        ).encode("utf-8")
        payload = {k: v for k, v in payload.items() if not k.startswith(".claude-plugin/")}
    for name in ("LICENSE", "NOTICE"):
        path = repository / name
        _plain(path)
        if path.stat().st_size > MAX_FILE_BYTES:
            raise ValueError("License exceeds byte limit.")
        payload[name] = path.read_bytes()
    if output.exists() or output.is_symlink():
        raise FileExistsError("Output already exists; choose a new path.")
    # Check ancestors before mkdir: a link must not create directories elsewhere.
    for path in (output.parent, *output.parent.parents):
        if path.exists() or path.is_symlink():
            _plain(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    created = False
    try:
        with output.open("xb") as handle:
            created = True
            with zipfile.ZipFile(handle, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name, data in sorted(payload.items()):
                    info = zipfile.ZipInfo(f"telegram-operator/{name}", (2026, 10, 12, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = 0o100644 << 16
                    archive.writestr(info, data)
    except BaseException:
        if created:
            output.unlink(missing_ok=True)
        raise
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client", choices=("portable", "antigravity"), required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    repository = Path(__file__).resolve().parents[1]
    sys.path.insert(0, str(repository))
    from telegram_mcp.session_log import run_log

    with run_log("package-operator-plugin") as event:
        try:
            build(repository / "plugins/telegram-operator", args.output, args.client, repository)
        except (OSError, ValueError, UnicodeError, KeyError):
            event("ERROR", "Package creation refused or failed")
            print(
                "Package creation failed: inspect payload and choose a new output path.",
                file=sys.stderr,
            )
            return 1
        event("INFO", "Guidance-only package created")
        print(f"Created {args.client} plugin archive.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
