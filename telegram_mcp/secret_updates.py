"""Recover missing secret-state pushes through the client's native update loop."""

import asyncio
import logging

from telethon.tl import functions

from telegram_mcp.safe_log import log_event


class SecretUpdateRecovery:
    INTERVAL = 5.0
    REQUEST_TIMEOUT = 10.0

    def __init__(self, client, manager, is_current, account):
        self.client = client
        self.manager = manager
        self.is_current = is_current
        self.account = account
        self._wake = asyncio.Event()
        self._task = None
        self._stopped = False

    def _active(self):
        return any(chat.state.value != "closed" for chat in self.manager.list())

    def start(self):
        if self._task is None and not self._stopped:
            for name in ("ChatRequested", "ChatReady", "ChatClosed"):
                self.manager.on(name, self.refresh)
            self._task = asyncio.create_task(self._run())
        return self._task

    def refresh(self, _event=None):
        self._wake.set()

    async def stop(self):
        self._stopped = True
        if self._task is not None:
            self._task.cancel()
            await asyncio.gather(self._task, return_exceptions=True)

    async def check(self):
        if self._stopped or not self.is_current() or not self._active():
            return False
        if not self.client.is_connected():
            return False
        box = self.client._message_box
        if (
            self.client._no_updates
            or box.is_empty()
            or box.getting_diff_for
            or not self.client._updates_queue.empty()
        ):
            return False
        async with asyncio.timeout(self.REQUEST_TIMEOUT):
            remote = await self.client(functions.updates.GetStateRequest())
            if self._stopped or not self.is_current() or not self._active():
                return False
            if (
                box is not self.client._message_box
                or box.getting_diff_for
                or not self.client._updates_queue.empty()
            ):
                return False
            local = box.session_state()[0]
            if any(getattr(remote, key) > local[key] for key in ("pts", "qts", "seq")):
                # The queue marker alone starts recovery. Reserving the box as well
                # could fetch once before the marker, then again when it is consumed.
                await self.client.catch_up()
                log_event(logging.INFO, "secret update recovery requested", account=self.account)
                return True
        return False

    async def _run(self):
        delay = self.INTERVAL
        failed = False
        next_check = 0.0
        loop = asyncio.get_running_loop()
        while not self._stopped and self.is_current():
            self._wake.clear()
            if not self._active():
                # Outbound requests emit no event until acceptance. Rescan locally;
                # an idle manager must make no Telegram requests.
                try:
                    await asyncio.wait_for(self._wake.wait(), self.INTERVAL)
                except asyncio.TimeoutError:
                    pass
                continue
            remaining = next_check - loop.time()
            if remaining > 0:
                try:
                    await asyncio.wait_for(self._wake.wait(), remaining)
                except asyncio.TimeoutError:
                    pass
                continue
            try:
                await self.check()
                if failed:
                    log_event(
                        logging.INFO, "secret update state check recovered", account=self.account
                    )
                failed = False
                delay = self.INTERVAL
            except Exception as error:
                if not failed:
                    log_event(
                        logging.WARNING,
                        "secret update state check failed",
                        account=self.account,
                        error=error,
                    )
                failed = True
                delay = max(min(delay * 2, 60.0), getattr(error, "seconds", 0))
            # Chat events wake an idle worker, not a rate limit or failure backoff.
            next_check = loop.time() + delay
