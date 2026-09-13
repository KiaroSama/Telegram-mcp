"""How many feed consumers exist, and what the feed keeps.

The consumer CONSUMES settled bursts: one it takes is one
`wait_for_settled_message` will never see. So a second consumer is not untidy
bookkeeping, it is events answered twice or not at all - and there were three
ways to get one.

Every test here drives task progress with events rather than by sleeping: a
consumer that is deliberately refusing to stop must refuse for as long as the
test needs and not one moment of wall clock longer.
"""

import asyncio
import json
import os
import time
from pathlib import Path

import pytest

from telegram_mcp.tools import events_store as store
from telegram_mcp.tools import feed_lifecycle as lifecycle


@pytest.fixture(autouse=True)
def _clean(monkeypatch, tmp_path):
    monkeypatch.setattr(lifecycle, "_task", None)
    monkeypatch.setattr(lifecycle, "_stopping", None)
    monkeypatch.setattr(lifecycle, "_settle_ms", 6000)
    monkeypatch.setattr(lifecycle, "_autostart_done", False)
    monkeypatch.setattr(lifecycle, "_transition", asyncio.Lock())
    monkeypatch.setattr(store, "_last_rotation_error", None)
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_FILE", str(tmp_path / "feed.jsonl"))
    for name in ("TELEGRAM_EVENT_FEED_MAX_BYTES", "TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS"):
        monkeypatch.delenv(name, raising=False)
    yield
    for task in (lifecycle._task, lifecycle._stopping):
        if task is not None:
            task.cancel()


class _Consumer:
    """A consumer whose progress and whose refusal to stop are both controlled.

    `stop_allowed` is what makes a stop time out on demand: until it is set the
    coroutine swallows its cancellation and keeps running, which is exactly the
    live task the old code started a replacement on top of.
    """

    def __init__(self, obey_cancel: bool = True):
        self.started = asyncio.Event()
        self.ended = asyncio.Event()
        self.stop_allowed = asyncio.Event()
        self.obey_cancel = obey_cancel
        self.settles = []

    async def run(self, settle_ms: int) -> None:
        self.settles.append(settle_ms)
        self.started.set()
        try:
            while True:
                try:
                    await asyncio.sleep(3600)
                except asyncio.CancelledError:
                    if self.obey_cancel:
                        raise
                    # Refusing on purpose: wait to be released, uncancellable.
                    await asyncio.shield(self.stop_allowed.wait())
                    raise
        finally:
            self.ended.set()


async def _running(consumer, settle=100):
    outcome = await lifecycle.enable(settle, consumer.run)
    assert outcome["ok"], outcome
    await asyncio.wait_for(consumer.started.wait(), timeout=1)
    return outcome


# --- how many consumers -------------------------------------------------------


@pytest.mark.asyncio
async def test_a_changed_settle_interval_replaces_the_one_consumer():
    first = _Consumer()
    await _running(first, settle=100)
    task = lifecycle._task

    second = _Consumer()
    outcome = await lifecycle.enable(250, second.run)
    await asyncio.wait_for(second.started.wait(), timeout=1)

    assert outcome["ok"] and outcome["reason"] == "started"
    assert task.cancelled() or task.done(), "the old consumer was left running"
    assert lifecycle.settle_ms() == 250


@pytest.mark.asyncio
async def test_the_same_settle_interval_does_not_restart_anything():
    consumer = _Consumer()
    await _running(consumer, settle=100)
    task = lifecycle._task

    outcome = await lifecycle.enable(100, consumer.run)

    assert outcome == {"ok": True, "reason": "already-running"}
    assert lifecycle._task is task
    assert consumer.settles == [100], "it started a second consumer for no reason"


@pytest.mark.asyncio
async def test_a_consumer_that_will_not_stop_blocks_its_replacement(monkeypatch):
    """The defect. `enable` awaited the stop, ignored the False it returned, and
    started a replacement beside a consumer that was still taking bursts."""
    monkeypatch.setattr(lifecycle, "_FEED_STOP_TIMEOUT_SECONDS", 0.05)
    stubborn = _Consumer(obey_cancel=False)
    await _running(stubborn, settle=100)

    replacement = _Consumer()
    outcome = await lifecycle.enable(250, replacement.run)

    assert outcome == {"ok": False, "reason": "stop-timeout"}
    assert replacement.settles == [], "a second consumer was started beside the first"
    stubborn.stop_allowed.set()


