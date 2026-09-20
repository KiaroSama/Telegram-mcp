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


# --- one request, the messages Telegram will actually accept -----------------
#
# `force_document` is ONE flag per media group, so a file and a compressed photo
# cannot share a message however they were asked for. The owner said it in their
# own words: if one goes as a file and another compressed, they belong in two
# separate messages. Splitting is the only honest answer - the alternatives are
# refusing a request Telegram's own clients accept, or silently re-typing one of
# the two.


def _plan(kinds):
    """The group each message carries, as a list of index lists."""
    return [indices for indices, _ in media_send.group_sends(kinds)]


def test_a_file_and_a_photo_become_two_messages_in_the_order_given():
    assert _plan(["photo", "document"]) == [[0], [1]]
    assert _plan(["document", "photo"]) == [[0], [1]]


def test_three_photos_are_one_message():
    assert _plan(["photo", "photo", "photo"]) == [[0, 1, 2]]


def test_photos_and_videos_share_one_group():
    """Telegram's own client does this, and so does every album a human sends."""
    assert _plan(["photo", "video", "photo"]) == [[0, 1, 2]]


def test_files_group_with_files_and_tracks_with_tracks():
    assert _plan(["document", "document"]) == [[0, 1]]
    assert _plan(["audio", "audio"]) == [[0, 1]]


def test_an_album_of_files_does_not_absorb_a_track():
    """Both are 'not compressed', which is not the same as 'the same group'."""
    assert _plan(["document", "audio"]) == [[0], [1]]


@pytest.mark.parametrize("kind", ["voice_note", "video_note", "sticker"])
def test_the_kinds_with_no_group_always_travel_alone(kind):
    """The protocol gives them no media group at all, so two in a row are two
    messages - not one message that silently drops the second."""
    assert _plan([kind, kind]) == [[0], [1]]
    assert _plan(["photo", kind, "photo"]) == [[0], [1], [2]]


def test_a_group_carries_the_flags_of_everything_in_it():
    """One `send_file` call per message, so the group's members have to agree on
    the flags - and they do by construction, because only kinds that share a
    group are put together."""
    ((_, flags),) = media_send.group_sends(["photo", "video"])
    assert flags["force_document"] is False
    assert flags["supports_streaming"] is True


def test_every_kind_is_placed_by_the_planner():
    """A kind nobody assigned would fall through and be grouped by accident."""
    from telegram_mcp import media_kinds

    for kind in media_kinds.KINDS:
        assert _plan([kind]) == [[0]], f"{kind} has no place in the plan"


# --- an .ogg is read before it is named ---------------------------------------
#
# The extension admits both voice_note and audio and cannot choose between them,
# so when no kind is asked for the container header decides. Everything else keeps
# choosing on the extension alone: no other family has two readings this close.


def _ogg_header(*comments: str) -> bytes:
    head = b"OggS" + b"\x00" * 24 + b"OpusHead" + b"\x01\x01" + b"\x00" * 16 + b"OpusTags"
    block = (4).to_bytes(4, "little") + b"test" + len(comments).to_bytes(4, "little")
    for comment in comments:
        raw = comment.encode("utf-8")
        block += len(raw).to_bytes(4, "little") + raw
    return head + block


def test_an_untagged_ogg_is_still_inferred_as_a_voice_note():
    assert media_send.resolve_kind("clip.ogg", None, "", header=_ogg_header()) == "voice_note"


def test_a_tagged_ogg_is_inferred_as_audio_instead():
    """The owner's decision, 2026-09-20: tags mean music. Without this an `.ogg`
    downloaded from a music site arrived as a voice message with a waveform."""
    assert (
        media_send.resolve_kind("song.ogg", None, "", header=_ogg_header("TITLE=A Song"))
        == "audio"
    )


def test_an_explicit_kind_still_wins_over_what_the_header_says():
    """Reading the file informs the DEFAULT. It never overrides the caller."""
    tagged = _ogg_header("TITLE=A Song")
    assert media_send.resolve_kind("song.ogg", "voice_note", "", header=tagged) == "voice_note"
    assert media_send.resolve_kind("clip.ogg", "audio", "", header=_ogg_header()) == "audio"


