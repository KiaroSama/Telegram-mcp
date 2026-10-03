"""Native Google client configuration and form capability contracts."""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from mcp.types import ClientCapabilities

from telegram_mcp import export_dialog


@pytest.mark.parametrize(
    "elicitation, expected",
    [
        (None, False),
        ({}, True),
        ({"form": {}}, True),
        ({"url": {}}, False),
        ({"form": {}, "url": {}}, True),
    ],
)
def test_export_uses_only_supported_forms(elicitation, expected):
    caps = ClientCapabilities(elicitation=elicitation)
    session = SimpleNamespace(client_capabilities=caps)
    assert export_dialog._has_form(SimpleNamespace(session=session)) is expected


@pytest.mark.parametrize(
    "relative, key",
    [
        ("integrations/gemini/telegram-mcp/gemini-extension.json", "httpUrl"),
        ("integrations/antigravity/mcp_config.example.json", "serverUrl"),
    ],
)
def test_native_clients_attach_shared_http_without_credentials_or_trust(relative, key):
    root = Path(__file__).resolve().parents[1]
    config = json.loads((root / relative).read_text(encoding="utf-8"))
    server = config["mcpServers"]["telegram-mcp"]
    assert set(server) == {key}
    assert server[key] == "http://127.0.0.1:8765/mcp"
    if key == "httpUrl":
        assert config["name"] == "telegram-mcp"
        assert config["version"]