@pytest.mark.asyncio
async def test_a_consumer_that_will_not_stop_stays_owned(monkeypatch):
    """`disable` used to clear the registered task BEFORE the stop finished, so a
    stop that timed out left a live consumer nobody held - and the next enable
    saw a clean slate and started a third."""
    monkeypatch.setattr(lifecycle, "_FEED_STOP_TIMEOUT_SECONDS", 0.05)
    stubborn = _Consumer(obey_cancel=False)
    await _running(stubborn, settle=100)

    outcome = await lifecycle.disable()

    assert outcome == {"ok": False, "reason": "stop-timeout"}
    assert lifecycle._stopping is not None, "the live consumer was dropped, not held"
    assert lifecycle.feed_state() == "stopping"

    # And nothing may be started on top of it while it is still alive.
    blocked = _Consumer()
    assert await lifecycle.enable(300, blocked.run) == {"ok": False, "reason": "stopping"}
    assert blocked.settles == []

    stubborn.stop_allowed.set()


@pytest.mark.asyncio
async def test_a_consumer_that_finally_stops_frees_the_slot(monkeypatch):
    monkeypatch.setattr(lifecycle, "_FEED_STOP_TIMEOUT_SECONDS", 0.05)
    stubborn = _Consumer(obey_cancel=False)
    await _running(stubborn, settle=100)
    held = lifecycle._task
    await lifecycle.disable()

    stubborn.stop_allowed.set()
    await asyncio.wait({held}, timeout=1)

    assert lifecycle.feed_state() == "stopped"
    fresh = _Consumer()
    assert (await lifecycle.enable(300, fresh.run))["ok"]


@pytest.mark.asyncio
async def test_concurrent_enables_produce_exactly_one_consumer():
    """Nothing serialized the transitions, so callers interleaved at the await
    inside the stop and each went on to start a task of its own.

    Different settle periods on purpose: that is the path with an await in it,
    and an await is where the interleaving happened.
    """
    consumers = [_Consumer() for _ in range(4)]

    await asyncio.gather(*(lifecycle.enable(100 + 10 * i, c.run) for i, c in enumerate(consumers)))

    alive = [c for c in consumers if c.settles and not c.ended.is_set()]
    assert len(alive) == 1, f"{len(alive)} consumers left running for one feed"
    assert lifecycle.feed_enabled()


@pytest.mark.asyncio
async def test_disable_then_enable_leaves_one_consumer():
    first = _Consumer()
    await _running(first, settle=100)

    assert (await lifecycle.disable())["ok"]
    second = _Consumer()
    await _running(second, settle=100)

    assert lifecycle.feed_state() == "running"
    assert first.settles == [100] and second.settles == [100]


@pytest.mark.asyncio
async def test_disabling_a_stopped_feed_says_so():
    assert await lifecycle.disable() == {"ok": False, "reason": "not-enabled"}


@pytest.mark.asyncio
async def test_a_normal_cancellation_is_reported_as_stopped():
    consumer = _Consumer()
    await _running(consumer, settle=100)

    assert (await lifecycle.disable())["ok"]

    assert lifecycle.feed_state() == "stopped"
    assert lifecycle.failure() is None


@pytest.mark.asyncio
async def test_a_consumer_that_raised_is_reported_as_failed():
    """A file-write error that ends the loop is not the same as a stop, and a
    state that called it "stopped" hid the reason the feed went quiet."""

    async def _explode(settle_ms):
        raise OSError("the feed file could not be written")

    await lifecycle.enable(100, _explode)
    await asyncio.wait({lifecycle._task}, timeout=1)

    assert lifecycle.feed_state() == "failed"
    assert "could not be written" in lifecycle.failure()
    assert not lifecycle.feed_enabled()


@pytest.mark.asyncio
async def test_a_cancelled_caller_does_not_orphan_the_consumer(monkeypatch):
    """Cancellation aimed at the CALLER must not cost the process ownership of a
    consumer that is still running and still taking bursts."""
    monkeypatch.setattr(lifecycle, "_FEED_STOP_TIMEOUT_SECONDS", 30.0)
    stubborn = _Consumer(obey_cancel=False)
    await _running(stubborn, settle=100)

    caller = asyncio.get_running_loop().create_task(lifecycle.disable())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    caller.cancel()
    with pytest.raises(asyncio.CancelledError):
        await caller

    assert lifecycle._stopping is not None, "the consumer was orphaned by the caller's cancel"
    assert lifecycle.feed_state() == "stopping"
    stubborn.stop_allowed.set()


@pytest.mark.asyncio
async def test_autostart_refuses_to_double_up():
    consumer = _Consumer()
    await _running(consumer, settle=100)

    second = _Consumer()

    assert lifecycle.start_now(second.run) is False
    assert second.settles == []


# --- what the feed keeps ------------------------------------------------------


