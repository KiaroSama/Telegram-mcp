"""Telling a voice note from a music file when both are `.ogg`.

The extension cannot answer it and neither can the codec. Measured on 2026-09-20
against a real music file and a file recorded the way a Telegram client records
one:

| | music `.ogg` | voice note |
|---|---|---|
| container | Ogg | Ogg |
| codec | Opus | Opus |
| channels | 1 (mono) | 1 (mono) |
| sample rate | 48000 | 48000 |
| `TITLE` tag | present | absent |

So codec, channel count and sample rate are worthless here — all three are
identical — and the only signal in the container is the Vorbis-comment metadata
a music file carries and a recording does not.

The owner's decision (2026-09-20): no music tags means voice note, any music tag
means audio. The miss is a music file whose tags were stripped, which arrives as a
voice note; `kind="audio"` names it explicitly and costs one argument.

Only the header is read - a few hundred bytes of container, never the audio. This
is parsing, not decoding, which is why it can live beside a package that holds
ciphertext and a schema and no codec.
"""

import pytest

from telegram_mcp import ogg_tags

# The bytes are hand-built rather than fixtures: the whole point is which FIELD
# decides, and a real file would hide that behind megabytes of audio.


def _ogg(*comments: str, codec: bytes = b"OpusHead") -> bytes:
    """An Ogg first page carrying an identification header and a comment block.

    The comment block is framed the way the format really frames it — a
    length-prefixed vendor string, a count, then each comment length-prefixed.
    That framing is what puts a non-letter byte in front of every tag name, and
    that is exactly what tells `ALBUM=` apart from `MYALBUM=`. An unframed
    fixture made the parser look broken when it was the fixture that was wrong.
    """
    head = b"OggS" + b"\x00" * 24 + codec + b"\x01\x01" + b"\x00" * 16
    magic = b"OpusTags" if codec == b"OpusHead" else b"\x03vorbis"
    vendor = b"test"
    block = len(vendor).to_bytes(4, "little") + vendor
    block += len(comments).to_bytes(4, "little")
    for comment in comments:
        raw = comment.encode("utf-8")
        block += len(raw).to_bytes(4, "little") + raw
    return head + magic + block


def test_a_recording_with_no_music_tags_is_a_voice_note():
    assert ogg_tags.looks_like_voice(_ogg()) is True


def test_an_encoder_tag_alone_is_still_a_voice_note():
    """Every Ogg encoder writes one, including the one a phone records with, so
    it says nothing about what the file is."""
    assert ogg_tags.looks_like_voice(_ogg("ENCODER=Lavf62.3.100")) is True


@pytest.mark.parametrize(
    "comment",
    [
        "TITLE=Dishab To Bagh",
        "ARTIST=Someone",
        "ALBUM=A Record",
        "album=lowercase is the same tag",
        "TRACKNUMBER=3",
    ],
)
def test_any_music_tag_makes_it_audio(comment):
    assert ogg_tags.looks_like_voice(_ogg(comment)) is False


def test_the_tag_is_found_even_beside_an_encoder_tag():
    """The real music file measured for this carried both."""
    assert ogg_tags.looks_like_voice(_ogg("ENCODER=Lavf62.3.100", "TITLE=Song")) is False


def test_a_vorbis_ogg_is_read_the_same_way():
    """Vorbis rather than Opus changes the magic and nothing about the question."""
    assert (
        ogg_tags.looks_like_voice(
            _ogg(codec=b"\x01vorbis"),
        )
        is True
    )
    assert ogg_tags.looks_like_voice(_ogg("TITLE=Song", codec=b"\x01vorbis")) is False


def test_bytes_that_are_not_ogg_at_all_are_not_claimed_as_voice():
    """A file whose header says nothing is not evidence FOR anything. `document`
    is what the vocabulary already does with the unrecognised."""
    assert ogg_tags.looks_like_voice(b"ID3\x04not an ogg file at all") is None
    assert ogg_tags.looks_like_voice(b"") is None


def test_a_truncated_header_answers_none_rather_than_guessing():
    assert ogg_tags.looks_like_voice(b"OggS") is None


def test_a_tag_name_inside_a_VALUE_does_not_count():
    """A recording whose ENCODER string happens to contain the word TITLE would
    otherwise be demoted to audio by its own encoder's version string."""
    assert ogg_tags.looks_like_voice(_ogg("ENCODER=TITLEmaker 1.0")) is True
