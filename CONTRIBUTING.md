# Contributing

The service and its tooling use Rust 2024, with the checked-in Rust toolchain and
`Cargo.lock`. Windows launcher scripts are the explicit PowerShell exception.
Contributions are GPL-3.0-or-later; retain the notices for adapted upstream work.

## Work on the native implementation

Build with `cargo build --locked`. Change the source module that owns a behavior,
including its tool schema, validation, output, tests and documentation. Tools depend
on the mockable `backend::Telegram` interface; tests must prove request construction,
refusals, pagination and outcomes without using a real account.

Keep RPC results and user-controlled content out of diagnostics. Never log session
strings, auth keys, login codes, passwords, Mini App launch credentials, raw MCP
arguments or incoming Telegram content. Use explicit UTF-8 for text artifacts and
preserve entity offsets in UTF-16 code units.

## Required local checks

Use a disposable checkout without live Telegram credentials. Set `TELEGRAM_STATE_DIR`
to a private test directory and `TELEGRAM_FILE_ROOTS` to a JSON array of existing
fixture directories before running behavioral tests. CI creates these directories
separately for each leg under `RUNNER_TEMP`; tests must not share live account state.

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features -- --test-threads=1
cargo build --locked --release --bin telegram-mcp
cargo run --locked --bin ci -- privacy
cargo run --locked --bin ci -- privacy-history
```

The standard Cargo runner runs these tests serially because Unix subprocess tests
can briefly inherit a sibling test's open file lease before `exec` closes it.
This can make an immediate close/reopen assertion observe temporary contention.
CI uses Nextest's separate process for each case, preserving parallel execution
without sharing incidental lease descriptors between cases. For CI's bounded
runner, install the versions declared in `tests.yml` and run:

```bash
cargo nextest run --locked --all-features --profile ci
cargo run --locked --bin ci -- record linux-x86_64 target/nextest/ci/junit.xml cases/linux-x86_64.json
```

This example uses Linux x86_64. Select your actual OS/architecture label from
`.github/expected-native-legs.json`; another compiler host cannot be relabeled as that
target. The runner applies a three-minute stalled-test
ceiling. Tests that exercise external processes must also prove cleanup of their owned
children; stopping a test process alone is not proof its descendants exited.

Approval Bot API tests must distinguish an observed HTTP request from a consumed,
validated delivery acknowledgment. Exercise partial delivery with one owner's
response held, and keep owner, private-chat and message bindings enforced. For
approval-deadline assertions, use controlled Tokio time with an owned blocking gate
to prevent automatic advancement during real loopback I/O, plus an independent
wall-clock watchdog that releases on completion or unwinding. Poll the actual
operation at the boundary; preserve the semantic approval budget and verify the
decision, dispatch count, persisted grants, pending state and terminal prompt edits.

Secret-chat binding migrations must be tested with an authenticated historical v1
ciphertext fixture, not only an already-v2 reopen. Assert one-open recovery,
preservation of native keys/counters/ingress checkpoints, preflight refusals with
unchanged original files, a byte-identical read-only ciphertext backup before
replacement, and interrupted v2-store/v1-receipt recovery. Exercise backup resumption
and conflicts on both Windows and Linux; a Windows flush needs a write-capable
handle even when the retained file is made read-only before flushing.

## Preserve verification types

`resources/migration-baseline.json` records the pre-migration commit, suite inventory
and actual collected parameterized cases. The owner explicitly waived exact test-count
parity: replace language-specific tests with meaningful Rust behavior tests. Do not
inflate the count with repeated names, no-op tests or metadata-only assertions.

`.github/native-verification.json` names real executed tests for each critical behavior
category. `ci parity` checks their JUnit evidence. The catalogue dispatch test verifies
every ordinary-chat tool has a native route; domain tests must separately prove actual
requests, refusals and outcomes. Category membership and counts cannot establish
semantic equivalence, so reviewers must inspect assertions and known gaps.

Every base/renderer leg must match its reviewed operating-system inventory. Rust cfg
cases may differ across platforms; those differences must remain explicit. OS records
contain the tested commit, actual CPU architecture/native compiler host, lock digest,
case identities and outcomes.
`.github/expected-native-legs.json` declares the exact Linux/Windows base and
renderer set for both architectures. `.github/expected-native-tests.json` maps each OS to its reviewed native
case identities so deletion on all systems cannot silently pass. Update it when adding
or deliberately renaming tests. Missing records and unreviewed changes fail.

`tests/rust_launchers.rs` exercises the native account manager and the retained
PowerShell launchers. Its portable cases run with the native suite; Windows-only
cases in `tests/rust_launchers/windows.rs` run on both Windows architectures, including
the separate launcher jobs. Run `cargo test --locked --test rust_launchers` after
launcher changes. These tests invoke the actual compiled Rust executable where
native behavior is required and use a synthetic child for protocol/exit forwarding.
The four historical PowerShell suites remain recorded in the migration baseline;
they are not files executed by the current workflow. Release jobs separately invoke
all three actual release CLIs with `--help` on each supported architecture.

## CI and coverage

Pushes and PRs build release binaries for ARM64 and x86_64 on Linux and Windows before
behavioral parity is evaluated. Tests, formatting, Clippy, advisory audit, CodeQL,
privacy, Gitleaks current-tree/introduction scans, container replacement and Compose checks remain separate evidence.
The gated native coverage metric is executable application-core source lines: the
union of base and real-renderer reports from all four OS/architecture combinations must reach
85%. `.github/coverage-scope.json` records every source path and its original measurement
basis, including deterministic helpers now located inside tool modules. CI separately
publishes all-production coverage, including API adapters and development/skill helpers.
Scope changes require explicit review; new unclassified source fails the gate. This is
a different measurement from Python's former line/branch collector and must not be
presented as an unchanged numeric result.
Generated third-party protocol code is a dependency, not evidence of application tests.

Before instrumentation, CI runs `cargo llvm-cov clean --workspace` to remove stale
workspace objects/profiles while retaining dependencies. Coverage DA lines outside the
current reviewed source file are rejected.

The status reporter checks the actual run, attempt, job identities and head SHA before
publishing commit statuses. Push and pull-request contexts are distinct, so one
trigger cannot overwrite the other trigger's pending/final result. Do not replace a failing, skipped or missing job with an
unrelated successful workflow. Changes to job names must update
`.github/ci-status-jobs.json` in the same commit.

## Dependency updates and reproduction

After changing `Cargo.toml`, resolve and review `Cargo.lock`; do not use an unlocked CI
build to hide drift. grammers' Codeberg source revision is explicit. Read current
release notes and source before updating it or its Telegram layer; protocol-codec and
adapter tests must follow schema changes together. Dependabot watches Cargo, Actions
and Docker. The advisory job applies `cargo audit --deny warnings` with one explicit
exception: [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543.html),
the informational notice that `chrono` is unmaintained, with no patched version.
`chrono` is a direct dependency and is also required by locked transitive dependencies.
A passing advisory job therefore means the checked policy passed, not that the lockfile
has no known advisory. Review the exception when updating those dependencies.

The always-run Gitleaks gate scans every regular tracked source file and the exact
PR/push introduction range, including merge diffs. A separate manual historical audit
scans all fetched refs and can report inherited findings without rewriting history.
The custom reachable-blob shape scanner retains its original bounded-buffer and
exact verified synthetic-Qt-blob exception; it never exempts a path or rule family.

The manual reproduction workflow archives tracked source, a Git bundle and vendored
locked Rust dependencies, then proves an offline rebuild with an empty Cargo cache.
It does not export home directories, local configuration, sessions, environment dumps
or logs. A compatible Rust toolchain, native linker and system libraries are still
required; the artifact is not an operating-system image.
