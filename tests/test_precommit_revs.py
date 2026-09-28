"""pre-commit's `rev:` pins are hand-synced with uv.lock; this is the check that was missing.

CI lints with the locked black and flake8 (`uv run --locked`), a contributor's hook with
the `rev:` in `.pre-commit-config.yaml`. When they drift, the hook formats one way and CI
demands another (plan 010).
"""

import re
import tomllib
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent
PINNED = {"https://github.com/psf/black": "black", "https://github.com/pycqa/flake8": "flake8"}


def _locked_versions():
    packages = tomllib.loads((REPO / "uv.lock").read_text(encoding="utf-8"))["package"]
    return {p["name"]: p["version"] for p in packages}


def _precommit_revs():
    text = (REPO / ".pre-commit-config.yaml").read_text(encoding="utf-8")
    return dict(re.findall(r"-\s+repo:\s+(\S+)\s+rev:\s+v?([\w.]+)", text))


@pytest.mark.parametrize("repo, package", PINNED.items())
def test_precommit_rev_matches_the_lockfile(repo, package):
    revs, locked = _precommit_revs(), _locked_versions()
    assert repo in revs, f"{repo} is no longer in .pre-commit-config.yaml"
    assert revs[repo] == locked[package], (
        f"{package}: pre-commit rev {revs[repo]} != uv.lock {locked[package]}; "
        "run `pre-commit autoupdate --repo <repo>` or re-lock so CI and the hook agree"
    )
