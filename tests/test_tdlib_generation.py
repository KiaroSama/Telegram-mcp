"""A TDLib client is pinned to the account generation that asked for it.

TDLib holds an exclusive SQLite database and the secret-chat keys inside it
cannot be re-derived, so two questions decide everything here: is this still the
account it was started for, and is the previous owner definitely gone.

Four ways that came apart, each below:

* **The Telethon client was captured before the wait and never re-checked.**
  `secret_client` read the active client, then blocked on the registry lock and
  on a native start - during which a reload can replace that account. It
  returned the client for the account that USED to be there.
* **The shutdown latch was read outside the lock.** A caller that passed the
  check then queued on the lock and started a new native client after shutdown
  had begun.
* **A failed close of the old client was swallowed, and a new one started
  against the same database anyway.**
* **`close(timeout=T)` could spend 2T.** T on the request, then T again on the
  Closed event, so a shutdown budget bought half what it said.

Everything native is doubled; what is real is the ordering, the locking and the
budget arithmetic, which is what all four defects were about.
"""

import asyncio
import time

import pytest

from telegram_mcp import tdlib_registry as reg


class _Telethon:
    def __init__(self, name):
        self.name = name


class _Native:
    """A TDLib client double: records what was asked of it and when."""

    instances = []

    def __init__(self, account):
        self.account = account
        self.closed = False
        self.close_calls = 0
        self.start_state = "authorizationStateReady"
        self.close_delay = 0
        self.close_error = None
        _Native.instances.append(self)

    async def start(self):
        return self.start_state

    async def close(self, timeout=None):
        self.close_calls += 1
        if self.close_delay:
            await asyncio.sleep(self.close_delay)
        if self.close_error:
            raise self.close_error
        self.closed = True

    def is_running(self):
        return not self.closed


@pytest.fixture(autouse=True)
def _clean(monkeypatch):
    # A module-level `asyncio.Lock()` binds to the loop that first awaits it, and
    # every test here gets a fresh one - so a suite that actually CONTENDS for
    # this lock (the budget case below does) fails in the next test with "bound
    # to a different event loop". One process has one loop, so this is a test
    # fixture rather than a defect in the registry.
    monkeypatch.setattr(reg, "_by_account_lock", asyncio.Lock())
    _Native.instances = []
    reg._by_account.clear()
    reg._verified_against.clear()
    reg._closing = False
    monkeypatch.setattr(reg, "_is_usable", lambda client: not client.closed)
    yield
    reg._by_account.clear()
    reg._verified_against.clear()
    reg._closing = False


def _wire(monkeypatch, active, *, verify=None, hold=None):
    """Point the registry at doubles for Telethon, the native client and identity."""
    import telegram_mcp.connection as conn
    import telegram_mcp.tdlib as tdlib
    import telegram_mcp.tdlib_identity as identity

    monkeypatch.setattr(conn, "get_client", lambda account=None: active[account])
    monkeypatch.setattr(tdlib, "TDLibClient", _Native)

    async def _verify(account, client, telethon):
        if verify is not None:
            await verify(account, client, telethon)

    monkeypatch.setattr(identity, "verify_owner", _verify)
    if hold is not None:
        real_start = _Native.start

        async def _slow_start(self):
            await hold.wait()
            return await real_start(self)

        monkeypatch.setattr(_Native, "start", _slow_start)


# --- F03: the generation --------------------------------------------------------


@pytest.mark.asyncio
async def test_a_reload_during_start_refuses_rather_than_serving_the_old_account(monkeypatch):
    """The counterexample: the label moved from one user to another while TDLib
    was starting, and the caller got the first user's client back."""
    active = {"work": _Telethon("user-111")}
    release = asyncio.Event()
    _wire(monkeypatch, active, hold=release)

    asked = asyncio.ensure_future(reg.secret_client("work"))
    await asyncio.sleep(0)
    active["work"] = _Telethon("user-222")  # the reload lands mid-start
    release.set()

    with pytest.raises(Exception) as raised:
        await asyncio.wait_for(asked, timeout=5)
    assert "reconfigured" in str(raised.value).lower() or "generation" in str(raised.value).lower()


@pytest.mark.asyncio
async def test_an_unchanged_generation_is_served_normally(monkeypatch):
    active = {"work": _Telethon("user-111")}
    _wire(monkeypatch, active)

    client = await asyncio.wait_for(reg.secret_client("work"), timeout=5)

    assert client.account == "work"
    assert reg._by_account["work"] is client


@pytest.mark.asyncio
async def test_shutdown_beginning_during_the_lock_wait_refuses(monkeypatch):
    """`_closing` was read before the lock, so a queued caller started a native
    client against a database the shutdown was already flushing."""
    active = {"work": _Telethon("user-111")}
    _wire(monkeypatch, active)

    async with reg._by_account_lock:
        asked = asyncio.ensure_future(reg.secret_client("work"))
        await asyncio.sleep(0)
        reg._closing = True

    with pytest.raises(Exception, match="shutting down"):
        await asyncio.wait_for(asked, timeout=5)
    assert _Native.instances == [], "a native client was started after shutdown began"


