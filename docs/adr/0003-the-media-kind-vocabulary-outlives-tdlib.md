# The media kind vocabulary outlives TDLib

The eight media kinds live in `telegram_mcp/secret_media_content.py` as `KINDS`,
next to their only caller: the TDLib-backed secret chat tools. That file is
scheduled for deletion. TDLib is being replaced by the Telethon Secret Chat
package, and when it goes, the one place in this repository that names all eight
kinds goes with it — taking with it the only description of what a complete media
surface looks like.

So the vocabulary moves out first, into a module that names no backend, and a test
in CI asserts kind parity: every name in `KINDS` is reachable from an ordinary
chat. The test runs today against the current surface and keeps running after
TDLib is gone, because neither the vocabulary nor the assertion mentions TDLib.

The asymmetry it exists to close is real and was measured, not imagined. Today
`send_secret_media` carries all eight kinds and `send_disappearing_media` carries
voice notes and video notes, while the ordinary permanent path — `send_file` plus
`send_voice` — carries neither a video note nor anything but an OGG voice. A
capability reachable in a secret chat and missing from an ordinary one is exactly
the shape of defect nobody reports, because the caller assumes the richer path is
the special case.

## Considered options

**A parity test spanning both repositories** — comparing the package's kinds
against this server's — was the obvious reading of "keep the two in step", and was
rejected. The two live in separate repositories and this server does not depend on
the package yet, so such a test cannot run until after the migration: it would be
written now and first executed at the moment it is least wanted, in the middle of
a backend swap. The in-repo test runs from the day it is written.

**A Hook-Maker hook warning when one media file is edited without the other** was
rejected because a hook reminds and a test refuses. The parity relation is exactly
the kind of claim a machine can check, and a reminder that can be read past is
worth less than a red build.

**A shared code seam both paths call** — one function owning kind resolution for
ordinary and secret sends alike — is the right end state and is not reachable from
here: the two paths are in different repositories today. The shared VOCABULARY is
the part that can move now, and it is what makes the seam cheap later.

**Auto-converting a file that cannot be the requested kind** (an MP3 asked to be a
voice note, a wide video asked to be a round one) was rejected. This server's
established answer is to refuse before uploading, because Telegram refuses after
the bytes have crossed and names neither the file nor the kind. Converting would
also send the operator something they never saw.

## Consequences

The package reaches the same eight names, so after the migration the two sides
agree by construction rather than by review. Until then the parity test measures
one repository against itself, which is weaker than the cross-repository check it
replaces — it cannot catch the package falling behind, only this server.

`send_voice`, `send_sticker` and `send_gif` stay. They are the names callers
already know, and they become thin routes into the same path rather than separate
implementations, so a fix to kind resolution reaches them without being applied
three more times.

Two kinds refuse a caption — sticker and video note — because the protocol gives
them no caption field. That rule already existed as `_NO_CAPTION` beside the
kinds and moves with them, rather than being rediscovered on the ordinary path.
