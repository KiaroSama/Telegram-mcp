"""Own secret-manager activation and idle custody checks on admitted clients."""

import asyncio
import logging
from telegram_mcp.safe_log import log_event

_tasks = set()
_sweep = None
_closing = False
_failures = {}


async def activate(account, client):
    from telegram_mcp import connection, secret_backend

    if _closing or connection.clients.get(account) is not client:
        return
    try:
        await secret_backend.secret_manager(account)
        _failures.pop(account, None)
    except Exception as error:
        _failures[account] = type(error).__name__
        log_event(logging.ERROR, "secret archive activation failed", account=account, error=error)


async def _retire(account, manager, start):
    from telegram_mcp import secret_backend as backend, secret_control

    # Capture ownership at notification time, never re-read a reused label later.
    if start is not None:
        start.cancel()
        await asyncio.wait({start}, timeout=backend._CLEANUP_SECONDS)
    async with secret_control._lock(account):
        if manager is not None and backend._by_account.get(account) is manager:
            await asyncio.wait(
                {backend._stop_owned(account, manager)}, timeout=backend._CLEANUP_SECONDS
            )


def _own(coroutine):
    task = asyncio.create_task(coroutine)
    _tasks.add(task)

    def finished(done):
        _tasks.discard(done)
        if not done.cancelled() and done.exception() is not None:
            log_event(logging.ERROR, "secret activation task failed", error=done.exception())

    task.add_done_callback(finished)


def changed(added, removed):
    from telegram_mcp import connection

    if _closing:
        return
    from telegram_mcp import secret_backend as backend

    for account in removed:
        _own(_retire(account, backend._by_account.get(account), backend._starts.get(account)))
    for account in added:
        client = connection.clients.get(account)
        if client is not None:
            _own(activate(account, client))


async def _custody_sweep():
    from telegram_mcp import secret_backend as backend

    while not _closing:
        for manager in list(backend._by_account.values()):
            if (
                isinstance(manager, backend.BoundedSecretChatManager)
                and id(manager) not in backend._stops
            ):
                try:
                    manager.check_custody()
                except Exception as error:
                    log_event(logging.WARNING, "secret custody sweep failed", error=error)
        await asyncio.sleep(5)


async def start(accounts):
    global _closing, _sweep
    from telegram_mcp import connection

    _closing = False
    connection.on_clients_changed(changed)
    await asyncio.gather(
        *(activate(account, client) for account, client in list(accounts.items()))
    )
    if _sweep is None or _sweep.done():
        _sweep = asyncio.create_task(_custody_sweep())


async def stop():
    global _closing, _sweep
    _closing = True
    tasks = set(_tasks)
    if _sweep is not None:
        tasks.add(_sweep)
    for task in tasks:
        task.cancel()
    if tasks:
        await asyncio.wait(tasks, timeout=5)
    _sweep = None