def test_no_header_falls_back_to_the_extension_default():
    """A caller that cannot supply bytes gets exactly the old behaviour."""
    assert media_send.resolve_kind("clip.ogg", None, "") == "voice_note"


def test_a_header_that_says_nothing_falls_back_too():
    assert media_send.resolve_kind("clip.ogg", None, "", header=b"not an ogg") == "voice_note"


def test_only_the_voice_family_is_read_at_all():
    """An mp3 is audio by extension and there is nothing in its header worth the
    read; a jpg is a photo. Reading every file to place it would cost an I/O on
    every send for one family's ambiguity."""
    assert media_send.resolve_kind("song.mp3", None, "", header=_ogg_header()) == "audio"
    assert media_send.resolve_kind("pic.jpg", None, "", header=_ogg_header()) == "photo"


# --- an mp3 is not a voice note -----------------------------------------------


def test_an_mp3_cannot_be_asked_for_as_a_voice_note():
    """The owner's ruling, 2026-09-20: "mp3 ویس نیست" — an mp3 is not a voice.

    The family table used to allow it, and `quickstart.md` expected a refusal, so
    the two contradicted each other. The real-client pass settled it the hard way:
    asking for a 10 MB mp3 as a voice note was ACCEPTED and began uploading, which
    is how the contradiction was found.
    """
    with pytest.raises(media_send.MediaKindError) as raised:
        media_send.resolve_kind("track.mp3", "voice_note", caption="")

    said = str(raised.value)
    assert "track.mp3" in said and "voice_note" in said


@pytest.mark.parametrize("name", ["track.mp3", "song.m4a", "rec.flac", "clip.wav", "x.aac"])
def test_no_compressed_music_format_may_be_a_voice_note(name):
    with pytest.raises(media_send.MediaKindError):
        media_send.resolve_kind(name, "voice_note", caption="")


def test_an_ogg_may_still_be_either(name="clip.ogg"):
    """Telegram's own voice format genuinely carries both, which is the whole
    reason the tag detection exists."""
    assert media_send.resolve_kind(name, "voice_note", caption="") == "voice_note"
    assert media_send.resolve_kind(name, "audio", caption="") == "audio"


# --- and `audio` now says what it is on the wire -------------------------------


def test_audio_carries_an_attribute_that_says_it_is_not_a_voice():
    """The defect the real-client pass found. Telethon builds
    `DocumentAttributeAudio` only when it can read the file's metadata, and
    without a metadata reader installed it builds NOTHING - so `kind="audio"`
    said nothing at all and Telegram guessed from the mime type, differently per
    file: a small Opus arrived as a voice message, a tagged one as a document.
    """
    flags = media_send.flags_for("audio")
    attributes = flags.get("attributes") or []
    assert attributes, "audio sends no attribute, so it states nothing"
    audio = attributes[0]
    assert audio.voice is False


def test_voice_note_and_video_note_still_need_no_attribute_of_their_own():
    """Telethon force-creates those two itself, which is exactly why they worked
    while `audio` did not. Adding a second one here would fight it."""
    assert "attributes" not in media_send.flags_for("voice_note")
    assert "attributes" not in media_send.flags_for("video_note")


def test_an_ogg_header_gives_the_track_its_real_duration():
    """A track that says 0:00 is the kind of thing an operator reports as broken.
    The duration comes from the container this code already reads for the tags -
    no decoder, no new dependency."""
    flags = media_send.flags_for("audio", header=_ogg_header("TITLE=A Song"))
    assert flags["attributes"][0].duration >= 0


def test_a_kind_that_is_not_audio_ignores_the_header_entirely():
    assert "attributes" not in media_send.flags_for("photo", header=_ogg_header())
    assert "attributes" not in media_send.flags_for("document", header=_ogg_header())
