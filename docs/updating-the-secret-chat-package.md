# Updating the native secret-chat backend

Version 3.1.0 integrates `kiaro-secret-chat` 0.3.0 from
[KiaroSama/Telethon-Secret-Chat](https://github.com/KiaroSama/Telethon-Secret-Chat).
The package is native Rust. `Cargo.toml` pins the backend and its matching
`grammers-client`, `grammers-mtsender` and `grammers-session` crates to the same exact
repository revision. The upstream protocol crates retain their separate explicit
Codeberg revision. Review both `Cargo.toml` and `Cargo.lock` when changing this graph.

## Integration boundaries

`src/secret/runtime.rs` prepares authorization-bound storage and durable ingress,
then attaches the backend to the account's shared sender. `src/backend/startup.rs`
verifies an imported authorization's Telegram self identity when it is missing from
the local session, using a bounded request that suppresses updates. Normal update
consumption must not start until custody is ready. Disabling exposed secret-chat
tools alone cannot protect encrypted updates from an ordinary difference request.

`src/secret/migration.rs` handles supported legacy stores, private backup copies and
migration receipts. `src/secret/history.rs` owns local history and media references;
`src/secret/export.rs` owns local export formats. The tool routing and approval
boundaries are in `src/tools/secret.rs` and the shared server safeguards. The current
[secret-chat command catalogue](COMMANDS.md#secret-chat-tools) documents the exposed
operations; an old tool name is not proof that a new backend preserves its behavior.

## Review a backend update

1. Read the candidate backend's source, release notes and tests. Check changes to
   authorization binding, store formats, key and sequence-counter custody, ingress
   checkpoints, bounded queues, recovery, send cancellation and unknown outcomes.
2. Update the compatible fork crates together, resolve `Cargo.lock`, and inspect the
   resulting source revisions and protocol layer. Do not mix an unreviewed sender or
   session implementation into the custody path.
3. Run the native secret-chat migration, custody, tool and local-history tests with
   synthetic data, then the full checks in [CONTRIBUTING.md](../CONTRIBUTING.md).
   Add behavior tests for any changed migration or protocol boundary, and update the
   reviewed native case inventory for added or renamed tests.
4. Verify the required native CI jobs on the actual commit. Offline tests establish
   local requests, storage and refusal behavior; real Telegram interoperability
   requires separate authorized accounts and explicit live inputs.

Keep every existing private store, counter, pending receipt and archive intact during
an upgrade. Supported legacy data is backed up before migration, and an incompatible
or conflicting store must refuse startup rather than be reset. See
[secret-chat store migration](MIGRATION.md#secret-chat-store-migration) for the supported
path. A grammers authorization session or a message-content export cannot regenerate
lost chat keys or one-time media keys.
