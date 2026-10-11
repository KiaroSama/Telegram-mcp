"""Serialize backend selection and archive controls with owned startup."""

import asyncio
from telegram_mcp import secret_archive, secret_backend_config

_locks = {}
_operations = {}
_archive_stops = set()


def _lock(account):
    return _locks.setdefault(account, asyncio.Lock())


async def manager(account):
    from telegram_mcp import secret_backend as backend

    async with _lock(account):
        operation = _operations.get(account)
        if operation is not None:
            await asyncio.shield(operation)
        return await backend._manager(account)


async def _settle_start(account):
    from telegram_mcp import secret_backend as backend

    task = backend._starts.get(account)
    if task is not None:
        try:
            await asyncio.shield(task)
        except asyncio.CancelledError:
            if not task.cancelled():
                raise


async def _owned(account, action):
    async with _lock(account):
        previous = _operations.get(account)
        if previous is not None:
            await asyncio.shield(previous)
        task = asyncio.create_task(action())
        _operations[account] = task

        def finished(done):
            if _operations.get(account) is done:
                _operations.pop(account, None)
            if not done.cancelled():
                done.exception()

        task.add_done_callback(finished)
        return await asyncio.shield(task)


async def configure_backend(account, mode, **budgets):
    from telegram_mcp import secret_backend as backend

    policy = secret_backend_config.make(mode, **budgets)

    async def change():
        async with asyncio.timeout(backend._START_SECONDS):
            client = backend._telethon_client(account)
            backend._check_current(account, client)
            await _settle_start(account)
            await backend._claim_store(account)
            try:
                await backend._bind_store(account, client)
                existing = backend._by_account.get(account)
                storage = (
                    backend._storages[id(existing)]
                    if existing is not None
                    else backend._storage_for(account)
                )
                old = secret_backend_config.load(storage)
                if old == policy:
                    return old
                secret_backend_config.require_empty(storage)
                if existing is not None:
                    await asyncio.shield(backend._stop_owned(account, existing))
                    await backend._claim_store(account)
                    storage = backend._storage_for(account)
                backend._check_current(account, client)
                secret_backend_config.save_empty(storage, policy)
                await backend._manager(account)
                return policy
            finally:
                if account not in backend._by_account:
                    backend._release_store(account)

    return await _owned(account, change)


async def stop_archive(account):
    from telegram_mcp import secret_backend as backend

    # Stop intent must interrupt a cold activation before waiting behind its lock.
    _archive_stops.add(account)
    initializer = backend._starts.get(account)
    if initializer is not None:
        initializer.cancel()

    async def stop():
        async with asyncio.timeout(backend._START_SECONDS):
            client = backend._telethon_client(account)
            backend._check_current(account, client)
            await _settle_start(account)
            await backend._claim_store(account)
            try:
                await backend._bind_store(account, client)
                live = backend._by_account.get(account)
                storage = (
                    backend._storages[id(live)]
                    if live is not None
                    else backend._storage_for(account)
                )
                # Public manager.stop cancels saver tasks; stopping only the saver
                # settles downloads before disabling and leaves an ingress window.
                if live is not None:
                    await asyncio.shield(backend._stop_owned(account, live))
                    await backend._claim_store(account)
                    storage = backend._storage_for(account)
                backend._check_current(account, client)
                secret_archive.disable(storage)
                _archive_stops.discard(account)
                if live is not None:
                    restarted = await backend._manager(account)
                    if restarted.auto_save_secret_chats is not None:
                        raise RuntimeError("Archive stop is unconfirmed")
            finally:
                if account not in backend._by_account:
                    backend._release_store(account)
            return {"auto_save": False}

    return await _owned(account, stop)


async def start_archive(account, folder, ctx=None):
    from telegram_mcp import secret_backend as backend

    path = await secret_archive.private_destination(folder, account, ctx)

    async def start():
        live = await backend._manager(account)
        secret_archive.pin(live, path)
        await live.start_auto_save_secret_chats(path)
        return {"auto_save": True, "folder": str(path), "owner_only": True}

    return await _owned(account, start)


async def backend_status(account):
    from telegram_mcp import secret_backend as backend

    live = await manager(account)
    policy = secret_backend_config.load(backend._storages[id(live)])
    bounded = isinstance(live, backend.BoundedSecretChatManager)
    return {
        "mode": policy["mode"],
        "limits": policy["limits"],
        "archive": {
            "on": live.auto_save_secret_chats is not None,
            "folder": live.auto_save_secret_chats,
        },
        "bounded": bounded,
        "close_semantics": "local preserving nonresumable stop" if bounded else "remote close",
        "archive_is_independent": True,
        "custody_is_encrypted_at_rest": False,
        "quotas_bound_process_ram": False,
        "quotas_bound_archive_disk": False,
    }


async def stop_operations():
    tasks = set(_operations.values())
    for task in tasks:
        task.cancel()
    if tasks:
        await asyncio.wait(tasks, timeout=5)
