# Updating the secret-chat package

Secret chats run on [`KiaroSama/Telethon-Secret-Chat`](https://github.com/KiaroSama/Telethon-Secret-Chat),
which is maintained in its own repository and released on its own schedule. This is how
this project takes a newer version of it.

## The short version

1. Read the package's `CHANGELOG.md`: every public API change is listed there, and a
   pre-1.0 minor version may break.
2. Edit one line in `pyproject.toml` — the `@<ref>` at the end of the
   `kiaro-telethon-secret-chat` requirement — to the full commit SHA you reviewed.
3. `uv lock --upgrade-package kiaro-telethon-secret-chat`
4. Commit both files, push, read CI.

A change the package makes to its API lands in `telegram_mcp/secret_backend.py` and the
tools that call it; CI names what broke.

## Why it is pinned by git and not by name

**The package is not on PyPI.** Its distribution is `kiaro-telethon-secret-chat`
since 0.1.0; before that it was called `telethon-secret-chat`, which on PyPI is
painor's unrelated, archived project at a *higher* version - a bare requirement
resolved to the wrong project and installed cleanly.

So the requirement names the repository:

```toml
"kiaro-telethon-secret-chat @ git+https://github.com/KiaroSama/Telethon-Secret-Chat.git@<ref>"
```

`<ref>` is a full commit SHA, including when selecting a tagged release: resolve the
tag to its commit first. The same commit is recorded in `uv.lock` and checked against
the installed package, so a tag label or unchanged version cannot hide a stale install.

`tests/test_secret_backend_contract.py` reads the installer's `direct_url.json` and
requires the exact repository and Git commit to match the manifest and resolved lockfile.
The version number alone is insufficient: newer commits may retain the same version.
An archived package or stale installed commit fails rather than shadowing the pinned one.

## Preserve application policy when defaults change

The owner-selected application default is now plaintext auto-save **ON** for current
and new accounts. A one-time policy migration under the store lease preserves a safe
chosen destination, otherwise uses `files/downloads/secret-chats/<account>`. Destination
validation and verified owner-only access precede startup or recovery writes; preference
and migration marker commit atomically. Failed validation/commit prevents activation.

After that migration, explicit `stop_secret_auto_save` persists OFF across restarts,
including a cold stop without first starting the saver. Existing saved copies remain.
Authorized accounts activate their manager on startup/admission, without a secret-tool call.
Saving can retain self-destructing content and is not encrypted at rest.

## Optional bounded integration

The earlier `735e8523047c2ddb6610e904a579c096292dc69a` introduced `BoundedSecretChatManager`.
Normal mode remains default. `configure_secret_backend` permits approved selection only
for an actually empty account store; occupied/corrupt state refuses without migration.
One manager rides the existing client: parallel normal/bounded subscriptions are unsafe.

Supply explicit `records`, `bytes`, `age_seconds`; optional defaults are `chats=64`,
`frame_bytes=262144`, `operation_seconds=15`, `tasks=8`. Hard ceilings are 4096 records,
64 MiB serialized custody, 30 days, 64 chats, 1 MiB frame, 60 seconds and 64 tasks.
These are protocol-custody budgets, not total RAM, client queues or archive-disk quotas.

Pause/check/status preserve records; explicit resume refuses missed required ingress.
Pending deliveries expose stable wire sequence identities and never acknowledge or read.
Explicit acknowledgment commits a private consumer result/media reference before separate
local settlement. It sends no read receipt and makes no exactly-once guarantee.
Capped RAM media receipt creates no content file itself; independent ON archive can still
save that media. Bounded close is a nonresumable preserving local stop, not peer erasure;
chat deletion/forget refuse. Protocol custody may contain plaintext and file keys.
Telethon transport acknowledgment can precede consumer custody: lossless replay is not
promised. Actual-package synthetic fixtures are not live cipher interoperability proof.

Committed consumer copies remain accessible to existing history/search/media tools even
when notification callbacks were dropped. Callback-missed copies use explicitly labeled
consumer-commit time for ordering, not an invented wire-arrival date; an ordered read
receipt is refused when arrival dates are unavailable. Consumer copies are independent
retained plaintext and are not evicted by upstream custody quotas.

Directory/privacy checks reject existing links before activation and harden inherited
archive subdirectories on restart. Windows retains the archive root handle while saving.
The library writes descendants by pathname: concurrent modification by another process
running as the same owner is outside this protection; this is not a sandbox against that
identity. Keep archive directories under the owner's exclusive control.

## Strong ingress: available upstream, not activated here

The current pin `d87bf037b90060383c7b704d8a06ebb98c621c6f` adds opt-in
`StrongSecretChatManager`, `DurableIngress`, `EncryptedIngressStorage` and
`IngressLimits`. Existing normal/bounded selection and archive preferences do not change.
`get_secret_backend_status` adds `strong_ingress` diagnostics: library availability,
verified transport prerequisite, integration/activation state and fixed blockers. Availability
or a verified prerequisite does **not** mean an integrated or active mode. This server still
accepts only `normal` and `bounded`; strong configuration is refused before account mutation.

Actual activation remains blocked, not implemented by the diagnostic:

- Exact normally installed `telethon==1.45.0+kiaro.ingress1` is required. Upstream verifies
  its provenance receipt, imported paths and five source hashes, not just a version label.
  Official Telethon remains selected here. The upstream `[strong]` extra supplies
  `cryptography==50.0.2`, **not** the patched transport; ordinary syncing can replace a
  manually installed patched wheel. Never edit installed files or monkeypatch the transport.
- `ingress.prepare` must precede connection on the already owned authorization. Fresh
  storage needs a verified continuity baseline; a new StringSession's empty counters are
  not that proof. Reopening instead retains the same caller-owned 32-byte key, account/auth/DC
  binding and stable store generation. Do not put these values into MCP arguments or output.
- Strong forces package plaintext auto-save **OFF** and refuses starting it. Selecting it
  therefore cannot preserve the owner's current ON default; existing archives stay untouched.
  Independent application history/consumer copies would still be plaintext, not encrypted
  merely because upstream custody is encrypted.
- Required aggregate ingress budgets are records/bytes/age, separate from per-chat custody.
  Defaults are batch bytes 1048576 and operation seconds 15; ceilings are 4096 records,
  64 MiB aggregate bytes, 4 MiB batch, 30 days and 60 seconds. Full/expired/failed ingress can
  block **all same-account RPCs**, including ordinary cloud operations.
- Existing GetState recovery cannot run with strong ingress. Stop permanently fences the
  coordinator; safe recovery needs confirmed retirement/disconnection/store close and a
  reconstructed generation, not clearing private stopped fields or retrying the same client.

The added diagnostic constructs no encrypted store, fetches no baseline and generates no key.
The existing backend-status call retains its prior lazy manager startup when needed. They never
return raw exception text, bindings or retained envelopes. Neither this upgrade nor upstream
synthetic tests prove live strong-mode interoperability, lossless historical recovery,
anti-rollback, physical erasure, total-process RAM quotas or exactly-once external sends.

Sources: [strong contract](https://github.com/KiaroSama/Telethon-Secret-Chat/blob/d87bf037b90060383c7b704d8a06ebb98c621c6f/docs/strong-ingress.md)
and [transport prerequisite](https://github.com/KiaroSama/Telethon-Secret-Chat/blob/d87bf037b90060383c7b704d8a06ebb98c621c6f/prerequisites/telethon/README.md).

## Why API adaptation stays localized

Two properties, both enforced by tests rather than by convention:

**One importer.** `telegram_mcp/secret_backend.py` is the only module in this project
that imports `telethon_secret_chat`. Every secret-chat tool goes through
`secret_manager(account)`. A test walks the package's syntax tree and fails if any other
module imports it, so an upstream signature change stays a one-file edit instead of
becoming a search across nineteen call sites.

**Written-down assumptions.** The operations this project calls, the manager's
constructor shape, the storage backend's shape and the package's public exports are all
asserted in `tests/test_secret_backend_contract.py`. They do not test the package's
correctness — its own suite owns that. They test the boundary, so a version that renamed
`flush_history` or dropped `send_file` fails in CI naming what it broke, rather than
raising `AttributeError` inside a tool at whatever moment the operator happened to use
that path.

One of those tests deliberately builds an incompatible stand-in and asserts the guard
catches it. A guard nobody has watched fail is a guard nobody knows works.

## When CI does go red after an update

Read which contract test failed; it names the operation or the signature that changed.
Then:

- **An operation was renamed or its arguments moved** — adapt
  `telegram_mcp/secret_backend.py`, and update `CALLED_OPERATIONS` in the contract test
  in the same commit.
- **`storage` gained a default** — do not accept it silently. The package refusing to
  guess where key material goes is a safety property, and a default would mean this
  server starts writing keys somewhere the operator did not choose.
- **The distribution check failed** — the pin did not hold and something installed the
  archived package from the index. Re-run the lock command; do not "fix" the test.
- **Nothing is obviously wrong** — pin back to the previous ref, which is one line, and
  reproduce against the package's own suite in its repository.

## What this project does NOT do

It does not vendor the package, patch it at runtime, or work around a defect in it. A
defect belongs upstream, in its own repository, with its own test. Pinning back is the
correct response here while that happens.