def _write_feed(path: Path, ts: float, records: int = 1, pad: int = 0) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = [json.dumps({"ts": ts + i, "chat_id": i, "pad": "x" * pad}) for i in range(records)]
    path.write_text("".join(line + "\n" for line in lines), encoding="utf-8")


def test_an_old_small_feed_is_rotated_out(monkeypatch):
    """The defect: max_age applied only to the ROTATED file, so a quiet feed kept
    months of contact metadata indefinitely by never growing past max_bytes."""
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "60")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time() - 3600)

    store.apply_retention()

    assert not path.exists() or path.stat().st_size == 0, "an expired feed survived"


def test_an_age_rotated_generation_then_expires_on_its_own_clock(monkeypatch):
    """It is moved aside first, not deleted outright. An age rotation fires on
    the OLDEST record, so the file can still hold recent ones; the retained
    generation goes when its newest record - its mtime, which nothing appends to
    after a rotation - is itself past the budget. Either way nothing survives
    indefinitely, which is the whole of the defect."""
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "60")
    path = store.feed_file_path()
    rotated = store._rotated_feed_path(path)
    _write_feed(path, ts=time.time() - 3600)

    store.apply_retention()
    assert rotated.exists(), "the records were dropped rather than retained"

    aged = time.time() - 120
    os.utime(rotated, (aged, aged))
    store.apply_retention()

    assert not rotated.exists(), "the retained generation outlived the age budget"


def test_a_young_small_feed_is_left_alone(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "3600")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time(), records=3)

    store.apply_retention()

    assert len(path.read_text(encoding="utf-8").strip().splitlines()) == 3


def test_appending_does_not_make_an_old_feed_look_young(monkeypatch):
    """mtime is what an append moves forward. The age budget reads the first
    record instead, which is why a feed written to every minute for a year no
    longer reports an age of one minute."""
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "60")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time() - 86400)
    path.touch()  # mtime is now; the oldest record is still a day old

    assert store._oldest_record_age(path) > 86000
    store.apply_retention()
    assert not path.exists() or path.stat().st_size == 0


def test_size_still_rotates_a_young_feed(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_BYTES", "500")
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "3600")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time(), records=4, pad=300)

    store.apply_retention()

    assert store._rotated_feed_path(path).exists(), "a full feed was not rotated"
    assert not path.exists(), "the active name should be free for a fresh generation"


def test_the_retained_generation_expires(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "60")
    path = store.feed_file_path()
    rotated = store._rotated_feed_path(path)
    _write_feed(rotated, ts=time.time())
    old = time.time() - 3600
    os.utime(rotated, (old, old))

    store.apply_retention()

    assert not rotated.exists()


def test_an_unreadable_first_line_does_not_claim_an_age(tmp_path):
    """A half-written or hand-edited line says nothing about age; size still
    bounds the file either way, so guessing here would only delete records."""
    path = tmp_path / "feed.jsonl"
    path.write_text("not json at all\n", encoding="utf-8")

    assert store._oldest_record_age(path) is None


def test_a_missing_feed_has_no_age(tmp_path):
    assert store._oldest_record_age(tmp_path / "absent.jsonl") is None


def test_a_failed_rotation_is_visible_not_only_logged(monkeypatch):
    """Disk filling up because a rename keeps failing must not be discoverable
    only by tailing a log nobody is reading."""
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_BYTES", "10")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time(), pad=200)

    def _refuse(src, dst):
        raise OSError("the rotated name is held open by something else")

    monkeypatch.setattr(store.os, "replace", _refuse)
    store.apply_retention()

    assert "held open" in (store.rotation_error() or "")
    assert path.exists(), "the records were lost to a failed rotation"


