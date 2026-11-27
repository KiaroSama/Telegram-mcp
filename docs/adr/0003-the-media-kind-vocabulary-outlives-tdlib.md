# The media kind vocabulary outlives TDLib

> Historical decision. Source paths refer to the former implementation. See the
> [Rust migration notes](../MIGRATION.md) for the current native backend and its limits.

## Decision

Before replacing TDLib with [Grammers Secret Chat](https://github.com/KiaroSama/Grammers-Secret-Chat), move the eight media kind names into a backend-independent module.
A CI test must check that an ordinary chat can send every kind in `KINDS`.

At the time, `telegram_mcp/secret_media_content.py` was the only file that named
all eight kinds, and it was due for removal with the TDLib tools. There was
already a gap: `send_secret_media` handled all eight, and `send_disappearing_media`
handled voice notes and video notes. On the ordinary permanent path, `send_file`
and `send_voice` could send an OGG voice note but not a video note or another
voice format. Keeping the vocabulary and its test independent of TDLib let us
catch those gaps before and after the switch.

## Considered options

- **Compare both repositories in CI.** The server did not yet depend on the new
  backend, so the test could not run until the switch. An in-repository test
  could run immediately. Its limit is that it cannot detect drift in the package.
- **Warn when one media file changes without the other.** A warning does not show
  whether a kind is reachable. The parity test checks that directly.
- **Share one kind resolver.** The send paths lived in separate repositories.
  They could agree on names before they could call the same implementation.
- **Convert incompatible input.** Refuse the request before uploading, without
  converting the operator's file. For example, do not turn an MP3 into a voice
  note or a wide video into a round one.

## Consequences

`send_voice`, `send_sticker` and `send_gif` keep their public names and use the
same kind resolution. Stickers and video notes still reject captions because
neither has a caption field in the protocol.
