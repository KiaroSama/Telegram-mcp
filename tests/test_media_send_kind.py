"""What a media kind means to the ORDINARY send path.

The vocabulary in `media_kinds` says which kinds exist and which files may take
them. This says what each one turns into on the wire, which is a different job and
a different backend - Telethon flags here, TDLib content bodies in
`secret_media_content.py`, protocol attributes in the sibling package.

It lives in its own module because `tools/media.py` was at 789 lines when this was
written, past the point where the project's own ceiling closes a file to new code.
"""

import pytest

from telegram_mcp import media_send

# --- what leaves the process, per kind ---------------------------------------


def test_a_voice_note_is_sent_as_one_rather_than_as_a_file():
    """The gap this feature exists to close: `send_file` never passed a single one
    of these flags, so an `.ogg` arrived as a file with a download button."""
    assert media_send.flags_for("voice_note") == {"voice_note": True}


def test_a_video_note_is_the_round_one():
    assert media_send.flags_for("video_note") == {"video_note": True}


def test_document_forces_the_file_form():
    """ "Send as file" in the operator's own client. Without it Telethon sends a
    `.jpg` as a photo and there is no way to ask for the file."""
    assert media_send.flags_for("document") == {"force_document": True}


@pytest.mark.parametrize("kind", ["photo", "video"])
def test_the_compressed_kinds_say_so_explicitly(kind):
    """`force_document=False` is Telethon's default, but saying it is what makes
    the pair symmetric: one request carries the caller's choice either way."""
    assert media_send.flags_for(kind)["force_document"] is False


def test_a_video_is_offered_for_streaming_and_a_video_note_is_not():
    """A round video note is seconds long and plays whole; streaming is for the
    rectangular kind."""
    assert media_send.flags_for("video").get("supports_streaming") is True
    assert "supports_streaming" not in media_send.flags_for("video_note")


def test_an_animation_rides_the_video_route_without_a_flag_of_its_own():
    """A soundless video is displayed as a GIF by Telegram already - the library's
    own `nosound_video` documentation says so - so a separate route would name
    behaviour that happens anyway. research.md records the decision."""
    assert "nosound_video" not in media_send.flags_for("animation")


def test_every_kind_in_the_vocabulary_has_flags():
    """The parity relation, at this layer: a kind nobody mapped would be accepted
    and then sent as something else."""
    from telegram_mcp import media_kinds

    for kind in media_kinds.KINDS:
        assert isinstance(media_send.flags_for(kind), dict), f"{kind} has no mapping"


# --- refusals, before anything is uploaded -----------------------------------


def test_a_kind_the_file_cannot_be_is_refused_and_names_both():
    with pytest.raises(ValueError) as raised:
        media_send.resolve_kind("song.mp3", "video_note", caption="")

    said = str(raised.value)
    assert "song.mp3" in said and "video_note" in said


def test_an_unknown_kind_lists_the_ones_that_exist():
    with pytest.raises(ValueError) as raised:
        media_send.resolve_kind("photo.jpg", "hologram", caption="")

    said = str(raised.value)
    assert "hologram" in said
    assert "voice_note" in said and "sticker" in said


@pytest.mark.parametrize("kind", ["sticker", "video_note"])
def test_a_caption_on_a_kind_that_has_none_is_refused(kind):
    """Refused rather than dropped: a caller who wrote one and saw it vanish
    cannot find out why."""
    name = "sticker.tgs" if kind == "sticker" else "clip.mp4"
    with pytest.raises(ValueError):
        media_send.resolve_kind(name, kind, caption="a caption")


def test_no_kind_named_infers_one_and_says_which():
    assert media_send.resolve_kind("recording.ogg", None, caption="") == "voice_note"
    assert media_send.resolve_kind("mystery.qqq", None, caption="") == "document"


def test_an_unknown_extension_is_not_proof_a_kind_is_impossible():
    """The check catches a file that demonstrably cannot be what was asked for, not
    one whose type nobody recognised."""
    assert media_send.resolve_kind("no-extension", "video", caption="") == "video"


def test_anything_may_be_sent_as_a_document():
    for name in ("photo.jpg", "clip.mp4", "song.mp3", "mystery.qqq"):
        assert media_send.resolve_kind(name, "document", caption="") == "document"
