# An `.ogg` is a voice note until its tags say otherwise

> Historical decision. Source paths refer to the former implementation. See the
> [Rust migration notes](../MIGRATION.md) for current native behavior and its limits.

Without an explicit `kind`, this server treated `.ogg` files as `voice_note`;
[Grammers Secret Chat](https://github.com/KiaroSama/Grammers-Secret-Chat) treated
them as `audio`. Switching away from TDLib would have changed how existing calls
sent the same file.

## Decision

When the caller omits `kind` for an `.ogg`, inspect its Vorbis comments. A music
tag such as `TITLE`, `ARTIST` or `ALBUM` means `audio`; without a music tag, use
`voice_note`. An explicit `kind` takes precedence. Other file families do not
need this check.

On 2026-09-20, a music file and a recording made like a Telegram voice note had
these properties:

| Field | Music `.ogg` | Voice note |
| --- | --- | --- |
| Container | Ogg | Ogg |
| Codec | Opus | Opus |
| Channels | 1 (mono) | 1 (mono) |
| Sample rate | 48000 | 48000 |
| `ENCODER` tag | Present | Present |
| `TITLE` tag | Present | Absent |

Neither codec nor channel count distinguished the samples. Reading up to 64 KiB
for tags required no decoder. A duration cutoff would add work without telling
us whether the file contains speech or music. Always choosing one kind would
misclassify one of the samples. Re-tagging or converting an upload would change
the caller's file, contrary to [ADR 0003](0003-the-media-kind-vocabulary-outlives-tdlib.md).

## Consequences

A track with its music tags stripped defaults to a voice note. The caller can
still request `kind="audio"`. The MIME type on the outgoing message matters:
with `audio/ogg`, Telegram showed the `.ogg` as a voice note even though the
legacy client's `get_attributes` returned `DocumentAttributeAudio(voice=False)`.
An `.mp3` with the same attribute appeared as audio. A working `.ogg` track
showed why: its MIME type was `audio/vorbis`.

Sending the same `.ogg` twice confirmed the distinction: `kind="audio"` with
`audio/vorbis` appeared as a track; `kind="voice_note"` with `audio/ogg` appeared
as a voice note. Other track formats keep their usual MIME type.

The legacy client uploaded from the current file position. After inspecting the
header, the sender had to rewind; otherwise the call could report success after
sending only the rest of the file. `tests/test_routed_send_tools.py` checked the
entire payload. Both repositories need the same inference rule and test cases.
