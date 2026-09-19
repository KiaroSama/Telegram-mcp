"""What a SECOND instance of this server does while the first one is running.

Starting the launcher while an instance is already up is the most common way
this server is run wrongly, and it is not a rare accident: the desktop client
restarts its connector, a terminal session is still attached, or the operator
simply starts it twice. The refusal itself is correct and must not change - one
auth key connected twice is invalidated by Telegram for BOTH ends.

What was wrong is everything the operator saw. Measured on this machine, with a
healthy server already holding the session:

* **24.6 seconds**, of which 20 were the lock's grace period, spent in complete
  silence. Nothing said a wait was happening, how long it could last, or what it
  was waiting for. A launcher that prints nothing for twenty seconds and then
  dies is indistinguishable from one that hung.
* The launcher's last word was ``uv exited with code 1``, which blames the
  package manager for a deliberate, correct refusal by this server.

So: the wait announces itself, and the refusal carries an exit code of its own
that the launcher can recognise rather than guess at.
"""

import time

import pytest

from telegram_mcp import admission as _admission
from telegram_mcp import runner
from telegram_mcp.singleton import SessionLock, SessionLockError, session_identity


@pytest.fixture
def lock_dir(tmp_path):
    return tmp_path / "locks"


def _held(identity: str, lock_dir):
    """A lock this process already holds, standing in for the first instance."""
    holder = SessionLock(identity, lock_dir=lock_dir)
    holder.acquire(grace_seconds=1, poll_interval=0.01)
    return holder


# --- the wait announces itself ------------------------------------------------


def test_a_free_lock_says_nothing(lock_dir):
    """The common path stays silent: a note on every start is noise."""
    said = []

    SessionLock("nobody-else", lock_dir=lock_dir).acquire(
        grace_seconds=1, poll_interval=0.01, on_wait=said.append
    )

    assert said == [], "a lock that was free announced a wait that never happened"


def test_a_held_lock_announces_the_wait_once_with_its_bound(lock_dir):
    """The counterexample: twenty seconds of silence, then an error.

    Called ONCE - a note per poll would be 40 lines for one wait - and handed
    the grace period, because 'waiting' without 'for up to how long' is the half
    of the message that makes the difference between informative and alarming.
    """
    holder = _held("taken", lock_dir)
    said = []

    try:
        with pytest.raises(SessionLockError):
            SessionLock("taken", lock_dir=lock_dir).acquire(
                grace_seconds=0.3, poll_interval=0.05, on_wait=said.append
            )
    finally:
        holder.release()

    assert len(said) == 1, f"expected exactly one note, got {len(said)}: {said}"
    assert said[0] == pytest.approx(0.3), "the note did not carry the grace period"


def test_the_wait_note_is_emitted_before_the_waiting_not_after(lock_dir):
    """Otherwise it is a post-mortem, not a warning."""
    holder = _held("taken-timing", lock_dir)
    when = []
    started = time.monotonic()

    try:
        with pytest.raises(SessionLockError):
            SessionLock("taken-timing", lock_dir=lock_dir).acquire(
                grace_seconds=0.4,
                poll_interval=0.05,
                on_wait=lambda _s: when.append(time.monotonic() - started),
            )
    finally:
        holder.release()

    assert when and when[0] < 0.2, f"the note arrived {when} into a 0.4s wait"


def test_a_lock_that_frees_up_during_the_grace_is_still_taken(lock_dir):
    """The grace exists for a real case - the old process is still exiting - and
    announcing the wait must not change whether the wait succeeds."""
    holder = _held("transient", lock_dir)
    said = []

    def _free_it(_seconds):
        holder.release()

    second = SessionLock("transient", lock_dir=lock_dir)
    second.acquire(grace_seconds=2, poll_interval=0.05, on_wait=_free_it)
    said.append("acquired")
    second.release()

    assert said == ["acquired"]


# --- the startup path carries the note through --------------------------------


class _FakeClient:
    def __init__(self, identity: str):
        self.session = type("S", (), {"save": lambda _self: identity})()
        self.connected = False

    async def connect(self):
        self.connected = True

    async def is_user_authorized(self):
        return True

    async def disconnect(self):
        self.connected = False


@pytest.mark.asyncio
async def test_startup_tells_the_operator_it_is_waiting_on_another_instance(
    monkeypatch, lock_dir, capsys
):
    """End to end through the caller startup actually uses.

    Unpatched this printed nothing at all between 'Starting N Telegram
    client(s)' and the refusal twenty seconds later.
    """
    # NOT `DEFAULT_LOCK_DIR`. `SessionLock.__init__` takes it as a default
    # argument, which binds at def time - patching the module attribute leaves
    # every lock still going to the real temp directory, and the test then
    # acquires a lock nobody was holding and passes for the wrong reason. Patch
    # the name the caller actually constructs through.
    monkeypatch.setattr(
        _admission, "SessionLock", lambda identity: SessionLock(identity, lock_dir=lock_dir)
    )
    monkeypatch.setenv("TELEGRAM_LOCK_GRACE_SECONDS", "0.3")
    client = _FakeClient("busy-session")
    # Through `session_identity`, not the raw string: it prefixes a saved string
    # session with "string:", so locking the bare value locks a DIFFERENT file
    # and the contest this test exists to create never happens.
    holder = _held(session_identity(client), lock_dir)

    try:
        with pytest.raises(SessionLockError):
            await runner._connect_authorized_client("default", client)
    finally:
        holder.release()
        _admission.session_locks.clear()

    said = capsys.readouterr().err
    assert "waiting" in said.lower(), f"startup said nothing about the wait: {said!r}"
    assert "0.3" in said or "0s" in said, f"the wait's bound was not stated: {said!r}"


