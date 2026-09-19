"""A session lease belongs to the client that took it, not to the label.

Five defects, one test each, all of them reachable on `main` at 7cc30fd:

1. `_publish` overwrote a live lease at the same label. During a replacement the
   previous client's `_Lease` was the only reference to its `SessionLock`, so it
   was collected and the operating system released a lock that client was still
   connected under.
2. `_release_when_closed` recorded a STRING on an unconfirmed close and returned,
   dropping the lease. The comment said "the lock is STILL HELD"; it was held
   until the next collection.
3. `forget(label)` released whatever lease sat at the label, without asking whose.
4. `_publish` refused at the stop boundary and returned `None`, so `claim_session`
   returned normally and its caller believed it held a lease.
5. `claim_session`'s `to_thread` acquire had no owner, so a cancelled caller left
   a real lock with nothing holding or releasing it.

**Why a second process and not an assertion about Python objects.** Defects 1 and
2 are about a lock being released by garbage collection. A test that asserts an
object still exists passes on the broken code for the wrong reason - the object
exists until the collector runs. The only thing that distinguishes "held" from
"about to be released" is whether another process is refused, so that is what is
asserted, against a real lock file.
"""

import asyncio
import functools
import gc
import subprocess
import sys
import textwrap

import pytest

from telegram_mcp import admission
from telegram_mcp.singleton import SessionLock

# The child below waits on nothing: it tries once and reports. A blocking probe
# would turn a failing assertion into a hang.
_PROBE = textwrap.dedent("""
    import sys
    from telegram_mcp.singleton import try_lock_exclusive
    with open(sys.argv[1], "a+") as fh:
        print("FREE" if try_lock_exclusive(fh) else "REFUSED")
    """)


def second_process_refused(lock_path) -> bool:
    """True when another process cannot take this lock - i.e. we really hold it."""
    done = subprocess.run(
        [sys.executable, "-c", _PROBE, str(lock_path)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert done.returncode == 0, f"probe failed: {done.stderr}"
    return done.stdout.strip() == "REFUSED"


class FakeSession:
    def __init__(self, value: str):
        self._value = value

    def save(self) -> str:
        return self._value


class FakeClient:
    """Enough of a Telethon client for `session_identity` and nothing more."""

    def __init__(self, value: str):
        self.session = FakeSession(value)


@pytest.fixture
def leases(monkeypatch, tmp_path):
    """A clean, isolated admission module over a real lock directory."""
    monkeypatch.setattr(admission, "session_locks", {})
    monkeypatch.setattr(admission, "unreleased_leases", {})
    monkeypatch.setattr(admission, "_awaiting_admission", {})
    monkeypatch.setattr(admission, "_admitting", {})
    monkeypatch.setattr(admission, "_releasing", set())
    monkeypatch.setattr(admission, "_stopped", False)
    for name in ("_leases", "_active", "_retiring"):
        if hasattr(admission, name):
            monkeypatch.setattr(admission, name, {})
    # Real locks, in a directory this test owns.
    monkeypatch.setattr(
        admission, "SessionLock", functools.partial(SessionLock, lock_dir=tmp_path)
    )
    return tmp_path


def lock_path_for(leases_dir, client) -> "object":
    """Where the module would put this client's lock file."""
    return SessionLock(admission.session_identity(client), lock_dir=leases_dir).path


# --- Defect 1: a replacement must not cost the serving client its claim --------


@pytest.mark.asyncio
async def test_a_replacement_does_not_release_the_serving_clients_lock(leases):
    """US1 scenario 1. The previous client is still connected; its claim stays."""
    serving = FakeClient("session-A")
    replacement = FakeClient("session-B")
    await admission.claim_session("work", serving, grace_seconds=1.0)
    serving_lock = lock_path_for(leases, serving)
    assert second_process_refused(serving_lock), "the fixture itself did not take the lock"

    await admission.claim_session("work", replacement, grace_seconds=1.0)
    gc.collect()

    assert second_process_refused(serving_lock), (
        "the serving client's session became free the moment a replacement claimed "
        "the same label - another process can now connect the key it is using"
    )


@pytest.mark.asyncio
async def test_a_failed_replacement_gives_up_only_its_own_claim(leases):
    """US1 scenario 2."""
    serving = FakeClient("session-A")
    replacement = FakeClient("session-B")
    await admission.claim_session("work", serving, grace_seconds=1.0)
    await admission.claim_session("work", replacement, grace_seconds=1.0)

    admission.forget("work", client=replacement)
    gc.collect()

    assert second_process_refused(
        lock_path_for(leases, serving)
    ), "retiring the failed replacement took the serving client's claim with it"
    assert not second_process_refused(
        lock_path_for(leases, replacement)
    ), "the failed replacement kept its own claim"


# --- Defect 2: an unconfirmed close keeps the lock, genuinely ------------------


@pytest.mark.asyncio
async def test_an_unconfirmed_close_keeps_the_lock_after_collection(leases, monkeypatch):
    """US2 scenarios 1 and 2. The module says it keeps the lock; this checks it does."""
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.2)
    client = FakeClient("session-A")
    await admission.claim_session("work", client, grace_seconds=1.0)
    path = lock_path_for(leases, client)

    never = asyncio.get_running_loop().create_future()
    admission.forget("work", closing=never, client=client)
    await asyncio.sleep(0.5)
    gc.collect()

    assert "work" in admission.unreleased_leases, "an unconfirmed close was not reported"
    assert second_process_refused(path), (
        "the lease was dropped after the close did not confirm, so the lock went with "
        "the next collection - exactly what the module's comment says it prevents"
    )
    never.cancel()


@pytest.mark.asyncio
async def test_a_late_confirmation_releases_the_claim_once(leases, monkeypatch):
    """US2 scenario 3."""
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 5.0)
    client = FakeClient("session-A")
    await admission.claim_session("work", client, grace_seconds=1.0)
    path = lock_path_for(leases, client)

    closing = asyncio.get_running_loop().create_future()
    admission.forget("work", closing=closing, client=client)
    await asyncio.sleep(0)
    closing.set_result(None)
    await asyncio.sleep(0.1)
    gc.collect()

    assert "work" not in admission.unreleased_leases
    assert not second_process_refused(path), "a confirmed close did not release the claim"


