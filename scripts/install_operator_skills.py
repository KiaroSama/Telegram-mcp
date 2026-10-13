#!/usr/bin/env python3
"""Explicit standalone skill copies; never configure or authenticate a client."""

import argparse
import importlib.util
from pathlib import Path
import sys

CLIENT_DIRS = {
    "claude": ".claude/skills",
    "codex": ".agents/skills",
    "cursor": ".cursor/skills",
    "kiro": ".kiro/skills",
    "cline": ".cline/skills",
    "hermes": ".hermes/skills",
    "opencode": ".opencode/skills",
    "antigravity": ".agents/skills",
}


class RollbackIncomplete(RuntimeError):
    """Some owned paths could not be removed; existing owner data was not changed."""


def install(repository, destination):
    spec = importlib.util.spec_from_file_location(
        "operator_payload", Path(__file__).with_name("package_operator_plugin.py")
    )
    packager = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(packager)
    read_payload, SKILLS, _plain = packager.read_payload, packager.SKILLS, packager._plain

    repository, destination = Path(repository).absolute(), Path(destination).absolute()
    payload = {
        name.removeprefix("skills/"): data
        for name, data in read_payload(repository).items()
        if name.startswith("skills/")
    }
    for path in (destination, *destination.parents):
        if path.exists() or path.is_symlink():
            _plain(path)
            if not path.is_dir():
                raise ValueError("Skill destination must be a directory.")
    repository, destination = repository.resolve(), destination.resolve()
    for name in SKILLS:
        target = destination / name
        if target.exists() or target.is_symlink():
            raise FileExistsError("A target skill already exists; nothing was copied.")
    if any(destination.is_relative_to(repository / tree) for tree in packager.PAYLOAD_TREES):
        raise ValueError("Installation cannot change the canonical payload source.")
    directories, files = [], []

    def directory(path):
        if path.exists():
            _plain(path)
            return
        directory(path.parent)
        path.mkdir()
        directories.append(path)

    try:
        directory(destination)
        for name in SKILLS:
            path = destination / name
            path.mkdir()
            directories.append(path)
        for name, data in sorted(payload.items()):
            target = destination / name
            directory(target.parent)
            with target.open("xb") as handle:
                files.append(target)
                handle.write(data)
    except BaseException:
        incomplete = False
        for path in reversed(files):
            try:
                path.unlink()
            except OSError:
                incomplete = True
        for path in reversed(directories):
            try:
                path.rmdir()
            except OSError:
                incomplete = True
        if incomplete:
            raise RollbackIncomplete(
                "Installation failed; owned rollback is incomplete."
            ) from None
        raise
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client", choices=sorted(CLIENT_DIRS))
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--project", type=Path)
    target.add_argument("--skills-dir", type=Path)
    args = parser.parse_args()
    if args.project and not args.client:
        parser.error("--project requires --client")
    root = Path(__file__).resolve().parents[1]
    sys.path.insert(0, str(root))
    from telegram_mcp.session_log import run_log

    destination = args.skills_dir if args.skills_dir else args.project / CLIENT_DIRS[args.client]
    with run_log("install-operator-skills") as event:
        try:
            install(root, destination)
        except RollbackIncomplete:
            event("ERROR", "Skill installation failed; owned rollback incomplete")
            print(
                "Installation failed; partial new files remain at the chosen destination.",
                file=sys.stderr,
            )
            return 1
        except Exception:
            event("ERROR", "Skill installation refused or failed")
            print(
                "Skill installation failed; existing skills were not overwritten.", file=sys.stderr
            )
            return 1
        event("INFO", "Five complete standalone skills installed")
        print(
            "Installed five skills. Restart the client and verify discovery; MCP setup is separate."
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
