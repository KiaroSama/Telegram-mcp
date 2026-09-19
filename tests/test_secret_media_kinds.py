"""All eight content kinds a secret chat carries, and the wrapper each needs.

The assertion that earns this file is the WRAPPER DEPTH. `inputMessagePhoto.photo`
is not an InputFile -- it is an `inputPhoto`, whose own `photo` field holds the
file. Passing the file one level too high left that inner field null and TDLib
answered "InputFile is not specified", an error naming the type it wanted and
not the place, which sent an earlier investigation through every path format,
file id and remote id before TDLib's own log settled it. That bug is now
possible in eight places instead of one, so every kind is pinned here.

Nothing here touches TDLib or the network: `build_content` is a pure function
from a path and a kind to the request body, which is exactly the part that was
wrong before and exactly the part a live test would have hidden behind an upload.
"""

import pytest

from telegram_mcp import secret_media_content as smc

# kind -> (outer field, inner wrapper @type)
WRAPPERS = {
    "photo": ("photo", "inputPhoto"),
    "video": ("video", "inputVideo"),
    "document": ("document", "inputDocument"),
    "audio": ("audio", "inputAudio"),
    "animation": ("animation", "inputAnimation"),
    "sticker": ("sticker", "inputSticker"),
    "video_note": ("video_note", "inputVideoNote"),
    "voice_note": ("voice_note", "inputVoiceNote"),
}


@pytest.mark.parametrize("kind", sorted(WRAPPERS))
def test_every_kind_puts_the_file_one_level_down_inside_its_wrapper(kind, tmp_path):
    """The bug that read as a broken TDLib build for an entire evening, now
    guarded for all eight kinds rather than the two that existed then."""
    sample = tmp_path / "x.bin"
    sample.write_bytes(b"bytes")
    outer, wrapper = WRAPPERS[kind]

    content = smc.build_content(str(sample), kind, caption="")

    assert content["@type"] == f"inputMessage{''.join(p.title() for p in kind.split('_'))}"
    holder = content[outer]
    assert holder["@type"] == wrapper, f"{kind} named the wrong wrapper"
    assert holder[outer]["@type"] == "inputFileLocal", f"{kind} passed the file a level too high"
    assert holder[outer]["path"] == str(sample)


@pytest.mark.parametrize(
    "suffix,expected",
    [
        (".jpg", "photo"),
        (".jpeg", "photo"),
        (".png", "photo"),
        (".mp4", "video"),
        (".mov", "video"),
        (".mp3", "audio"),
        (".flac", "audio"),
        (".gif", "animation"),
        (".ogg", "voice_note"),
        (".tgs", "sticker"),
        (".pdf", "document"),
        (".zip", "document"),
        (".whatisthis", "document"),
    ],
)
def test_the_kind_is_inferred_from_the_file_when_the_caller_does_not_say(suffix, expected):
    """A caller forced to name the kind will name it wrong. Anything unrecognised
    falls back to document, which always works -- guessing 'video' at an unknown
    extension would fail at the protocol instead of arriving as a file."""
    assert smc.infer_kind(f"/somewhere/sample{suffix}") == expected


def test_an_explicit_kind_the_file_contradicts_is_refused(tmp_path):
    """Refused HERE, before the upload. Telegram rejects a JPEG sent as a video
    note after the bytes have crossed the wire, which costs the transfer and
    returns an error naming neither the file nor the kind."""
    sample = tmp_path / "photo.jpg"
    sample.write_bytes(b"jpeg")

    with pytest.raises(ValueError) as raised:
        smc.build_content(str(sample), "video_note", caption="")

    message = str(raised.value)
    assert "video_note" in message, "the refusal did not name the kind asked for"
    assert ".jpg" in message or "photo" in message, "the refusal did not name the file's kind"


def test_a_kind_compatible_with_the_file_is_allowed_even_when_it_is_not_the_default(tmp_path):
    """Inference is a default, not a policy. A .mp4 sent as a round video note is
    exactly what video_note is for, and must not be refused for disagreeing with
    the guess."""
    sample = tmp_path / "clip.mp4"
    sample.write_bytes(b"mp4")

    content = smc.build_content(str(sample), "video_note", caption="")

    assert content["@type"] == "inputMessageVideoNote"


def test_any_kind_is_allowed_when_the_extension_says_nothing(tmp_path):
    """An unknown extension cannot contradict anything. Refusing here would block
    a correct send on this server's ignorance of a file suffix."""
    sample = tmp_path / "thing.whatisthis"
    sample.write_bytes(b"?")

    assert smc.build_content(str(sample), "video", caption="")["@type"] == "inputMessageVideo"


def test_a_kind_outside_the_eight_is_refused_by_name(tmp_path):
    sample = tmp_path / "x.bin"
    sample.write_bytes(b"x")

    with pytest.raises(ValueError) as raised:
        smc.build_content(str(sample), "hologram", caption="")

    message = str(raised.value)
    assert "hologram" in message
    for kind in WRAPPERS:
        assert kind in message, "the refusal did not list the kinds that do work"


@pytest.mark.parametrize("kind", ["photo", "video", "document", "audio", "animation"])
def test_the_kinds_that_carry_a_caption_carry_it(kind, tmp_path):
    sample = tmp_path / "x.bin"
    sample.write_bytes(b"x")

    content = smc.build_content(str(sample), kind, caption="hello")

    assert content["caption"]["text"] == "hello"
    assert content["caption"]["@type"] == "formattedText"


@pytest.mark.parametrize("kind", ["sticker", "video_note"])
def test_a_caption_on_a_kind_that_cannot_carry_one_is_refused_not_dropped(kind, tmp_path):
    """Silently dropping it is the failure this whole feature exists to remove.
    A caller who wrote a caption and saw it vanish has no way to find out why."""
    sample = tmp_path / "x.bin"
    sample.write_bytes(b"x")

    with pytest.raises(ValueError) as raised:
        smc.build_content(str(sample), kind, caption="hello")

    assert kind in str(raised.value)
    assert "caption" in str(raised.value).lower()


@pytest.mark.parametrize("kind", ["sticker", "video_note"])
def test_those_kinds_are_fine_with_no_caption(kind, tmp_path):
    sample = tmp_path / "x.bin"
    sample.write_bytes(b"x")

    content = smc.build_content(str(sample), kind, caption="")

    assert "caption" not in content, "sent a caption field to a kind that has none"


def test_voice_notes_and_video_notes_keep_their_underscored_field_names(tmp_path):
    """`voice_note`/`video_note` are two-word kinds, and a naive capitalisation
    would produce `inputMessageVoicenote`, which TDLib answers with 'Unknown
    class' - a failure that looks like a transport problem.

    Deliberately an extension this module does not recognise: the point here is
    the NAME, and a .ogg would also exercise the compatibility check, which
    rightly refuses a voice file sent as a round video.
    """
    sample = tmp_path / "x.unknownext"
    sample.write_bytes(b"bytes")

    assert smc.build_content(str(sample), "voice_note", "")["@type"] == "inputMessageVoiceNote"
    assert smc.build_content(str(sample), "video_note", "")["@type"] == "inputMessageVideoNote"
