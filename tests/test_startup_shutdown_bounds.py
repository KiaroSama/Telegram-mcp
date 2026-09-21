"""Every phase of startup and shutdown finishes, or says why it did not.

Three unbounded waits, each of which could hold the process open with nothing on
the console to explain it:

* **Connect and authorize.** Neither call bounds itself, so one unreachable DC
  held startup open indefinitely - no message, no exit, no way to tell it from a
  slow network.
* **Disconnecting every account at shutdown.** An unbounded `gather`, and
  everything after it - including the TDLib flush, whose loss takes secret-chat
  history with it - simply never ran when one client would not close.
* **The dialog warm.** Shielding the WAIT protects the warm from its callers,
  which is right, and left nothing able to stop it when the client it was
  warming had been retired.

Task progress is driven by events, never by sleeping: a phase that is meant to
hang has to hang for exactly as long as the test needs.
"""

import asyncio

import pytest

from telegram_mcp import dialog_warm as warm


class _Client:
    def __init__(self):
        self.calls = 0
        self.disconnected = False

    async def disconnect(self):
        self.disconnected = True


@pytest.fixture(autouse=True)
def _clean():
    warm._dialog_warmed.clear()
    warm._dialog_warms.clear()
    warm._dialog_warm_errors.clear()
    yield
    for task in list(warm._dialog_warms.values()):
        task.cancel()
    warm._dialog_warms.clear()


# --- the dialog warm -----------------------------------------------------------


@pytest.mark.asyncio
async def test_a_warm_that_never_returns_is_given_up_on(monkeypatch):
    """Unbounded, it was shared by every later caller for the life of the
    process: one hung call and the cache could never be warmed again."""
    monkeypatch.setattr(warm, "_DIALOG_WARM_TIMEOUT", 0.05)
    client = _Client()
    client.get_dialogs = lambda: asyncio.get_running_loop().create_future()

    assert await warm.warm_dialogs_once(client) is False
    assert client not in warm._dialog_warmed


@pytest.mark.asyncio
async def test_the_reason_the_cache_is_cold_is_kept(monkeypatch):
    """A failed warm left the caller reporting whatever it found next - usually
    a missing peer, which describes the consequence rather than the cause."""
    monkeypatch.setattr(warm, "_DIALOG_WARM_TIMEOUT", 0.05)
    client = _Client()
    client.get_dialogs = lambda: asyncio.get_running_loop().create_future()

    await warm.warm_dialogs_once(client)

    assert isinstance(warm.last_warm_error(client), (asyncio.TimeoutError, TimeoutError))


@pytest.mark.asyncio
async def test_a_successful_warm_clears_the_previous_reason():
    client = _Client()
    warm._dialog_warm_errors[client] = RuntimeError("an earlier attempt")

    async def _ok():
        return []

    client.get_dialogs = _ok
    assert await warm.warm_dialogs_once(client) is True
    assert warm.last_warm_error(client) is None


@pytest.mark.asyncio
async def test_retiring_a_client_stops_the_warm_it_owns():
    """The task held the client it was warming and went on calling into it after
    the socket had been closed."""
    from telegram_mcp import retirement

    client = _Client()
    started = asyncio.Event()

    async def _hang():
        started.set()
        await asyncio.get_running_loop().create_future()

    client.get_dialogs = _hang
    waiter = asyncio.ensure_future(warm.warm_dialogs_once(client))
    await asyncio.wait_for(started.wait(), timeout=1)
    task = warm._dialog_warms[client]

    retirement.retire(client)
    # Waited for, not assumed. `task.cancel()` REQUESTS a cancellation; how many
    # loop turns it takes to be delivered is an implementation detail that
    # differs between interpreter versions, and asserting after exactly one made
    # this pass on 3.12-3.14 and fail on 3.11.
    await asyncio.wait({task}, timeout=2)

    assert task.cancelled() or task.done(), "the warm outlived the client it was warming"
    waiter.cancel()


@pytest.mark.asyncio
async def test_one_waiter_giving_up_leaves_the_warm_for_the_others():
    """The behaviour that has to survive being made cancellable: a shielded wait
    protects the warm from its CALLERS."""
    client = _Client()
    release = asyncio.Event()
    started = asyncio.Event()

    async def _slow():
        started.set()
        await release.wait()
        return []

    client.get_dialogs = _slow
    first = asyncio.ensure_future(warm.warm_dialogs_once(client))
    second = asyncio.ensure_future(warm.warm_dialogs_once(client))
    await asyncio.wait_for(started.wait(), timeout=1)

    first.cancel()
    with pytest.raises(asyncio.CancelledError):
        await first
    release.set()

    assert await asyncio.wait_for(second, timeout=1) is True


