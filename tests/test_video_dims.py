"""Reading a video's real size out of its container.

Telethon builds `DocumentAttributeVideo` from `hachoir`, and without that reader
installed it falls back to `w=1, h=1, duration=0` — which is what this server has
been sending for every video. Telegram re-derives the real size on its side so
nothing looks broken to a viewer, but the attribute we send is simply wrong, and
a client that trusts it lays out a 1x1 box.

hachoir was measured as the alternative and rejected: it reads `.mp4` correctly
(640x360) but returns `w=1, h=1` for `.webm`, so it would have fixed one container
and left the other — while adding a dependency and stderr noise. Reading the
container is the same thing `ogg_tags` already does for the audio duration.

Only the head and tail of the file are read. This parses a container, it does not
decode a codec.
"""

import io
import os

import pytest

from telegram_mcp import video_dims


def _mp4(width: int, height: int, *, duration: int = 0, timescale: int = 1000) -> bytes:
    """A minimal MP4 carrying just the two boxes this reader looks at."""
    mvhd_payload = (
        b"\x00\x00\x00\x00"  # version 0 + flags
        + b"\x00" * 8  # creation, modification
        + timescale.to_bytes(4, "big")
        + (duration * timescale).to_bytes(4, "big")
    )
    mvhd = (len(mvhd_payload) + 8).to_bytes(4, "big") + b"mvhd" + mvhd_payload
    # width and height are 16.16 fixed point and sit at the very END of tkhd.
    tkhd_payload = (
        b"\x00\x00\x00\x00"
        + b"\x00" * 72
        + ((width << 16).to_bytes(4, "big"))
        + ((height << 16).to_bytes(4, "big"))
    )
    tkhd = (len(tkhd_payload) + 8).to_bytes(4, "big") + b"tkhd" + tkhd_payload
    inner = mvhd + tkhd
    moov = (len(inner) + 8).to_bytes(4, "big") + b"moov" + inner
    return b"\x00\x00\x00\x14ftypisom" + b"\x00" * 8 + moov


def _webm(width: int, height: int) -> bytes:
    """A minimal Matroska fragment with the two pixel elements."""
    return (
        b"\x1a\x45\xdf\xa3"
        + b"\x00" * 40
        + b"\xb0\x82"
        + width.to_bytes(2, "big")
        + b"\xba\x82"
        + height.to_bytes(2, "big")
    )


def test_an_mp4_states_its_real_size():
    assert video_dims.dimensions(_mp4(640, 360)) == (640, 360, 0)


def test_an_mp4_states_its_duration_in_whole_seconds():
    assert video_dims.dimensions(_mp4(1920, 1080, duration=12)) == (1920, 1080, 12)


def test_a_webm_states_its_real_size():
    w, h, _ = video_dims.dimensions(_webm(480, 480))
    assert (w, h) == (480, 480)


def test_a_square_clip_is_square():
    """A video note is round, which means square source. Sending 1x1 for it was
    the same bug wearing a different shape."""
    assert video_dims.dimensions(_mp4(480, 480))[:2] == (480, 480)


def test_bytes_that_are_not_a_video_answer_zeros():
    """Zeros mean "this reader learned nothing", and the caller then sends no
    attribute rather than a wrong one."""
    assert video_dims.dimensions(b"not a container at all") == (0, 0, 0)
    assert video_dims.dimensions(b"") == (0, 0, 0)


def test_a_moov_past_the_head_is_found_in_the_tail():
    """A large mp4 without faststart puts `moov` at the END. Reading only the head
    is why this has to take both."""
    body = _mp4(1280, 720, duration=30)
    split = body.index(b"moov") - 8
    head, tail = body[:split], body[split:]
    assert video_dims.dimensions(head) == (0, 0, 0), "the head alone cannot know"
    assert video_dims.dimensions(head, tail) == (1280, 720, 30)


def test_a_truncated_box_answers_zeros_rather_than_guessing():
    body = _mp4(640, 360)
    assert video_dims.dimensions(body[: body.index(b"tkhd") + 6]) == (0, 0, 0)


# --- and against files ffmpeg actually produced ------------------------------

_SAMPLES = os.environ.get("TELEGRAM_MCP_VIDEO_SAMPLES", "")


@pytest.mark.skipif(not _SAMPLES, reason="no real sample directory supplied")
@pytest.mark.parametrize(
    ("name", "expected"), [("v640.mp4", (640, 360)), ("v480.webm", (480, 480))]
)
def test_real_files_from_ffmpeg(name, expected):
    path = os.path.join(_SAMPLES, name)
    if not os.path.exists(path):
        pytest.skip(f"{name} not present")
    with io.open(path, "rb") as handle:
        head = handle.read(video_dims.HEAD_BYTES)
        handle.seek(0, os.SEEK_END)
        size = handle.tell()
        handle.seek(max(0, size - video_dims.TAIL_BYTES))
        tail = handle.read()
    assert video_dims.dimensions(head, tail)[:2] == expected
