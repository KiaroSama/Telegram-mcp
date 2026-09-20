"""Is this `.ogg` a voice note, or a music file?

The extension cannot say and neither can the codec. Measured 2026-09-20 on a real
music file and on one recorded the way a Telegram client records: both are Ogg,
both Opus, both mono, both 48 kHz. Every field a reader would reach for first is
identical. What differs is the Vorbis comment block — a music file carries `TITLE`,
`ARTIST`, `ALBUM`; a recording carries at most `ENCODER`, which every Ogg writer
emits and which therefore means nothing.

So the rule, decided by the owner on 2026-09-20: **no music tag means voice note,
any music tag means audio.** The one miss is a music file whose tags were stripped;
it arrives as a voice note, and `kind="audio"` names it explicitly.

This reads the CONTAINER, never the audio: the first few hundred bytes hold the
identification header and the comment block. Parsing a header is not decoding a
codec, which is why the same rule can live in the sibling Telethon Secret Chat
package — the one that holds ciphertext and a schema and deliberately no codec.

`docs/adr/0004-an-ogg-is-a-voice-note-until-its-tags-say-otherwise.md` records why
this exists rather than the two simpler answers (always voice, always audio).
"""

__all__ = ["HEADER_BYTES", "MUSIC_TAGS", "looks_like_voice"]

#: How much of the file the answer needs. An Ogg page is capped at 65 307 bytes
#: and the identification header plus the comment block sit in the first pages;
#: 64 KiB reaches them without ever touching the audio.
HEADER_BYTES = 64 * 1024

#: The Vorbis comment names that mean "this is a piece of music". `ENCODER` is
#: deliberately absent: every Ogg writer emits one, including a phone's recorder,
#: so its presence distinguishes nothing.
MUSIC_TAGS = (
    b"TITLE",
    b"ARTIST",
    b"ALBUM",
    b"ALBUMARTIST",
    b"TRACKNUMBER",
    b"GENRE",
    b"PERFORMER",
    b"COMPOSER",
)


def looks_like_voice(header: bytes):
    """``True`` voice note, ``False`` music, ``None`` when the bytes do not say.

    ``None`` is not "probably music" — it means this reader learned nothing, and
    the caller should fall back to whatever it would have done without a reader
    at all. Bytes that are not an Ogg stream, or a stream cut off before its
    comment block, both land there.
    """
    if not header.startswith(b"OggS") or len(header) <= 4:
        return None

    # A comment block only exists after an identification header. Without one
    # there is nothing to have an opinion about.
    if b"OpusTags" not in header and b"\x03vorbis" not in header:
        return None

    return not _has_music_tag(header)


def _has_music_tag(header: bytes) -> bool:
    """A music tag NAME, at the start of a comment, not anywhere in the bytes.

    A Vorbis comment is ``NAME=value``, so the name is what precedes the first
    `=`. Searching for `b"TITLE"` loose in the buffer matches an encoder whose
    own version string contains the word, and demotes a real recording to audio
    for no reason.
    """
    upper = header.upper()
    for tag in MUSIC_TAGS:
        start = 0
        while True:
            found = upper.find(tag + b"=", start)
            if found == -1:
                break
            # The byte before the name has to be a delimiter, never a letter:
            # `ALBUM=` would otherwise be found inside `MYALBUM=`, and
            # `ENCODER=TITLEmaker` would be found at all.
            if found == 0 or not upper[found - 1 : found].isalpha():
                return True
            start = found + 1
    return False