# --- the refusal has an exit code of its own ----------------------------------


def test_an_already_running_instance_exits_with_its_own_code():
    """Not 1. The launcher prints 'uv exited with code 1' for a generic failure,
    which blamed the package manager for this server's deliberate refusal - and
    left the operator debugging uv instead of closing the other instance."""
    assert runner.EXIT_SESSION_HELD != 1
    assert runner.EXIT_SESSION_HELD != 0
    # Bounded to what a shell can carry back.
    assert 1 < runner.EXIT_SESSION_HELD < 126


@pytest.mark.asyncio
async def test_a_held_session_leaves_through_that_code(monkeypatch, lock_dir):
    """Through `_main`, which is what `main.py` runs."""

    async def _boom(*_a, **_k):
        raise SessionLockError("already connected")

    # The real seam: `_main` gathers this over `clients`.
    monkeypatch.setattr(runner, "_connect_authorized_client", _boom)
    monkeypatch.setattr(runner, "clients", {"default": _FakeClient("held")})

    with pytest.raises(SystemExit) as left:
        await runner._main()

    assert left.value.code == runner.EXIT_SESSION_HELD


def test_the_launcher_names_the_reason_instead_of_blaming_uv():
    """The launcher reads that code and says what actually happened.

    Source-level, because the alternative is a full launcher run with a stubbed
    uv; what matters is that the code is recognised and that `uv exited with
    code` is no longer the only thing an operator is told.
    """
    from pathlib import Path

    launcher = Path(__file__).resolve().parents[1] / "start-mcp.ps1"
    text = launcher.read_text(encoding="utf-8")

    assert (
        "EXIT_SESSION_HELD" in text or str(runner.EXIT_SESSION_HELD) in text
    ), "the launcher does not recognise the already-running exit code"
    held = text.index("already") if "already" in text else -1
    assert held != -1, "the launcher never mentions another instance already running"


# --- the launcher must survive the shell it is actually started from ----------


def test_the_launcher_never_passes_python_source_as_a_command_line_argument():
    """The launcher was dead on Windows PowerShell 5.1, which is `powershell.exe`
    - the host a double-click, a `Start-Process`, a scheduled task and most
    embedding clients get by default.

    It ran `& uv run python -c <multi-line source>`. Passing a native executable
    an argument containing double quotes is exactly where 5.1's argument
    handling differs from pwsh 7's: the quotes around the ANSI regex were
    dropped, `python -c` received `r\x1b(?:...` and died with

        File "<string>", line 7
        SyntaxError: unexpected character after line continuation character

    four seconds in, before a single client was started. Under pwsh 7 the same
    file worked, which is why it survived - the failure was invisible to anyone
    testing in the shell they develop in.

    The fix is not a better escape. Source goes to a FILE and the file is run,
    which has no quoting problem to get wrong on any host.
    """
    from pathlib import Path

    launcher = Path(__file__).resolve().parents[1] / "start-mcp.ps1"
    text = launcher.read_text(encoding="utf-8")

    assert "run python -c" not in text, (
        "the launcher still hands Python source to `-c`, which Windows " "PowerShell 5.1 corrupts"
    )


def test_the_tee_wrapper_puts_the_checkout_on_the_path_itself(tmp_path):
    """Running the wrapper from a file must import the project the same way
    `python main.py` does.

    `python -c` set ``sys.path[0]`` to the working directory - the checkout -
    and the install guard reads the project's provenance from where
    `telegram_mcp` was imported. Moving the wrapper into a file moved
    ``sys.path[0]`` to the file's own directory, the distribution resolved
    through site-packages alone, and the server refused to start with

        Refusing to start: the installed 'telegram-mcp' distribution was not
        installed from an explicit source checkout.

    Measured: `source_root` went from the checkout to ``None``. So the wrapper
    puts the checkout on the path deliberately rather than inheriting it from
    wherever it happens to be run - it is handed `main.py`'s absolute path and
    the checkout is its directory.
    """
    from pathlib import Path

    launcher = (Path(__file__).resolve().parents[1] / "start-mcp.ps1").read_text(encoding="utf-8")

    start = launcher.index("$pythonWrapper = @'")
    wrapper = launcher[start : launcher.index("'@", start)]

    assert "sys.path.insert(0, os.path.dirname" in wrapper, (
        "the wrapper does not put the checkout on sys.path, so the install "
        "guard cannot see the project's provenance"
    )
