# Native API coverage

The native catalogue has 391 tools: 356 cloud tools and 35 secret-chat tools.
Before startup, the server checks that every advertised tool has a native dispatch
route. The Rust tests exercise requests, responses and failures. The route check
alone does not establish parity.

| Native domain | Catalogue tools |
| --- | --- |
| `advanced` | 94 |
| `documents and upload preparation` | 2 |
| `identity` | 149 |
| `local` | 24 |
| `messaging` | 79 |
| `visual` | 8 |
| [Secret chats](COMMANDS.md#secret-chat-tools) | 35 |

See [COMMANDS.md](COMMANDS.md) for exact tool names and the runtime schema for current
arguments/defaults. Telegram API layer 234 is pinned with grammers 0.12.0. Native local
adapters bridge gaps in high-level grammers helpers through typed-schema-validated
raw requests rather than retaining a Telethon process. The pinned grammers sender
requests missed updates on the existing account pool after a home-DC disconnect.
`transcribe_audio` uses an allowed local file and returns Whisper segment times in
seconds. It has no hosted-engine or Telegram fallback.

## General topic

Telegram uses topic ID `1` to scope General, but General has no ordinary message to use as a send root. `send_message`, `reply_to_message`, `send_gif` and forwards use the ordinary chat route; a reply still names the message it replies to. Typing and mark-read operations keep the explicit topic scope.

`get_messages`, `list_messages` and `search_messages` verify the forum, filter General locally and count only matching messages toward page offsets. `scan_limit_reached` means the scan could not finish; it does not prove that no further General messages exist. See the [Telegram forum reference](https://core.telegram.org/api/forum), [reader regressions](../tests/rust_messaging_branches/general_and_layer234.rs), and [send regressions](../tests/rust_messaging_branches/send.rs).

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

## Secret chats

[`kiaro-secret-chat` 0.3.0](https://github.com/KiaroSama/Telethon-Secret-Chat) handles
native MTProto 2.0 secret chats over the existing account connection. The server
commits incoming encrypted updates to durable storage before acknowledging them.
On first open, it creates a read-only backup of a supported legacy secret-chat store
and migrates it after verifying authorization identity and before normal update
consumption. See the [migration notes](MIGRATION.md#secret-chat-store-migration)
and the [35 tools in six groups](COMMANDS.md#secret-chat-tools). The connected
backend automatically attempts the incoming handshake on `ChatRequested`.
`accept_secret_chat` is the owner-gated MCP tool for accepting a known pending
incoming chat by ID. Both routes use the same handshake and confirm readiness
before reporting success.
`export_secret_chat` does not support `peer_user` filtering. The runtime refuses a
nonempty filter instead of exporting an unfiltered chat.

## Restricted capabilities

Native window capture requires Windows. Optional TGS rendering requires a reviewed
rlottie shared library. Native export output is not necessarily byte-identical to
Telegram Desktop, Qt or mozjpeg output. See the [migration notes](MIGRATION.md) for
lifecycle and export limits.

Chat and forum-topic allowlists check source and destination and refuse operations
that cannot enforce the restricted scope. A newer Telegram layer does not make all
upstream Python behavior available.

Live Telegram sign-in, Premium-only features, administrator permissions, proxy routing
and server-side acceptance need real authorized credentials/inputs. Offline CI does not
claim those checks ran.

## Upstream sync (2026-11-27)

| Upstream | Revision | Decision |
| --- | --- | --- |
| [Lonami/grammers](https://codeberg.org/Lonami/grammers/commit/02367f9711ca3589a8bbb02e4128aca1631c77c4) | `02367f9711ca` (HEAD `11e13a989a75` checked) | Retain pinned layer 234 revision `02367f9711ca` for v3.3.1. |
| [KiaroSama/Telethon-Secret-Chat](https://github.com/KiaroSama/Telethon-Secret-Chat/commit/bf0fc26906f1) | `bf0fc26906f1` | Adopt the layer 234 revision with refreshed export integrity hashes and the corrected synthetic layer 234 fixture. |
| [chigwell/telegram-mcp](https://github.com/chigwell/telegram-mcp/commit/519899cca9) | `519899cca9` (`v3.3.31`) | [#412](https://github.com/chigwell/telegram-mcp/pull/412) (`view_video`) and [#420](https://github.com/chigwell/telegram-mcp/pull/420) (explicit forum `topic_id`) are ported. Open PRs [#421](https://github.com/chigwell/telegram-mcp/pull/421) (webhook draft), [#422](https://github.com/chigwell/telegram-mcp/pull/422) (draft forum topic), and [#411](https://github.com/chigwell/telegram-mcp/pull/411) (Python test speed) are deferred or not applicable. |
