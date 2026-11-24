# Native API coverage

The native catalogue currently contains 352 nonsecret tools. The server validates
that every advertised tool has a native dispatch route before startup. A route is not
by itself proof of semantic parity: actual request/response, validation and failure
behavior is exercised by the Rust suites and the critical verification category gate.

| Native domain | Catalogue tools |
| --- | --- |
| `advanced` | 94 |
| `documents and upload preparation` | 2 |
| `identity` | 148 |
| `local` | 24 |
| `messaging` | 76 |
| `visual` | 8 |

See [COMMANDS.md](COMMANDS.md) for exact tool names and the runtime schema for current
arguments/defaults. Telegram API layer 233 is pinned with grammers 0.12.0. Native local
adapters bridge gaps in high-level grammers helpers through typed-schema-validated
raw requests rather than retaining a Telethon process. The pinned grammers sender
requests missed updates on the existing account pool after a home-DC disconnect.
`transcribe_audio` uses an allowed local file and returns Whisper segment times in
seconds. It has no hosted-engine or Telegram fallback.

## Verification boundaries

- Protocol tests verify TL flags, integer widths, vector/bytes encoding, schema drift
  and refusal of malformed values.
- Domain tests use a mockable Telegram backend to verify real RPC method/argument
  construction, permission-sensitive branches, paging limits and explicit partial
  or unknown outcomes.
- Storage and approval tests exercise account isolation, valid UTF-8, atomic state
  updates, path containment, single-use owner grants and untrusted content handling.
- Media, visual and export tests distinguish actual bytes/rendered frames from
  structured metadata, and report platform/runtime restrictions explicitly.
- Native build, renderer and launcher CI jobs provide operating-system-specific proof.

The historical baseline is 6,117 collected Python cases across 302 suites plus four PowerShell
suites. Exact native test-count equality was explicitly waived. The native test inventory
is independently recorded so silent deletion remains detectable. Combined application-core
source-line coverage must reach 85%, with the full all-production metric published
separately and each scope decision recorded in `.github/coverage-scope.json`. Test counts and category labels do not substitute for
reviewing the assertions or for authorized live Telegram checks.

## Deferred and restricted capabilities

Secret-chat integration awaits the owner’s Rust rewrite of the separate encryption
backend. No encrypted chat/key migration, peer erasure or secret archive parity is
claimed. Native Windows window capture remains platform-specific. Optional TGS rendering
requires a reviewed rlottie shared library. Export output must not be described as
byte-identical Telegram Desktop/Qt ICC/mozjpeg output unless its actual oracle tests prove
that property. Missing native lifecycle guarantees remain explicit in
[migration notes](MIGRATION.md).

Useful upstream changes are reviewed at exact revisions listed in MIGRATION.md. Chat
and forum-topic allowlists preserve source/destination checking and refusal of operations
that cannot enforce the restricted scope. A newer Telegram layer does not make all
upstream Python behavior automatically available.

Live Telegram sign-in, Premium-only features, administrator permissions, proxy routing
and server-side acceptance need real authorized credentials/inputs. Offline CI does not
claim those checks ran.
