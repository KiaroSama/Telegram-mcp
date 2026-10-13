"""Readiness is diagnostic, never an implicit strong-mode cutover."""

from types import SimpleNamespace

import pytest


@pytest.mark.asyncio
async def test_existing_backend_status_reports_strong_without_activating(monkeypatch):
    from telegram_mcp import secret_backend as backend, secret_control

    live = SimpleNamespace(auto_save_secret_chats="synthetic archive")
    store = SimpleNamespace(load_setting=lambda name: None)

    async def manager(account):
        assert account == "acct"
        return live

    monkeypatch.setattr(secret_control, "manager", manager)
    monkeypatch.setattr(backend, "_storages", {id(live): store})
    result = await secret_control.backend_status("acct")
    assert result["mode"] == "normal"
    assert result["archive"] == {"on": True, "folder": "synthetic archive"}
    readiness = result["strong_ingress"]
    assert readiness["integrated"] is False
    assert readiness["active"] is False
    assert readiness["activation_ready"] is False
    assert readiness["requires_package_archive_off"] is True
    assert readiness["backpressure_scope"] == "all same-account RPCs"
    assert "verified_cutover_baseline_required" in readiness["blockers"]


@pytest.mark.parametrize("available,valid", [(False, False), (True, False), (True, True)])
def test_prerequisite_verification_never_implies_activation(monkeypatch, available, valid):
    from telegram_mcp import secret_ingress_status, secret_backend as backend

    calls = []

    def verify():
        calls.append("verify")
        if not valid:
            raise RuntimeError("synthetic private detail must not escape")

    monkeypatch.setattr(backend, "verify_ingress_prerequisite", verify, raising=False)
    monkeypatch.setattr(
        backend, "StrongSecretChatManager", object if available else None, raising=False
    )
    result = secret_ingress_status.readiness()
    assert result["library_available"] is available
    assert result["prerequisite_verified"] is (available and valid)
    assert result["activation_ready"] is False
    assert calls == (["verify"] if available else [])
    assert "synthetic private" not in str(result)


@pytest.mark.asyncio
async def test_unsupported_strong_configuration_refuses_before_client_lookup(monkeypatch):
    from telegram_mcp import secret_backend as backend, secret_control

    def forbidden(account):
        pytest.fail("unsupported mode reached account state")

    monkeypatch.setattr(backend, "_telethon_client", forbidden)
    with pytest.raises(ValueError, match="normal or bounded"):
        await secret_control.configure_backend("acct", "strong")
