"""Offline discovery and distribution contracts for the operator skills."""

import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / "plugins" / "telegram-operator"
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
        links = re.findall(r"\]\((references/[^)]+)\)", text)
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
    shutil.copytree(PLUGIN, source)
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
    (source / ".env").write_text("SYNTHETIC=not-a-secret\n", encoding="utf-8")
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
    assert claude["version"] == portable["version"]
    for manifest in (portable, claude):
        assert "mcpServers" not in manifest
        assert "hooks" not in manifest
    cases = json.loads(
        (ROOT / "tests/fixtures/operator-plugin/evals.json").read_text(encoding="utf-8")
    )["evals"]
    assert {case["skill"] for case in cases} == NAMES
    assert all(case["prompt"] and case["assertions"] for case in cases)