# --- Defect 3: retiring a label acts on one client's lease --------------------


@pytest.mark.asyncio
async def test_retiring_the_wrong_owner_releases_nothing(leases):
    """US3 scenario 1."""
    gone = FakeClient("session-A")
    holder = FakeClient("session-B")
    await admission.claim_session("work", holder, grace_seconds=1.0)

    admission.forget("work", client=gone)
    gc.collect()

    assert second_process_refused(lock_path_for(leases, holder)), (
        "retiring a client that does not own this label released the claim of the "
        "client that does"
    )
    assert "work" in admission.session_locks, "the holder's lease was unpublished"


# --- Defect 4: a refused claim is an error, not a normal return ---------------


@pytest.mark.asyncio
async def test_a_claim_that_lands_after_shutdown_refuses_its_caller(leases, monkeypatch):
    """US4 scenario 1."""
    client = FakeClient("session-A")
    monkeypatch.setattr(admission, "_stopped", True)

    with pytest.raises(admission.AdmissionSuperseded) as refusal:
        await admission.claim_session("work", client, grace_seconds=1.0)

    assert "work" in str(refusal.value)
    assert "work" not in admission.session_locks
    gc.collect()
    assert not second_process_refused(
        lock_path_for(leases, client)
    ), "the refused claim kept its lock"


# --- Defect 5: an abandoned request still leaves an owner ---------------------


@pytest.mark.asyncio
async def test_a_cancelled_caller_still_leaves_its_claim_owned(leases):
    """US5 scenario 1. The thread cannot be cancelled, so its result needs an owner."""
    client = FakeClient("session-A")
    task = asyncio.ensure_future(admission.claim_session("work", client, grace_seconds=5.0))
    await asyncio.sleep(0)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    # Let the acquire's own owner finish whatever it is doing.
    await asyncio.sleep(0.3)
    gc.collect()

    assert "work" in admission.session_locks, (
        "the caller was cancelled and the acquire's result was dropped: this process "
        "may hold a real lock it has no record of"
    )
    assert second_process_refused(lock_path_for(leases, client))


@pytest.mark.asyncio
async def test_two_callers_for_one_account_take_one_claim(leases):
    """FR-009, preserved. Two acquires on one session conflict inside one process."""
    client = FakeClient("session-A")
    admission.mark_awaiting_admission({"work": client})

    await asyncio.gather(
        admission.admit_if_pending(client),
        admission.admit_if_pending(client),
    )

    assert "work" in admission.session_locks
    assert second_process_refused(lock_path_for(leases, client))


# --- Cross-cutting ------------------------------------------------------------


@pytest.mark.asyncio
async def test_shutdown_names_every_account_it_could_not_account_for(leases, monkeypatch):
    """SC-004."""
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.2)
    client = FakeClient("session-A")
    await admission.claim_session("work", client, grace_seconds=1.0)

    never = asyncio.get_running_loop().create_future()
    admission.forget("work", closing=never, client=client)
    await asyncio.sleep(0.5)
    admission.release_all()

    assert admission.unreleased_leases.get(
        "work"
    ), "shutdown reported nothing about an account whose session it still holds"
    never.cancel()
    admission.begin_serving()


@pytest.mark.asyncio
async def test_no_refusal_or_report_contains_a_session_value(leases, monkeypatch):
    """SC-005. The session string is the credential; a message names the label."""
    secret = "THIS-IS-THE-SESSION-STRING"
    client = FakeClient(secret)
    monkeypatch.setattr(admission, "_CLOSE_BEFORE_RELEASE_SECONDS", 0.2)

    monkeypatch.setattr(admission, "_stopped", True)
    with pytest.raises(admission.AdmissionSuperseded) as refusal:
        await admission.claim_session("work", client, grace_seconds=1.0)
    assert secret not in str(refusal.value)

    monkeypatch.setattr(admission, "_stopped", False)
    await admission.claim_session("work", client, grace_seconds=1.0)
    never = asyncio.get_running_loop().create_future()
    admission.forget("work", closing=never, client=client)
    await asyncio.sleep(0.5)

    assert secret not in "".join(admission.unreleased_leases.values())
    never.cancel()
