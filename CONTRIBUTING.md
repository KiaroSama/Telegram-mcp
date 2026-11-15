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

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --locked --release --bin telegram-mcp
cargo run --locked --bin ci -- privacy
cargo run --locked --bin ci -- privacy-history
```

For CI's bounded runner, install the versions declared in `tests.yml` and run:

```bash
cargo nextest run --locked --all-features --profile ci
cargo run --locked --bin ci -- record linux target/nextest/ci/junit.xml cases/linux.json
```

Use your actual operating-system label. The runner applies a three-minute stalled-test
ceiling. Tests that exercise external processes must also prove cleanup of their owned
children; stopping a test process alone is not proof its descendants exited.

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
contain the tested commit, compiler, lock digest, case identities and outcomes.
`.github/expected-native-legs.json` declares the exact Linux/macOS/Windows base and
renderer set. `.github/expected-native-tests.json` maps each OS to its reviewed native
case identities so deletion on all systems cannot silently pass. Update it when adding
or deliberately renaming tests. Missing records and unreviewed changes fail.

Four preserved PowerShell suites are discovered as `tests/test_*.ps1` on Windows. Run
all of them after launcher changes; they must exercise the compiled Rust executable.
Do not remove a verification type merely because the application language changed.

## CI and coverage

Pushes and PRs always build release binaries for all three operating systems before
behavioral parity is evaluated. Tests, formatting, Clippy, advisory audit, CodeQL,
privacy, Gitleaks current-tree/introduction scans, container replacement and Compose checks remain separate evidence.
The gated native coverage metric is executable application-core source lines: the
union of base and real-renderer reports from all three operating systems must reach
85%. `.github/coverage-scope.json` records every source path and its original measurement
basis, including deterministic helpers now located inside tool modules. CI separately
publishes all-production coverage, including API adapters and development/skill helpers.
Scope changes require explicit review; new unclassified source fails the gate. This is
a different measurement from Python's former line/branch collector and must not be
presented as an unchanged numeric result.
Generated third-party protocol code is a dependency, not evidence of application tests.

The status reporter checks the actual run, attempt, job identities and head SHA before
publishing commit statuses. Do not replace a failing, skipped or missing job with an
unrelated successful workflow. Changes to job names must update
`.github/ci-status-jobs.json` in the same commit.

## Dependency updates and reproduction

After changing `Cargo.toml`, resolve and review `Cargo.lock`; do not use an unlocked CI
build to hide drift. grammers' Codeberg source revision is explicit. Read current
release notes and source before updating it or its Telegram layer; protocol-codec and
adapter tests must follow schema changes together. Dependabot watches Cargo, Actions
and Docker.

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
