"""Offline discovery and distribution contracts for the operator skills."""

import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT
NAMES = {
    "telegram-bot-reconstruction",
    "telegram-chat-migration",
    "telegram-backup-restore",
    "telegram-access-audit",
    "telegram-format-reconstruction",
}


def test_five_portable_skills_and_local_resources():
    files = list((PLUGIN / "skills").glob("*/SKILL.md"))
    assert {p.parent.name for p in files} == NAMES
    for path in files:
        text = path.read_text(encoding="utf-8")
        front = text.split("---", 2)[1]
        assert f"name: {path.parent.name}" in front
        description = re.search(r"^description: (.+)$", front, re.MULTILINE).group(1)
        assert 1 <= len(description) <= 1024
        assert "allowed-tools" not in front
        assert len(text.splitlines()) < 500
        links = re.findall(r"\]\(((?:references/|\.\./)[^)]+)\)", text)
        assert links
        for link in links:
            target = path.parent / link
            assert target.is_file()
            assert target.resolve().is_relative_to(PLUGIN.resolve())


def test_package_flavors_preserve_skills_and_refuse_extra_files(tmp_path):
    import importlib.util
    import shutil
    import zipfile

    import pytest

    spec = importlib.util.spec_from_file_location(
        "operator_packager", ROOT / "scripts" / "package_operator_plugin.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    source = tmp_path / "source with spaces"
    source.mkdir()
    for name in module.FILES:
        target = source / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(PLUGIN / name, target)
    archives = {}
    for client in ("portable", "antigravity"):
        output = tmp_path / f"{client}.zip"
        module.build(source, output, client, ROOT)
        with zipfile.ZipFile(output) as archive:
            archives[client] = {n: archive.read(n) for n in archive.namelist()}
        with pytest.raises(FileExistsError):
            module.build(source, output, client, ROOT)
    portable = json.loads(archives["portable"]["telegram-operator/plugin.json"])
    ag = json.loads(archives["antigravity"]["telegram-operator/plugin.json"])
    assert portable["$schema"] == "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
    assert set(ag) == {"name", "description"}
    skill_paths = [n for n in archives["portable"] if "/skills/" in n]
    assert len(skill_paths) == 10
    assert all(archives["portable"][n] == archives["antigravity"][n] for n in skill_paths)
    assert "telegram-operator/LICENSE" in archives["portable"]
    assert "telegram-operator/.codex-plugin/plugin.json" in archives["portable"]
    assert "telegram-operator/.cursor-plugin/plugin.json" in archives["portable"]
    assert not any("-plugin/" in name for name in archives["antigravity"])
    assert not any(
        "telegram_mcp/" in name or name.endswith(".env") for name in archives["portable"]
    )
    (source / "skills" / ".env").write_text("SYNTHETIC=not-a-secret\n", encoding="utf-8")
    refused = tmp_path / "refused.zip"
    with pytest.raises(ValueError, match="Unexpected"):
        module.build(source, refused, "portable", ROOT)
    assert not refused.exists()


def test_official_descriptor_shapes_and_synthetic_eval_coverage():
    portable = json.loads((PLUGIN / "plugin.json").read_text(encoding="utf-8"))
    assert set(portable) <= {
        "$schema",
        "name",
        "version",
        "description",
        "author",
        "homepage",
        "repository",
        "license",
        "keywords",
        "extensions",
    }
    assert portable["name"] == "telegram-operator"
    assert portable["$schema"] == "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
    claude = json.loads((PLUGIN / ".claude-plugin/plugin.json").read_text(encoding="utf-8"))
    assert claude["skills"] == "./skills/"
    assert claude["name"] == portable["name"]
    marketplace = json.loads(
        (PLUGIN / ".claude-plugin/marketplace.json").read_text(encoding="utf-8")
    )
    assert claude["version"] == portable["version"] == "1.2.0"
    assert marketplace["metadata"]["version"] == portable["version"]
    assert len(marketplace["plugins"]) == 1
    assert marketplace["plugins"][0]["name"] == portable["name"]
    assert marketplace["plugins"][0]["version"] == portable["version"]
    for manifest in (portable, claude):
        assert "mcpServers" not in manifest
        assert "hooks" not in manifest
    cases = json.loads(
        (ROOT / "tests/fixtures/operator-plugin/evals.json").read_text(encoding="utf-8")
    )["evals"]
    assert {case["skill"] for case in cases} == NAMES
    assert all(case["prompt"] and case["assertions"] for case in cases)
    assert len({case["id"] for case in cases}) == len(cases)
    delta = {case["scenario"]: case for case in cases if "scenario" in case}
    assert set(delta) == {
        "management-selfbot-authority",
        "keyboard-kind-near-miss",
        "downloader-ambiguous-backend",
        "entity-visual-typography",
    }
    assert all(case["expected_output"] and len(case["assertions"]) >= 5 for case in delta.values())


def test_reconstruction_routes_existing_companions():
    """Resolve the instructions a reader reaches, not model effectiveness."""
    path = PLUGIN / "skills/telegram-bot-reconstruction/SKILL.md"
    text = path.read_text(encoding="utf-8")
    links = re.findall(r"\]\((\.\./[^)]+/SKILL\.md)\)", text)
    targets = {(path.parent / link).resolve() for link in links}
    assert targets == {
        (PLUGIN / "skills" / name / "SKILL.md").resolve()
        for name in NAMES - {"telegram-bot-reconstruction"}
    }
    assert all(target.is_file() and target.is_relative_to(PLUGIN.resolve()) for target in targets)


def test_repository_root_descriptors_resolve_single_canonical_payload():
    assert not (ROOT / "plugins/telegram-operator/skills").exists()
    for folder in (".claude-plugin", ".codex-plugin", ".cursor-plugin"):
        value = json.loads((ROOT / folder / "plugin.json").read_text(encoding="utf-8"))
        assert value["name"] == "telegram-operator"
        assert value["version"] == "1.2.0"
        assert value["skills"] == "./skills/"
        assert {p.name for p in (ROOT / value["skills"]).iterdir()} == NAMES
        assert not {"mcpServers", "hooks"} & value.keys()
    market = json.loads((ROOT / ".claude-plugin/marketplace.json").read_text(encoding="utf-8"))
    assert market["plugins"][0]["source"] == "./"


def test_explicit_installer_copies_complete_skills_and_refuses_collision(tmp_path):
    import importlib.util
    import pytest

    spec = importlib.util.spec_from_file_location(
        "install_operator", ROOT / "scripts/install_operator_skills.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    destination = tmp_path / "project with spaces" / ".hermes/skills"
    module.install(ROOT, destination)
    assert {p.name for p in destination.iterdir()} == NAMES
    for name in NAMES:
        assert (destination / name / "SKILL.md").read_bytes() == (
            ROOT / "skills" / name / "SKILL.md"
        ).read_bytes()
    with pytest.raises(FileExistsError):
        module.install(ROOT, destination)
    assert {p.name for p in destination.iterdir()} == NAMES


def test_installer_failure_rolls_back_only_owned_content(tmp_path, monkeypatch):
    import importlib.util
    import pytest

    spec = importlib.util.spec_from_file_location(
        "install_operator", ROOT / "scripts/install_operator_skills.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    destination = tmp_path / "skills"
    destination.mkdir()
    keep = destination / "owner.txt"
    keep.write_text("preserve", encoding="utf-8")
    original = Path.open
    calls = 0

    def failing(path, mode="r", *args, **kwargs):
        nonlocal calls
        if mode == "xb" and path.is_relative_to(destination):
            calls += 1
            if calls == 2:
                raise OSError("synthetic copy failure")
        return original(path, mode, *args, **kwargs)

    monkeypatch.setattr(Path, "open", failing)
    with pytest.raises(OSError, match="synthetic"):
        module.install(ROOT, destination)
    assert list(destination.iterdir()) == [keep]
    assert keep.read_text(encoding="utf-8") == "preserve"


def test_eight_installer_routes_and_link_refusal(tmp_path):
    import importlib.util
    import pytest

    spec = importlib.util.spec_from_file_location(
        "install_operator", ROOT / "scripts/install_operator_skills.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert module.CLIENT_DIRS == {
        "claude": ".claude/skills",
        "codex": ".agents/skills",
        "cursor": ".cursor/skills",
        "kiro": ".kiro/skills",
        "cline": ".cline/skills",
        "hermes": ".hermes/skills",
        "opencode": ".opencode/skills",
        "antigravity": ".agents/skills",
    }
    linked = tmp_path / "link"
    target = tmp_path / "target"
    target.mkdir()
    try:
        linked.symlink_to(target, target_is_directory=True)
    except OSError:
        pytest.skip("OS does not permit creating symlink fixture")
    with pytest.raises(ValueError, match="Linked"):
        module.install(ROOT, linked / "skills")
    assert not list(target.iterdir())


def test_installer_dot_segments_cannot_write_inside_canonical_source(tmp_path):
    import importlib.util
    import pytest
    import shutil

    spec = importlib.util.spec_from_file_location(
        "install_operator", ROOT / "scripts/install_operator_skills.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    repository = tmp_path / "repository"
    from scripts.package_operator_plugin import FILES

    for name in FILES:
        target = repository / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, target)
    with pytest.raises(ValueError, match="canonical"):
        module.install(repository, repository / "docs/../skills/extra")
    assert not (repository / "skills/extra").exists()


def test_installer_cli_uses_own_packager_and_explicit_project(tmp_path):
    import os
    import subprocess
    import sys

    env = dict(os.environ, XDG_STATE_HOME=str(tmp_path / "private-state"))
    command = [
        sys.executable,
        str(ROOT / "scripts/install_operator_skills.py"),
        "--client",
        "opencode",
        "--project",
        str(tmp_path / "workspace"),
    ]
    result = subprocess.run(
        command,
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=15,
    )
    assert result.returncode == 0, result.stderr
    assert "Installed five skills" in result.stdout
    assert {p.name for p in (tmp_path / "workspace/.opencode/skills").iterdir()} == NAMES
    refused = subprocess.run(
        command,
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=15,
    )
    assert refused.returncode == 1
    assert "not overwritten" in refused.stderr
    logs = list(
        (tmp_path / "private-state/telegram-mcp/logs").glob("install-operator-skills_*.log")
    )
    assert len(logs) == 2
    assert any("[ERROR]" in p.read_text(encoding="utf-8") for p in logs)


def test_outputs_cannot_poison_canonical_payload(tmp_path):
    import importlib.util
    import pytest
    from scripts.package_operator_plugin import build

    spec = importlib.util.spec_from_file_location(
        "install_operator", ROOT / "scripts/install_operator_skills.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    with pytest.raises(ValueError, match="canonical"):
        module.install(ROOT, ROOT / ".codex-plugin/copied-skills")
    with pytest.raises(ValueError, match="canonical"):
        build(
            ROOT,
            ROOT / "skills/telegram-bot-reconstruction/references/operator.zip",
            "portable",
            ROOT,
        )
    assert not (ROOT / ".codex-plugin/copied-skills").exists()
    assert not (ROOT / "skills/telegram-bot-reconstruction/references/operator.zip").exists()
