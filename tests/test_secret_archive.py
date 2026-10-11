"""One-time archive policy at the durable storage seam."""

import pytest


@pytest.mark.asyncio
async def test_default_on_then_explicit_off_survives_restart(tmp_path, monkeypatch):
    from telethon_secret_chat import FileStorage
    from telegram_mcp import secret_archive

    destination = tmp_path / "private archive"

    async def resolve(*args):
        return destination, None

    monkeypatch.setattr(secret_archive.file_roots, "resolve_allowed_folder", resolve)

    async def roots(*args):
        return [tmp_path]

    monkeypatch.setattr(secret_archive.file_roots, "_get_effective_allowed_roots", roots)
    store = FileStorage(tmp_path / "protocol.json")
    setting = await secret_archive.prepare(store, "acct")
    assert setting == {"folder": str(destination), "on": True}
    secret_archive.disable(store)
    restarted = FileStorage(tmp_path / "protocol.json")
    assert await secret_archive.prepare(restarted, "acct") == {
        "folder": str(destination),
        "on": False,
    }


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["acl", "commit"])
async def test_archive_failure_publishes_no_marker_or_on_policy(tmp_path, monkeypatch, failure):
    from telethon_secret_chat import FileStorage
    from telegram_mcp import secret_archive

    store = FileStorage(tmp_path / "protocol.json")
    destination = tmp_path / "archive"

    async def resolve(*args):
        return destination, None

    async def roots(*args):
        return [tmp_path]

    monkeypatch.setattr(secret_archive.file_roots, "resolve_allowed_folder", resolve)
    monkeypatch.setattr(secret_archive.file_roots, "_get_effective_allowed_roots", roots)
    if failure == "acl":
        monkeypatch.setattr(secret_archive, "restrict_to_owner_strict", lambda path: False)
    else:

        def fail():
            raise OSError("synthetic durable commit failure")

        monkeypatch.setattr(store, "_write", fail)
    with pytest.raises(OSError):
        await secret_archive.prepare(store, "acct")
    assert store.load_setting(secret_archive.MARKER) is None
    assert store.load_setting("auto_save_secret_chats") is None
    reopened = FileStorage(tmp_path / "protocol.json")
    assert reopened.load_setting(secret_archive.MARKER) is None
    assert reopened.load_setting("auto_save_secret_chats") is None


@pytest.mark.asyncio
async def test_custom_destination_is_preserved_even_when_previous_choice_was_off(
    tmp_path, monkeypatch
):
    from telethon_secret_chat import FileStorage
    from telegram_mcp import secret_archive

    store = FileStorage(tmp_path / "protocol.json")
    chosen = tmp_path / "chosen"
    store.save_setting("auto_save_secret_chats", {"folder": str(chosen), "on": False})
    observed = []

    async def private(folder, account):
        observed.append(folder)
        return chosen

    monkeypatch.setattr(secret_archive, "private_destination", private)
    assert await secret_archive.prepare(store, "acct") == {"folder": str(chosen), "on": True}
    assert observed == [str(chosen)]


@pytest.mark.asyncio
async def test_cold_stop_persists_off_without_constructing_a_manager(tmp_path, monkeypatch):
    from telethon_secret_chat import FileStorage
    from telegram_mcp import secret_backend as backend, secret_control, secret_archive

    client = object()
    store = FileStorage(tmp_path / "protocol.json")
    monkeypatch.setattr(backend, "_telethon_client", lambda account: client)
    monkeypatch.setattr(backend, "_check_current", lambda *args: None)
    monkeypatch.setattr(backend, "_by_account", {})
    monkeypatch.setattr(backend, "_starts", {})
    monkeypatch.setattr(backend, "_storage_for", lambda account: store)

    async def no_op(*args):
        pass

    monkeypatch.setattr(backend, "_claim_store", no_op)
    monkeypatch.setattr(backend, "_bind_store", no_op)
    monkeypatch.setattr(backend, "_release_store", lambda account: None)

    def forbidden(*args):
        raise AssertionError("cold stop must not start a manager")

    monkeypatch.setattr(backend, "_manager", forbidden)
    assert await secret_control.stop_archive("acct") == {"auto_save": False}
    reopened = FileStorage(tmp_path / "protocol.json")
    assert await secret_archive.prepare(reopened, "acct") == {"folder": None, "on": False}


@pytest.mark.asyncio
async def test_cold_stop_interrupts_activation_before_archive_recovery(tmp_path, monkeypatch):
    import asyncio
    from telethon_secret_chat import FileStorage
    from telegram_mcp import secret_backend as backend, secret_control, secret_archive
    from test_secret_backend import _Client, _wire

    client = _Client()
    _wire(monkeypatch, backend, client, tmp_path)
    for name in ("_by_account", "_verified_against", "_starts", "_storages"):
        monkeypatch.setattr(backend, name, {})
    monkeypatch.setattr(secret_control, "_locks", {})
    monkeypatch.setattr(secret_control, "_operations", {})
    monkeypatch.setattr(secret_control, "_archive_stops", set())
    storage = FileStorage(tmp_path / "store.json")
    monkeypatch.setattr(backend, "_storage_for", lambda account: storage)
    entered = asyncio.Event()
    release = asyncio.Event()

    async def delayed_bind(*args):
        entered.set()
        await release.wait()

    monkeypatch.setattr(backend, "_bind_store", delayed_bind)
    activation = asyncio.create_task(backend.secret_manager("acct"))
    await asyncio.wait_for(entered.wait(), 1)
    stopping = asyncio.create_task(secret_control.stop_archive("acct"))
    await asyncio.sleep(0)
    release.set()
    try:
        assert await asyncio.wait_for(stopping, 2) == {"auto_save": False}
        assert (await asyncio.gather(activation, return_exceptions=True))[
            0
        ].__class__ is asyncio.CancelledError
        assert await secret_archive.prepare(storage, "acct") == {"folder": None, "on": False}
        assert client.handlers == []
    finally:
        await backend.close_all()


@pytest.mark.asyncio
async def test_package_created_archive_subdirectories_survive_restart(tmp_path, monkeypatch):
    from telethon_secret_chat import FileStorage, SecretChatManager
    from telegram_mcp import secret_archive
    from test_secret_custody import OfflineClient

    archive = tmp_path / "archive"

    async def resolve(*args):
        return archive, None

    async def roots(*args):
        return [tmp_path]

    monkeypatch.setattr(secret_archive.file_roots, "resolve_allowed_folder", resolve)
    monkeypatch.setattr(secret_archive.file_roots, "_get_effective_allowed_roots", roots)
    store = FileStorage(tmp_path / "store.json")
    await secret_archive.prepare(store, "acct")
    manager = SecretChatManager(OfflineClient(), store)
    await manager.start()
    # Actual package AutoSave writes the inherited chat/files directory structure.
    manager._autosave._append(701, {"type": "message", "id": 43, "text": "retained"})
    await manager.stop()
    setting = await secret_archive.prepare(FileStorage(tmp_path / "store.json"), "acct")
    assert setting["on"] is True
    assert (archive / "701" / "messages.jsonl").read_text(encoding="utf-8").find("retained") >= 0
