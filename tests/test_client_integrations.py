"""Native Google client configuration and form capability contracts."""

import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from mcp.types import ClientCapabilities

from telegram_mcp import export_dialog
from telegram_mcp.safeguard.channels import DialogChannel, new_request, request_approval


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
    assert DialogChannel(session).available() is expected


@pytest.mark.asyncio
async def test_url_only_skips_form_and_preserves_fallback_denial():
    class Session:
        client_capabilities = ClientCapabilities(elicitation={"url": {}})

        async def elicit_form(self, *args, **kwargs):
            raise AssertionError("URL-only client was asked to show a form")

    class Fallback:
        kind = "bot"

        def available(self):
            return True

        async def ask(self, request, timeout):
            return "declined"

    request = new_request(tool="fixture", account="test", chat="1", effect="test", reasons=[])
    assert await request_approval(request, [DialogChannel(Session()), Fallback()], timeout=1) == (
        "declined",
        "bot",
        [],
    )


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