@pytest.mark.asyncio
async def test_a_replacement_is_refused_while_the_old_client_will_not_close(monkeypatch):
    """Starting a second native client against a database the first may still
    hold is the one thing this registry exists to prevent."""
    active = {"work": _Telethon("user-111")}
    _wire(monkeypatch, active)
    stuck = _Native("work")
    stuck.close_error = RuntimeError("the database is still checkpointing")
    reg._by_account["work"] = stuck
    reg._verified_against["work"] = _Telethon("user-000")  # a replaced generation

    before = len(_Native.instances)
    with pytest.raises(Exception, match="did not close"):
        await asyncio.wait_for(reg.secret_client("work"), timeout=5)

    assert len(_Native.instances) == before, "a second client was started on the same database"


# --- F04: budgets ---------------------------------------------------------------


@pytest.mark.asyncio
async def test_the_shutdown_budget_covers_the_lock_wait(monkeypatch):
    """The deadline started AFTER the registry lock was taken, so time spent
    waiting for it was free and the budget overran."""
    active = {"work": _Telethon("user-111")}
    _wire(monkeypatch, active)
    slow = _Native("work")
    slow.close_delay = 5
    reg._by_account["work"] = slow

    held = asyncio.Event()

    async def _hold_the_lock():
        async with reg._by_account_lock:
            held.set()
            await asyncio.sleep(0.3)

    holder = asyncio.ensure_future(_hold_the_lock())
    await asyncio.wait_for(held.wait(), timeout=2)

    started = time.monotonic()
    await asyncio.wait_for(reg.close_all(budget=0.5), timeout=5)
    elapsed = time.monotonic() - started

    assert elapsed < 1.5, f"the budget did not cover the lock wait ({elapsed:.2f}s)"
    await holder


@pytest.mark.asyncio
async def test_every_client_is_asked_to_close_even_when_the_first_stalls(monkeypatch):
    """Sequential closure let one stalled client spend the whole budget, so the
    others were never asked at all - and an unasked client is unflushed keys."""
    active = {name: _Telethon(name) for name in ("a", "b", "c")}
    _wire(monkeypatch, active)
    stalled, second, third = _Native("a"), _Native("b"), _Native("c")
    stalled.close_delay = 10
    reg._by_account.update({"a": stalled, "b": second, "c": third})

    await asyncio.wait_for(reg.close_all(budget=0.5), timeout=6)

    assert second.close_calls == 1, "the second account was never asked to close"
    assert third.close_calls == 1, "the third account was never asked to close"


@pytest.mark.asyncio
async def test_a_stalled_close_is_reported_not_silently_dropped(monkeypatch):
    active = {"a": _Telethon("a")}
    _wire(monkeypatch, active)
    stalled = _Native("a")
    stalled.close_delay = 10
    reg._by_account["a"] = stalled

    failures = await asyncio.wait_for(reg.close_all(budget=0.3), timeout=6)

    assert [name for name, _ in failures] == ["a"]
    assert "a" in reg._by_account, "an unconfirmed close dropped the client from the registry"


# --- F04: close() spends ONE budget, and Closed settles what is in flight -------


@pytest.mark.asyncio
async def test_close_spends_one_budget_not_two(monkeypatch):
    """`timeout` on the request and `timeout` again on the Closed event meant a
    caller that asked for T could wait 2T."""
    from telegram_mcp import tdlib

    client = tdlib.TDLibClient.__new__(tdlib.TDLibClient)
    client.account = "work"
    client._client_id = 7
    client._closed = None
    client.authorization_state = "authorizationStateReady"
    client._pending = {}

    async def _never_answers(payload, timeout=None):
        await asyncio.sleep(timeout)
        raise TimeoutError("no answer")

    monkeypatch.setattr(client, "request", _never_answers)

    started = time.monotonic()
    with pytest.raises(TimeoutError):
        await asyncio.wait_for(client.close(timeout=0.4), timeout=5)
    elapsed = time.monotonic() - started

    assert elapsed < 0.75, f"close spent two budgets, not one ({elapsed:.2f}s)"
    assert client._client_id == 7, "an unconfirmed close unregistered the client"


@pytest.mark.asyncio
async def test_an_unsolicited_closed_settles_requests_in_flight():
    """The session is terminated from another device. Every pending call used to
    wait out its own timeout reporting a network stall for a client that had
    already ended."""
    from telegram_mcp import tdlib

    client = tdlib.TDLibClient.__new__(tdlib.TDLibClient)
    client.account = "work"
    client._client_id = 7
    client._closed = None
    client._state_changed = None
    client.authorization_state = "authorizationStateReady"
    waiting = asyncio.get_running_loop().create_future()
    client._pending = {"1": waiting}

    client._on_authorization({"@type": "authorizationStateClosed"})

    assert waiting.done(), "a request was left in flight by a client that had closed"
    with pytest.raises(RuntimeError, match="closed"):
        waiting.result()
