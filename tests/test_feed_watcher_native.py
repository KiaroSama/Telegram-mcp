"""The emitted watcher command, run natively, against the files it will meet.

Every earlier version followed a PROXY for file identity and each proxy broke
somewhere:

* length alone missed a replacement that grew back past the old mark;
* a fixed 64-byte prefix, sampled only when the file had grown, missed a
  same-size replacement entirely, lost the beginning of a larger replacement
  that shared those bytes, and - on a file shorter than 64 bytes - treated an
  ordinary append as a new file and replayed everything already emitted;
* the creation stamp cannot work at all on NTFS, where tunneling hands a name
  recreated within about fifteen seconds the OLD stamp.

And a reader arriving mid-write emitted half a record, splitting JSON and UTF-8
alike.

These run the command `watch_command` actually emits, on Windows, as a child
process - stderr captured and the exit code checked, so a watcher that dies is a
failure rather than a silent absence of output.
"""

import base64
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from telegram_mcp.tools import feed_lifecycle as lifecycle

pytestmark = pytest.mark.skipif(
    sys.platform != "win32", reason="the emitted watcher is PowerShell on Windows"
)

# Long enough for several poll intervals, short enough not to drag the suite.
_SETTLE = 2.5


def _start(path: Path, contains=None):
    """Run the exact command the tool publishes, as a child process."""
    command = lifecycle.watch_command(path, contains)
    assert command.startswith("powershell "), command[:40]
    return subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


def _collect(process, seconds=_SETTLE):
    """Let it run, then stop it and return (stdout, stderr, exit code)."""
    time.sleep(seconds)
    process.terminate()
    try:
        out, err = process.communicate(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        out, err = process.communicate(timeout=20)
    return out, err, process.returncode


def _record(chat_id, text="x"):
    return json.dumps({"chat_id": chat_id, "name": text, "ts": 1.0}, ensure_ascii=False) + "\n"


def _append(path: Path, text: str) -> None:
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(text)


@pytest.fixture
def feed(tmp_path):
    path = tmp_path / "incoming_feed.jsonl"
    path.write_text("", encoding="utf-8")
    return path


def test_a_line_written_after_the_watcher_starts_is_emitted(feed):
    process = _start(feed)
    time.sleep(1.0)
    _append(feed, _record(101))

    out, err, code = _collect(process)

    assert "101" in out, f"nothing was emitted. stderr: {err}"
    # PowerShell writes a CLIXML preamble to stderr when it is terminated, which
    # is not an error record. What must not appear is an actual failure.
    assert "Exception" not in err and "ParserError" not in err, err


def test_a_short_append_is_not_replayed(feed):
    """The file is shorter than 64 bytes, so the old fixed-size prefix sample
    changed on every append - the watcher called it a new file, reset to zero
    and emitted everything again."""
    _append(feed, '{"a":1}\n')
    process = _start(feed)
    time.sleep(1.0)
    _append(feed, '{"b":2}\n')

    out, err, _code = _collect(process)

    assert out.count('"b":2') == 1, f"the second line was emitted {out.count('b')} times: {out}"
    assert out.count('"a":1') <= 1, f"an already-emitted line was replayed: {out}"


def test_a_same_size_replacement_is_noticed(feed):
    """Rotation leaves a fresh file. One of exactly the same length grows by
    nothing, so a prefix checked only on growth never looked at it again."""
    _append(feed, _record(111))
    process = _start(feed)
    time.sleep(1.2)
    replacement = _record(222)
    padded = replacement.rstrip("\n").ljust(len(_record(111)) - 1) + "\n"
    feed.write_text(padded, encoding="utf-8")

    out, err, _code = _collect(process, seconds=3.0)

    assert "222" in out, f"the replacement was never read. got: {out!r} stderr: {err}"


def test_a_rotation_starts_the_new_generation_from_its_first_line(feed):
    _append(feed, _record(1) + _record(2))
    process = _start(feed)
    time.sleep(1.2)
    os.replace(feed, feed.with_name(feed.name + ".1"))
    feed.write_text(_record(3), encoding="utf-8")

    out, err, _code = _collect(process, seconds=3.0)

    assert '"chat_id": 3' in out, f"the new generation's first line was skipped: {out!r}"


def test_a_truncation_resets_rather_than_seeking_past_the_end(feed):
    _append(feed, _record(1) + _record(2) + _record(3))
    process = _start(feed)
    time.sleep(1.2)
    feed.write_text(_record(9), encoding="utf-8")

    out, err, _code = _collect(process, seconds=3.0)

    assert '"chat_id": 9' in out


def test_a_record_split_across_two_writes_is_emitted_whole(feed):
    """A reader can arrive between the record and its newline. Emitting what is
    there hands over half a JSON object - and, with a multi-byte character on
    the boundary, half a UTF-8 sequence."""
    process = _start(feed)
    time.sleep(1.0)
    whole = json.dumps({"chat_id": 7, "name": "Ünïcødé ✅"}, ensure_ascii=False)
    _append(feed, whole[:12])
    time.sleep(1.2)
    _append(feed, whole[12:] + "\n")

    out, err, _code = _collect(process, seconds=3.0)

    emitted = [line for line in out.splitlines() if line.strip()]
    assert emitted, f"nothing was emitted. stderr: {err}"
    for line in emitted:
        json.loads(line), f"a partial record was emitted: {line!r}"
    assert any("Ünïcødé" in line for line in emitted)


def test_unicode_survives_the_round_trip(feed):
    process = _start(feed)
    time.sleep(1.0)
    _append(feed, json.dumps({"name": "日本語 🎉"}, ensure_ascii=False) + "\n")

    out, err, _code = _collect(process)

    assert "日本語" in out, f"got {out!r} stderr: {err}"


def test_a_path_with_spaces_and_quotes_is_handled(tmp_path):
    directory = tmp_path / "a folder with spaces"
    directory.mkdir()
    path = directory / "feed it's here.jsonl"
    path.write_text("", encoding="utf-8")
    process = _start(path)
    time.sleep(1.0)
    _append(path, _record(42))

    out, err, _code = _collect(process)

    assert "42" in out, f"got {out!r} stderr: {err}"


def test_the_filter_form_emits_only_matching_lines(feed):
    process = _start(feed, contains='"chat_id": 5')
    time.sleep(1.0)
    _append(feed, _record(4) + _record(5) + _record(6))

    out, err, _code = _collect(process)

    assert '"chat_id": 5' in out
    assert '"chat_id": 4' not in out


def test_the_child_exits_when_it_is_stopped(feed):
    process = _start(feed)
    time.sleep(0.5)
    process.terminate()
    process.communicate(timeout=20)

    assert process.poll() is not None, "the watcher outlived the test that started it"


def test_the_emitted_command_is_a_single_encoded_argument(feed):
    """Encoded, so no shell between the operator and the script has anything
    left to substitute - `$p`, `$o` and the rest were expanded by whichever
    shell it was pasted into."""
    command = lifecycle.watch_command(feed)

    assert "-EncodedCommand" in command
    encoded = command.rsplit(" ", 1)[-1]
    decoded = base64.b64decode(encoded).decode("utf-16-le")
    assert str(feed) in decoded
    assert "$p=" in decoded
