"""TDLib's lifecycle: what a failed send leaves behind, and what shutdown owes.

Three defects met here, and they share a shape - something that only looks
finished. A request registered its future before the send that might fail. A
cached client was judged alive by an id it keeps after dying. And `close_all`,
the function whose docstring says "called on server shutdown", was called by
nothing at all, so every run exited without flushing the secret-chat keys TDLib
writes lazily - keys that cannot be re-derived.

No native library and no sleeps: the doubles raise and resolve on demand.
"""

import asyncio

import pytest

from telegram_mcp import tdlib
from telegram_mcp import tdlib_registry as reg


class _FakeClient:
    """Stands in for a started TDLibClient."""

    def __init__(self, account="acc", state="authorizationStateReady", refuse_close=False):
        self.account = account
        self.authorization_state = state
        self._client_id = 1
        self.closed = 0
        self.refuse_close = refuse_close

    async def close(self):
        self.closed += 1
        if self.refuse_close:
            raise OSError("the database would not flush")
        self._client_id = None


@pytest.fixture(autouse=True)
def _empty_registry(monkeypatch):
    monkeypatch.setattr(reg, "_by_account", {})
    monkeypatch.setattr(reg, "_by_account_lock", asyncio.Lock())


# --- R13: a send that fails must not leave its future behind ----------------


def _client_for_request(send):
    client = tdlib.TDLibClient.__new__(tdlib.TDLibClient)
    client._client_id = 1
    client._pending = {}
    client._loop = asyncio.get_running_loop()
    client._send = send
    return client


@pytest.mark.asyncio
async def test_a_send_that_raises_leaves_no_pending_entry():
    """`_send` serialises to JSON and calls into the native library; either can
    raise. Registered above the try, the entry was orphaned for the life of a
    client that lives as long as the server."""

    def _explode(_payload):
        raise ValueError("Object of type bytes is not JSON serializable")

    client = _client_for_request(_explode)

    with pytest.raises(ValueError):
        await client.request({"@type": "getMe"})

    assert client._pending == {}, "the abandoned future is still registered"


@pytest.mark.asyncio
async def test_the_abandoned_future_is_cancelled_not_left_pending():
    """Nothing will ever resolve it - no reply carries that `@extra`. Left
    pending it warns at interpreter exit and buries the real error."""
    captured = {}

    def _explode(payload):
        captured["future"] = client._pending[payload["@extra"]]
        raise RuntimeError("native send failed")

    client = _client_for_request(_explode)

    with pytest.raises(RuntimeError):
        await client.request({"@type": "getMe"})

    assert captured["future"].cancelled()


@pytest.mark.asyncio
async def test_an_ordinary_request_still_answers_and_cleans_up():
    def _answer(payload):
        client._loop.call_soon(
            client._pending[payload["@extra"]].set_result, {"@type": "ok", "id": 7}
        )

    client = _client_for_request(_answer)

    assert await client.request({"@type": "getMe"}) == {"@type": "ok", "id": 7}
    assert client._pending == {}


@pytest.mark.asyncio
async def test_a_timeout_still_cleans_up():
    client = _client_for_request(lambda payload: None)

    with pytest.raises(TimeoutError):
        await client.request({"@type": "getMe"}, timeout=0.05)

    assert client._pending == {}


# --- R14: a cached client is judged by whether it can still serve -----------


@pytest.mark.parametrize(
    "state",
    ["authorizationStateClosed", "authorizationStateClosing", "authorizationStateLoggingOut"],
)
@pytest.mark.asyncio
async def test_a_client_that_died_on_its_own_is_not_handed_back(state, monkeypatch):
    """The session is terminated from another device, or TDLib closes itself
    after an error: `_client_id` is still set and the cache returned a corpse."""
    dead = _FakeClient(state=state)
    reg._by_account["acc"] = dead
    fresh = _FakeClient()
    monkeypatch.setattr(reg, "TDLibClient", lambda account: fresh)
    fresh.start = lambda: _ready()

    got = await reg.secret_client("acc")

    assert got is fresh, "the dead client was handed back"
    assert dead.closed == 1, "and it was never closed"


@pytest.mark.asyncio
async def test_a_healthy_cached_client_is_reused():
    alive = _FakeClient()
    reg._by_account["acc"] = alive

    assert await reg.secret_client("acc") is alive
    assert alive.closed == 0


async def _ready():
    return "authorizationStateReady"


@pytest.mark.asyncio
async def test_a_start_that_raises_closes_what_it_brought_up(monkeypatch):
    """`start()` registers a native client with the reader thread; abandoning it
    leaked both for the life of the process."""
    built = _FakeClient()

    async def _fail():
        raise ConnectionError("tdlib would not come up")

    built.start = _fail
    monkeypatch.setattr(reg, "TDLibClient", lambda account: built)

    with pytest.raises(ConnectionError):
        await reg.secret_client("acc")

    assert built.closed == 1
    assert "acc" not in reg._by_account


@pytest.mark.asyncio
async def test_a_cancelled_start_also_closes_what_it_brought_up(monkeypatch):
    built = _FakeClient()

    async def _cancelled():
        raise asyncio.CancelledError()

    built.start = _cancelled
    monkeypatch.setattr(reg, "TDLibClient", lambda account: built)

    with pytest.raises(asyncio.CancelledError):
        await reg.secret_client("acc")

    assert built.closed == 1, "cancellation leaked the native client"


@pytest.mark.asyncio
async def test_an_unauthorised_start_is_refused_and_closed(monkeypatch):
    built = _FakeClient()

    async def _waiting():
        return "authorizationStateWaitPhoneNumber"

    built.start = _waiting
    monkeypatch.setattr(reg, "TDLibClient", lambda account: built)

    with pytest.raises(tdlib.NotSignedIn):
        await reg.secret_client("acc")

    assert built.closed == 1
    assert "acc" not in reg._by_account


# --- R14 / S01: shutdown closes every client and says what failed -----------


@pytest.mark.asyncio
async def test_one_client_refusing_to_close_does_not_strand_the_rest():
    """A `for` loop over `await close()` abandoned the rest after the first
    refusal - which loses exactly the keys this function exists to flush."""
    stubborn = _FakeClient("first", refuse_close=True)
    willing = _FakeClient("second")
    reg._by_account.update({"first": stubborn, "second": willing})

    failures = await reg.close_all()

    assert willing.closed == 1, "the second client was never closed"
    assert [account for account, _ in failures] == ["first"]
    assert reg._by_account == {}, "the registry was left holding closed clients"


@pytest.mark.asyncio
async def test_a_clean_shutdown_reports_nothing_and_empties_the_registry():
    reg._by_account["acc"] = _FakeClient()

    assert await reg.close_all() == []
    assert reg._by_account == {}


def test_the_server_shutdown_actually_closes_tdlib():
    """S01, and the whole reason the rest of this file matters: `close_all` was
    defined, exported and documented as "called on server shutdown", and nothing
    called it. Proved by reading the shutdown path, not by assuming one exists."""
    import inspect

    from telegram_mcp import runner

    shutdown = inspect.getsource(runner._main)

    assert "close_all" in shutdown, "the shutdown path still never closes TDLib"
    assert shutdown.index("close_all") < shutdown.index(
        "lock.release()"
    ), "TDLib must be closed before the session locks go"