def test_the_status_reports_the_state_and_the_retention_contract(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", "3600")
    state = lifecycle.incoming_feed_state()

    assert state["state"] == "stopped"
    assert state["rotation_error"] is None
    assert state["consumer_error"] is None
    assert "oldest record" in state["retention_note"]


# --- generations, backpressure and an idle server ------------------------------


class _Sender:
    bot = False
    is_self = False
    username = "someone"
    first_name = "Someone"


class _Event:
    """An incoming message whose `get_sender()` the test controls."""

    is_private = True
    chat_id = 4242

    def __init__(self, released=None):
        self.message = type("M", (), {"id": 7})()
        self.released = released

    async def get_sender(self):
        if self.released is not None:
            await self.released.wait()
        return _Sender()


@pytest.mark.asyncio
async def test_a_handler_suspended_across_a_replacement_writes_nothing(monkeypatch):
    """Detaching a handler stops NEW events; it cannot reach into one already
    suspended at `get_sender()`. That call goes to the network, so the gap is
    long enough for a reload - and on the far side the old handler went on
    writing pending state under a label that now means a different login."""
    from telegram_mcp.tools import events as mod
    from telegram_mcp.tools import events_store as store_mod

    old_client = object()
    monkeypatch.setattr(mod, "clients", {"work": old_client})
    monkeypatch.setattr(store_mod, "_pending_msgs", {})
    released = asyncio.Event()

    handling = asyncio.ensure_future(mod._on_new_incoming("work", old_client, _Event(released)))
    await asyncio.sleep(0)

    # The reload lands while the handler is suspended.
    mod.clients["work"] = object()
    released.set()
    await handling

    assert store_mod._pending_msgs == {}, "a replaced generation recreated pending state"


@pytest.mark.asyncio
async def test_a_handler_for_the_current_generation_still_records(monkeypatch):
    from telegram_mcp.tools import events as mod
    from telegram_mcp.tools import events_store as store_mod

    client = object()
    monkeypatch.setattr(mod, "clients", {"work": client})
    monkeypatch.setattr(store_mod, "_pending_msgs", {})

    await mod._on_new_incoming("work", client, _Event())

    assert ("work", 4242) in store_mod._pending_msgs


def test_an_append_is_refused_rather_than_breaking_the_size_bound(monkeypatch, tmp_path):
    """Rotation failing was recorded and the append proceeded anyway, so a
    rename that kept failing turned "roughly twice max_bytes" into no bound at
    all while the status went on reporting one."""
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_BYTES", "50")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time(), records=4, pad=60)

    def _refuse(src, dst):
        raise OSError("the rotated name is held open")

    monkeypatch.setattr(store.os, "replace", _refuse)

    with pytest.raises(store.FeedWriteRefused):
        store._open_feed_append()


def test_an_append_is_allowed_once_rotation_works_again(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_BYTES", "50")
    path = store.feed_file_path()
    _write_feed(path, ts=time.time(), records=4, pad=60)

    store._open_feed_append().close()

    assert store.rotation_error() is None


def test_a_feed_within_its_ceiling_is_never_refused(monkeypatch):
    monkeypatch.setenv("TELEGRAM_EVENT_FEED_MAX_BYTES", "100000")
    _write_feed(store.feed_file_path(), ts=time.time())

    store._open_feed_append().close()


@pytest.mark.asyncio
async def test_a_refused_write_keeps_the_burst_rather_than_dropping_it(monkeypatch):
    """Backpressure: the burst is not acknowledged and not discarded, so it is
    still there when the feed can take it again."""
    from telegram_mcp.tools import events as mod

    monkeypatch.setattr(mod, "_WRITE_REFUSED_BACKOFF_SECONDS", 0.01)
    monkeypatch.setattr(
        store,
        "_open_feed_append",
        lambda: (_ for _ in ()).throw(store.FeedWriteRefused("full")),
    )
    monkeypatch.setattr(store, "_pending_msgs", {("a", 1): {"last_ts": 0.0}})
    monkeypatch.setattr(mod, "_scan_settled", lambda *a, **k: (("a", 1), None))
    monkeypatch.setattr(mod, "_burst_summary", lambda key, rec: {"event": True, "chat_id": 1})
    monkeypatch.setattr(store, "_expire_pending", lambda: None)

    loop = asyncio.ensure_future(mod._feed_loop(10))
    await asyncio.sleep(0.05)
    loop.cancel()
    with pytest.raises(asyncio.CancelledError):
        await loop

    assert ("a", 1) in store._pending_msgs, "a burst that could not be written was dropped"


@pytest.mark.asyncio
async def test_an_idle_feed_still_has_its_retention_applied(monkeypatch):
    """Enforcing age only on append and on a status call meant an idle process
    kept whatever the feed already held for as long as nothing happened - which
    is exactly when nothing does."""
    from telegram_mcp.tools import events as mod

    monkeypatch.setattr(mod, "_IDLE_MAINTENANCE_SECONDS", 0.01)
    monkeypatch.setattr(store, "_pending_msgs", {})
    monkeypatch.setattr(mod, "_scan_settled", lambda *a, **k: (None, None))
    monkeypatch.setattr(store, "_expire_pending", lambda: None)
    swept = []
    monkeypatch.setattr(store, "apply_retention", lambda: swept.append(True))

    loop = asyncio.ensure_future(mod._feed_loop(10))
    for _ in range(200):
        if swept:
            break
        await asyncio.sleep(0.01)
    loop.cancel()
    with pytest.raises(asyncio.CancelledError):
        await loop

    assert swept, "an idle consumer never ran the retention pass"
