"""A real TDLib client, started and closed, against a throwaway database.

Every other TDLib test in this repository uses a double, and a double cannot
answer the question M05 turned on: does the native library actually emit
`authorizationStateClosed`, and does `close()` see it? The repair was to stop
treating the `close` REQUEST's own `ok` as completion - but "TDLib documents
Closed as the completion signal" is a claim about TDLib, and only TDLib can
settle it.

So this starts the genuine `tdjson` library against a temporary directory and
closes it. It is CREDENTIAL-FREE by construction: nothing signs in, nothing is
asked for, and the client stops at `authorizationStateWaitPhoneNumber` - the
state that means "ready for a login that is not going to happen". No account is
touched and no real database is opened.

Skipped where `tdjson` is not installed, which is the supported configuration
for anyone who never wanted secret chats.
"""

import asyncio

import pytest

from telegram_mcp import tdlib


def _tdjson_available() -> bool:
    try:
        tdlib._tdjson()
    except Exception:
        return False
    return True


# Every await below carries its own `wait_for`, so a library that never answers
# fails the test rather than hanging the suite.
pytestmark = pytest.mark.skipif(
    not _tdjson_available(), reason="the optional tdjson binary is not present"
)


@pytest.fixture
def own_database(monkeypatch, tmp_path):
    """A database directory this test owns and throws away."""
    monkeypatch.setattr(tdlib, "database_dir_for", lambda label: tmp_path / "tdlib" / label)
    return tmp_path


@pytest.mark.asyncio
async def test_a_real_client_starts_and_reaches_a_waiting_state(own_database):
    """The library comes up, takes its parameters and asks for a login.

    `authorizationStateWaitPhoneNumber` is the credential-free end of the line:
    it means TDLib is running and would accept a sign-in. Nothing here gives it
    one.
    """
    client = tdlib.TDLibClient("native-smoke")
    try:
        state = await asyncio.wait_for(client.start(), timeout=60)

        assert state.startswith("authorizationState"), state
        assert state != "authorizationStateReady", "a throwaway database must not be signed in"
    finally:
        try:
            await asyncio.wait_for(client.close(timeout=30), timeout=45)
        except Exception:
            pass


@pytest.mark.asyncio
async def test_a_real_close_reaches_the_documented_completion_signal(own_database):
    """The claim M05 rests on, checked against the library rather than a double.

    `close()` answers `ok` immediately and finishes on its own thread. If the
    state below never arrived, the repair would be waiting for something TDLib
    does not send - and the old code, which took the `ok` for completion, would
    have been right after all.
    """
    client = tdlib.TDLibClient("native-close")
    await asyncio.wait_for(client.start(), timeout=60)

    await asyncio.wait_for(client.close(timeout=45), timeout=60)

    assert client.authorization_state == "authorizationStateClosed"
    assert client._client_id is None, "a closed client kept its dispatch registration"


@pytest.mark.asyncio
async def test_the_database_directory_is_released_once_it_is_closed(own_database):
    """What the quarantine's precondition is actually FOR: after a confirmed
    close the directory can be moved, which is what recovery needs to do."""
    import os

    client = tdlib.TDLibClient("native-release")
    await asyncio.wait_for(client.start(), timeout=60)
    directory = tdlib.database_dir_for("native-release")
    assert directory.exists(), "TDLib created no database to release"

    await asyncio.wait_for(client.close(timeout=45), timeout=60)

    moved = directory.with_name(directory.name + ".moved")
    os.replace(directory, moved)
    assert moved.exists()


@pytest.mark.asyncio
async def test_closing_a_client_twice_is_harmless(own_database):
    client = tdlib.TDLibClient("native-twice")
    await asyncio.wait_for(client.start(), timeout=60)

    await asyncio.wait_for(client.close(timeout=45), timeout=60)
    await asyncio.wait_for(client.close(timeout=5), timeout=20)

    assert client._client_id is None


def test_the_receive_thread_leaves_the_native_library_when_asked():
    """The defect this file found. The reader is a DAEMON thread blocked inside
    `td_receive`, and a daemon thread is killed where it stands at interpreter
    shutdown - which, inside TDLib's C++ runtime, ends the process with
    `terminate called without an active exception` and exit 250.

    Nothing had started a real client under pytest before, so a whole test
    session could pass and then abort on its way out. CI reported `2950 passed`
    and exit 250 in the same job.
    """
    tdlib._ensure_reader()
    assert tdlib._reader is not None and tdlib._reader.is_alive()

    assert tdlib.stop_reader(timeout=5) is True, "the receive thread would not leave TDLib"
    assert tdlib._reader is None


def test_stopping_a_reader_that_never_started_is_harmless():
    tdlib.stop_reader(timeout=1)

    assert tdlib.stop_reader(timeout=1) is True
