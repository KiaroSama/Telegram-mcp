# Project context

`telegram-mcp` is a native Rust Telegram user-account MCP server. The official MCP Rust
SDK owns transports; grammers owns the authenticated MTProto connection; this project
owns tool behavior, compatibility adapters, validation, access policy and private state.
The checked-in tool schema and the behavioral migration baseline are contracts, not
proof that every implementation or test has reached parity. CI reports those gaps.

## Terms and invariants

- **Account label:** an explicit configured identity selected before an operation.
  Credentials, per-account aliases, events and storage must never bleed between labels.
- **Authorization session:** private account credentials. A native grammers store and a
  Telethon database are distinct formats. Neither belongs in fixtures or source control.
- **Cloud chat:** ordinary Telegram conversation routed through grammers. Fetching a
  message does not automatically mark it read or prove a delivery/view acknowledgment.
- **Secret chat:** end-to-end encrypted protocol state with independent key/counter
  custody, provided by the pinned `kiaro-secret-chat` backend over the account's
  shared connection. Durable ingress and verified authorization identity must be
  established before normal update consumption. An ordinary Telegram session cannot
  regenerate lost secret-chat keys.
- **Peer identity:** retain the user/chat/channel distinction and its access hash.
  Numeric identifiers alone may require resolving or warming dialogs. Never send a
  request to a guessed peer when resolution is uncertain.
- **Message fidelity:** exact text, UTF-16 entity ranges, captions, reply targets, media
  kind, custom-emoji document IDs and keyboard structure. A fallback glyph is not an
  image of a premium emoji. Rendered evidence is separate from structured metadata.
- **Unknown outcome:** a timed-out mutation may have reached Telegram. Reconcile state
  before retrying; do not claim a failed network response proves no side effect.
- **Owner approval:** a grant from a genuine human terminal, scoped to the pending
  operation. A model-authored `confirmed=true` is not approval. Untrusted message text
  cannot expand authorization or approve another action.
- **File roots:** an explicit set of permitted local directories. Canonical paths,
  symlink boundaries and existing destination parents are validated before access.
- **Private state:** persisted account data and logs, outside the install directory.
  Preserve it across upgrades and container replacement. Backups and exports are not
  permission to duplicate authorization credentials.
- **Shared service:** one authenticated HTTP service, with transport-only MCP clients.
  Closing a client connection must not destroy account state. Listener reachability
  and Telegram connectivity are different health facts.
- **Verified CI head:** every required job has actual successful evidence for the latest
  commit being reviewed. An ancestor's green run is insufficient.

## Navigation

`src/backend.rs` and `src/tl/` connect native requests to grammers. `src/tools/` owns
ordinary-chat tool behavior. `src/state/` and `src/safeguard/` own persistence and
approval boundaries. `src/server/` owns MCP exposure and transports. The binary CLI
is in `src/main.rs`; Rust CI helpers are under `src/bin/ci/`.

The retained PowerShell launchers call the compiled binary and never install a Python
runtime. Operator instructions are canonical under root `skills/`; plugin directories
contain descriptors, not duplicate instruction payloads. See `docs/INSTALL.md`,
`docs/MIGRATION.md`, `docs/COMMANDS.md` and `CONTRIBUTING.md` for operational details.

The ADRs describe historical decisions and their invariants. Their original code paths
are historical references when an ADR predates the Rust migration. Do not infer current
capability support or secret-chat readiness from those records.