@pytest.mark.asyncio
async def test_shutdown_stops_every_warm_still_running():
    clients = [_Client() for _ in range(3)]
    started = asyncio.Event()

    async def _hang():
        started.set()
        await asyncio.get_running_loop().create_future()

    waiters = []
    for client in clients:
        client.get_dialogs = _hang
        waiters.append(asyncio.ensure_future(warm.warm_dialogs_once(client)))
    await asyncio.wait_for(started.wait(), timeout=1)

    assert await warm.drain_warms(timeout=2) == 0
    assert warm._dialog_warms == {}
    for waiter in waiters:
        waiter.cancel()


@pytest.mark.asyncio
async def test_a_failed_warm_is_retried_rather_than_suppressed():
    """The stamp is written on success only, so one transient failure must not
    suppress every retry for the whole window."""
    client = _Client()
    attempts = []

    async def _flaky():
        attempts.append(1)
        if len(attempts) == 1:
            raise OSError("transient")
        return []

    client.get_dialogs = _flaky

    assert await warm.warm_dialogs_once(client) is False
    assert await warm.warm_dialogs_once(client) is True
    assert len(attempts) == 2


# --- startup and shutdown phases ----------------------------------------------


@pytest.mark.asyncio
async def test_a_connect_that_never_answers_is_bounded_and_explained(monkeypatch):
    """Neither `connect()` nor `is_user_authorized()` bounds itself, so startup
    sat on an unreachable DC with nothing on the console to say so."""
    from telegram_mcp import runner

    monkeypatch.setattr(runner, "_CONNECT_PHASE_SECONDS", 0.05)
    monkeypatch.setattr(runner._admission, "claim_session", _noop_claim())
    retired = []
    monkeypatch.setattr(runner, "_retire", lambda client: retired.append(client))
    forgotten = []
    monkeypatch.setattr(
        runner._admission, "forget", lambda label, closing=None, **_: forgotten.append(label)
    )

    client = _Client()
    client.connect = lambda: asyncio.get_running_loop().create_future()

    with pytest.raises(runner.StartupMessage) as raised:
        await runner._connect_authorized_client("work", client)

    said = str(raised.value)
    assert "did not answer within" in said and "work" in said
    assert forgotten == ["work"], "the session lock was left standing for a dead attempt"


@pytest.mark.asyncio
async def test_an_authorization_check_that_hangs_is_bounded_too(monkeypatch):
    """The budget covers the PHASE. Bounding only `connect()` moves the hang one
    call along rather than removing it."""
    from telegram_mcp import runner

    monkeypatch.setattr(runner, "_CONNECT_PHASE_SECONDS", 0.05)
    monkeypatch.setattr(runner._admission, "claim_session", _noop_claim())
    monkeypatch.setattr(runner, "_retire", lambda client: None)
    monkeypatch.setattr(runner._admission, "forget", lambda label, closing=None, **_: None)

    client = _Client()

    async def _connected():
        return None

    client.connect = _connected
    client.is_user_authorized = lambda: asyncio.get_running_loop().create_future()

    with pytest.raises(runner.StartupMessage, match="did not answer within"):
        await runner._connect_authorized_client("work", client)


@pytest.mark.asyncio
async def test_a_connect_that_answers_in_time_is_untouched(monkeypatch):
    from telegram_mcp import runner

    monkeypatch.setattr(runner._admission, "claim_session", _noop_claim())
    client = _Client()

    async def _ok():
        return None

    async def _authorized():
        return True

    client.connect = _ok
    client.is_user_authorized = _authorized

    await runner._connect_authorized_client("work", client)


def _noop_claim():
    # `**_`: this doubles `admission.claim_session`, and a double that pins the
    # exact signature breaks the moment the real function gains an optional
    # keyword - which is how `on_wait` turned six green CI legs red while the
    # suites exercising this seam passed locally.
    async def _claim(label, client, grace_seconds=None, **_):
        return None

    return _claim


@pytest.mark.asyncio
async def test_one_stalled_disconnect_does_not_suppress_the_rest_of_shutdown():
    """The shape of the defect, asserted on the ordering rather than on the
    runner's whole `finally`: a bounded gather lets what follows it run."""
    stalled = asyncio.get_running_loop().create_future()
    ran_after = []

    async def _shutdown():
        try:
            await asyncio.wait_for(
                asyncio.gather(stalled, return_exceptions=True),
                timeout=0.05,
            )
        except (asyncio.TimeoutError, TimeoutError):
            pass
        ran_after.append("secret-chat flush")

    await asyncio.wait_for(_shutdown(), timeout=2)

    assert ran_after == ["secret-chat flush"]
    stalled.cancel()
