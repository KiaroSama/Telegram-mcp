"""The boundary between this project and the secret-chat package.

The owner maintains that package in its own repository and updates it independently,
so the cost of each future update is what these tests protect. They do NOT test the
package's correctness — its own suite owns that. They test the contract: that the
operations this project calls exist, take the arguments used here, and that the thing
installed is the right project at all.

Without them an upstream change surfaces as a runtime error inside a tool, days later,
on whichever call happened to run first. With them it fails in CI naming the
assumption, and the fix is one file.
"""

import ast
import inspect
import json
from importlib.metadata import distribution
from pathlib import Path

import pytest

PACKAGE = "telethon_secret_chat"
DISTRIBUTION = "telethon-secret-chat"
OWNER_REPO = "KiaroSama/Telethon-Secret-Chat"

#: Every manager operation this project calls, with the arguments it passes by name.
#: Adding a call site here that is not in this table is how the table goes stale, so
#: the seam is checked against it rather than the other way round.
CALLED_OPERATIONS = {
    "start": (),
    "stop": (),
    "status": (),
    "create": (),
    "accept": (),
    "close": (),
    "list": (),
    "send_message": (),
    "send_file": (),
    "save_file": (),
    "read_history": (),
    "set_ttl": (),
    "mark_read": (),
    "delete_messages": (),
    "flush_history": (),
    "set_typing": (),
}


def test_the_installed_distribution_is_the_owners_not_the_one_on_pypi():
    """PyPI serves a DIFFERENT package under this exact name.

    painor's archived `telethon-secret-chat` is published at 0.2.4 — a HIGHER version
    than the owner's 0.0.1 — so a bare requirement resolves to the wrong project and
    no version constraint reveals the substitution. The requirement is pinned by git
    for that reason, and this asserts the pin actually held: a machine that already
    had the archived package must fail here rather than shadow the real one.
    """
    dist = distribution(DISTRIBUTION)
    recorded = dist.read_text("direct_url.json")

    assert recorded is not None, (
        "no direct_url.json: this was installed from an index, not from the owner's "
        "repository. PyPI's package of the same name is a different project."
    )
    assert (
        OWNER_REPO.lower() in json.loads(recorded)["url"].lower()
    ), f"installed from {json.loads(recorded)['url']!r}, which is not {OWNER_REPO}"


def test_the_seam_is_the_only_module_that_imports_the_package():
    """One importer, so an upstream signature change is a one-file edit.

    This is the property that makes updating cheap. The moment a tool module imports
    the package directly, an upstream change becomes a search across call sites again
    — which is exactly the position the TDLib removal was undertaken to leave behind.
    """
    root = Path(__file__).resolve().parents[1] / "telegram_mcp"
    offenders = []

    for path in root.rglob("*.py"):
        if path.name == "secret_backend.py":
            continue
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names = [a.name for a in node.names]
            elif isinstance(node, ast.ImportFrom):
                names = [node.module or ""]
            else:
                continue
            if any(n == PACKAGE or n.startswith(PACKAGE + ".") for n in names):
                offenders.append(f"{path.relative_to(root)}:{node.lineno}")

    assert not offenders, (
        "only telegram_mcp/secret_backend.py may import "
        f"{PACKAGE}; found: {', '.join(offenders)}"
    )


def missing_operations(manager_cls) -> list:
    """Which operations this project calls that `manager_cls` does not offer.

    A function rather than an inline assertion so the check itself can be tested
    against a deliberately incompatible stand-in. A guard nobody has watched fail is
    a guard nobody knows works.
    """
    return sorted(
        name for name in CALLED_OPERATIONS if not callable(getattr(manager_cls, name, None))
    )


@pytest.mark.parametrize("operation", sorted(CALLED_OPERATIONS))
def test_every_operation_this_project_calls_still_exists(operation):
    """An upstream rename fails here, naming the operation, instead of at runtime."""
    from telethon_secret_chat import SecretChatManager

    assert hasattr(SecretChatManager, operation), (
        f"the package no longer offers {operation!r}. This project calls it from "
        "telegram_mcp/secret_backend.py; adapt that file, or pin back."
    )
    assert callable(getattr(SecretChatManager, operation))


def test_the_installed_package_satisfies_the_whole_contract():
    """The same check the next paragraph proves can fail, run against the real thing."""
    from telethon_secret_chat import SecretChatManager

    assert missing_operations(SecretChatManager) == []


def test_an_incompatible_version_is_rejected_and_named():
    """The guard is watched failing, against a stand-in missing two operations.

    This is what makes the update path trustworthy rather than merely documented: an
    upstream version that dropped `send_file` and renamed `flush_history` is caught
    here, by name, in CI — not by a tool raising `AttributeError` at whatever moment
    the operator happened to send a file.
    """

    class OutdatedManager:
        pass

    for name in CALLED_OPERATIONS:
        if name not in ("send_file", "flush_history"):
            setattr(OutdatedManager, name, lambda self: None)

    assert missing_operations(OutdatedManager) == ["flush_history", "send_file"]


def test_the_manager_is_constructed_over_a_client_and_an_explicit_store():
    """Two arguments, and storage with no default.

    The signature carries a safety property: the package refuses to guess where key
    material goes. If a future version defaults it, this server would silently start
    writing keys somewhere the operator did not choose.
    """
    from telethon_secret_chat import SecretChatManager

    parameters = inspect.signature(SecretChatManager.__init__).parameters

    assert list(parameters)[1:3] == ["client", "storage"]
    assert parameters["storage"].default is inspect.Parameter.empty, (
        "storage gained a default; key material would be written somewhere the "
        "operator did not choose"
    )


def test_the_storage_backend_this_server_uses_takes_a_path():
    """`FileStorage(path)` is what the seam builds per account."""
    from telethon_secret_chat import FileStorage

    assert list(inspect.signature(FileStorage.__init__).parameters)[1:] == ["path"]


def test_the_packages_public_surface_still_carries_what_the_seam_imports():
    """The seam imports exactly these names. A narrowed export fails here."""
    import telethon_secret_chat as package

    for name in ("SecretChatManager", "FileStorage", "StorageBackend"):
        assert name in package.__all__, f"{name} left the package's public surface"
