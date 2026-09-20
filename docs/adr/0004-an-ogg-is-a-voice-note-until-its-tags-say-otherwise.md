# An `.ogg` is a voice note until its tags say otherwise

The two repositories agreed on the eight media kind names and disagreed on which one
an unnamed `.ogg` becomes: this server inferred `voice_note`, the Telethon Secret Chat
package inferred `audio`. Both defaults were deliberate and both were argued in
comments. Neither is visible today, because the secret path still runs on TDLib — and
the day TDLib is removed, every `.ogg` sent without an explicit kind silently changes
what it is.

So the disagreement had to be settled rather than papered over. The owner's answer was
neither default: **detect it.**

## What can actually be detected

Measured on 2026-09-20, against a real music file and a file recorded the way a
Telegram client records one:

| | music `.ogg` | voice note |
|---|---|---|
| container | Ogg | Ogg |
| codec | Opus | Opus |
| channels | 1 (mono) | 1 (mono) |
| sample rate | 48000 | 48000 |
| `ENCODER` tag | present | present |
| `TITLE` tag | **present** | **absent** |

Every field a reader reaches for first is identical. Codec, channel count and sample
rate distinguish nothing at all, and an implementation built on any of them would have
looked right and been wrong. The only signal in the container is the Vorbis comment
block: a music file carries `TITLE`/`ARTIST`/`ALBUM`, a recording carries at most
`ENCODER`, which every Ogg writer emits.

**The rule**: no music tag means voice note, any music tag means audio. An explicit
`kind` is never second-guessed; this decides the default only.

## Considered options

**Keep one side's default and change the other** was the obvious reading of "make them
agree", and it was rejected because both defaults are wrong about half the time. The
owner's own library contains both a voice message and an `.ogg` music file, and no
single default serves both.

**Read the duration and use a threshold** — under five minutes is a voice note — was
rejected because reading a duration means decoding, not parsing: it needs the whole
stream, not the head, and it would put a codec inside a package that deliberately holds
ciphertext and a schema and nothing else. The tag signal costs 64 KiB of reading and no
dependency.

**Convert or re-tag the file to match the requested kind** was rejected for the reason
this project always rejects it: it sends the operator something they never saw. The
refuse-never-convert rule in `0003` applies here unchanged.

## Consequences

A music file whose tags were stripped is indistinguishable from a recording and arrives
as a voice note. That is the accepted miss. The reverse — a real voice note demoted to
a music file — cannot happen, because a recorder does not write music tags.

**Correction, and then a correction OF the correction — both measured (2026-09-20).** The
sentence that used to stand here said `kind="audio"` names the exception for one
argument. It does not, for this container. An OGG/Opus file is a voice message to
Telegram whatever attribute is attached: the send carries
`DocumentAttributeAudio(voice=False)` — confirmed by calling Telethon's
`get_attributes` on the flags this server produces — and the stored message still
reports `voice`. The identical flags on an `.mp3` store as `audio`, so the rule
belongs to the container and to Telegram's server, not to this code.

**That blamed the wrong thing, and the owner disproved it** by producing a real
`.ogg` in the wild that arrives as a track. Its only difference: `mime_type` is
`audio/vorbis`, not `audio/ogg`.

Telegram picks the renderer from the **MIME type** — not from the container, and not
from `DocumentAttributeAudio.voice`. `mimetypes` guesses `audio/ogg` from the
extension, and `audio/ogg` is Telegram's VOICE type, so every `.ogg` this server sent
was asking to be a voice note whatever else it said. An `.ogg` asked for as a track
now claims `audio/vorbis`; a voice note keeps `audio/ogg`; formats that are already
tracks, like `.mp3`, are left alone.

**Verified end to end**: the same `.ogg`, sent twice, stores as `audio` with
`kind="audio"` and as `voice` with `kind="voice_note"`. So the detection can express
its whole answer after all, and there is no owner decision left here.

The lesson worth keeping is about the first conclusion, not the second: "the same
flags behave differently on two containers" was read as a container rule, when the
containers also differed in the field that actually decides. A difference that
travels with the thing you are comparing is not evidence about the thing you are
comparing.

Reading the head of an upload puts the file pointer past it, and Telethon uploads from
wherever the pointer is left. A peek that forgets to rewind truncates the file and the
send still reports success, so `tests/test_routed_send_tools.py` asserts the full
payload still reaches the client rather than trusting the rewind.

The rule is content-dependent, so **inferred kind is no longer a property of the name
alone** for one family. The glossary in `CONTEXT.md` says so.

Only the voice family is read. Every other family answers from its extension, because
no other extension has two readings this close, and reading every file to place it
would cost an I/O on every send for one family's ambiguity.

The same rule belongs in the sibling package, by the same algorithm and the same tests,
or the divergence this ADR exists to close simply moves.
