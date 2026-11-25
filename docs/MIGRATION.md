# Rust migration scope and evidence

The Rust rewrite started from `KiaroSama/Telegram-mcp` commit
`bd4b454c5392ef084ab265b92ca89ebc04d596df`. The core rewrite shipped in `v3.0.0`.
The native secret-chat integration for `v3.1.0` has since merged into `main`.

## Authorized scope

- Rewrite the application, local helpers and automated behavior tests in Rust.
- Retain `Manage-Accounts.ps1` and `start-mcp.ps1` as the only PowerShell launchers.
  The account manager helpers are native Rust, and the launcher tests are in
  `tests/rust_launchers.rs` and its modules.
- Replace Telethon with grammers and implement missing application adapters locally.
- Preserve verification types and CI quality; exact legacy test-count equality was
  explicitly waived in favor of meaningful tests appropriate to Rust.
- Build Windows and Linux releases for x64 and arm64 and report checks against the
  exact commit. Build success alone does not establish runtime behavior.
- Review upstream changes and dependency documentation.
- Use `kiaro-secret-chat` 0.3.0 for native MTProto 2.0 secret chats, without the legacy
  Python runtime. Version 3.2.0 exposes 390 native tools: 356 ordinary tools and all
  34 [secret-chat tools](COMMANDS.md#secret-chat-tools).

## Source revisions reviewed

The core upstream review took place on 2026-11-15 and 2026-11-16. The selected
Chigwell changes through #419 were reviewed on 2026-11-24. Table dates refer to
commits or releases. Historical byte-oracle versions remain distinct from current sources.

| Source | Reviewed revision | Source date | Review and adaptation scope |
| --- | --- | --- | --- |
| This fork | `bd4b454c5392ef084ab265b92ca89ebc04d596df` | Migration base | Existing tools, account state, approvals and fidelity contracts |
| [grammers](https://codeberg.org/Lonami/grammers/commit/02367f9711ca3589a8bbb02e4128aca1631c77c4) | `02367f9711ca3589a8bbb02e4128aca1631c77c4`, 0.12.0 | 2026-11-25 | Native Telegram transport, layer 234 TL schema, update and authorization custody |
| [chigwell/telegram-mcp](https://github.com/chigwell/telegram-mcp/commit/b1263b564c834b9eb91daae1667790a4247b2b8f) | `b1263b564c834b9eb91daae1667790a4247b2b8f` | 2026-11-23 | Selected changes through #419: formatting kinds, reconnect catch-up, private-chat leave, exact photo IDs, local Whisper serialization and timestamps, and bounded login retries |
| [Official MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk/commit/0e2396f73a88617141b31f645de6f1171ed5f993) | `0e2396f73a88617141b31f645de6f1171ed5f993`; rmcp 3.10 API (migration review) | 2026-11-15 | Stdio, authenticated streamable HTTP, shared-session bridge and cancellation. The current dependency is [rmcp 3.11.0](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.11.0). |
| [Telegram Desktop](https://github.com/telegramdesktop/tdesktop/releases/tag/v7.4.4) | v7.4.4, `bf4f714036f21c750d535dab199c811b3f9bfdd4` | 2026-11-13 | Current export changes: date cursors, migrated groups and JSON/HTML escaping |
| [Qt qtbase](https://github.com/qt/qtbase/tree/v6.12.0) | v6.12.0, `025bdad181de241e81bf853c8a2d7bf3d19261a9` | 2026-09-26 | Current JPEG allocation and ICC validation changes; Qt 5.15.19 remains the historical oracle |
| [mozjpeg](https://github.com/mozilla/mozjpeg/tree/v4.1.5) | v4.1.5, `6c9f0897afa1c2738d7222a0a9ab49e8b536a267`; master `08265790774cd0714832c9e675522acbe5581437` | 2023-10-12; 2025-06-23 | Latest stable tag and build changes; native Rust encoder does not claim mozjpeg bitstream parity |
| [Samsung/rlottie](https://github.com/Samsung/rlottie/commit/ea06d2f29ba01b8d06c00a838d107f5e484ae59b) | `ea06d2f29ba01b8d06c00a838d107f5e484ae59b` | 2026-09-30 | Allocation/stride guards and pinned external shared library for real animated-sticker rendering |
| [FFmpeg Windows builds](https://ffmpeg.org/download.html) | Gyan 9.0.2 essentials ZIP, SHA-256 `60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba` | 2026-09-20 | Actual native FFmpeg/FFprobe test dependency; official download guidance links this build provider |
| [FFmpeg Windows ARM64 builds](https://github.com/tordona/ffmpeg-win-arm64/releases/tag/9.0.2) | Tordona 9.0.2 essentials-static ARM64 7z, SHA-256 `5319165d85ecee87360166dc0198b6de7747be6ce7a97a9f59b3920eddb6a3c3` | 2026-09-18 | Publisher archive/checksum and native PE headers reviewed on November 16; includes required VP9-alpha and Opus codecs |

The grammers GitHub mirror is stale relative to Codeberg; the live Codeberg source was
read directly. Search excerpts that still named 0.10.0 were not used as version proof.
The grammers and [native secret-chat](https://github.com/KiaroSama/Telethon-Secret-Chat/commit/84f2b8e57f30af10218effb7cb58688a57431e0d) pins now share Layer 234. The former Telethon source pin used layer 232.

Version 3.2.0 implements General topic reads and video review as native tools. The public contracts were checked against [Chigwell #420](https://github.com/chigwell/telegram-mcp/pull/420) and [#412](https://github.com/chigwell/telegram-mcp/pull/412).

Upstream changes are adapted to this fork's validation, account isolation, approvals,
filesystem and exact-text contracts. A blind merge of upstream Python code would not
satisfy those contracts or the Rust requirement. Attribution remains in `NOTICE`.

For #409, grammers already signals a home-DC disconnect and requests a difference
from the existing update cursor. The Rust service keeps its single authenticated
session per account. For #415, timestamped file transcription is local-only; no
hosted upload fallback was carried over. For #419, grammers reuses the current
code token and maps Telegram's `PHONE_CODE_*` responses to one error, so login
limits retries and tells the operator to restart if the code expired.

### Export and rendering review

Telegram Desktop's November 11 change
[`08c62ec65df145d90069f970529309e766c69f77`](https://github.com/telegramdesktop/tdesktop/commit/08c62ec65df145d90069f970529309e766c69f77)
uses a start-date cursor in its oldest-first walk, accounting for Telegram's exclusive
`offset_date`. The native exporter walks newest-first, so it needs the corresponding
upper-date cursor and then ID pagination, rather than copying the upstream direction.
The November 2 migrated-group change
[`c81f092c324baf65041ba4b120748cfdf1f1f2b9`](https://github.com/telegramdesktop/tdesktop/commit/c81f092c324baf65041ba4b120748cfdf1f1f2b9)
validates the old group's migration target before joining its history. Current writer
changes reviewed also cover U+2028/U+2029, JSON control characters and nonfinite values.
Native tests establish the implemented behavior; the review itself is not a parity claim.

Qt's current JPEG allocation-limit change
[`bf14121e7e9c1c496c328142b3e3b72d78e03f82`](https://github.com/qt/qtbase/commit/bf14121e7e9c1c496c328142b3e3b72d78e03f82)
and invalid-ICC handling reinforce the need for bounded decode allocations. The native
implementation applies its own pixel/byte budgets and refusal paths. It does not embed
Qt, reproduce all Qt 6 color transforms or upgrade historical Qt 5 oracle expectations
by renaming them. The latest mozjpeg master changes reviewed concern build configuration;
the Rust encoder's output remains subject to the explicit fidelity limit below.

The optional rlottie renderer is loaded through a Rust C-ABI adapter. CI builds the
exact external revision above and requires distinct decoded frames with transparency;
a missing shared library cannot turn that renderer job green. Its C++ dependency source
is fetched outside this repository. Ordinary tests separately exercise safe fallback.
On Unix ARM64 the pinned upstream NEON implementation lacks the pixman assembly
helpers supplied for ARM32. CI selects its existing scalar C++ implementation with `-U__ARM_NEON__`;
the same required rendering assertions still run.

## Verification baseline

The original locked environment was collected with Python 3.13 and all extras:
**6,117 collected cases**, **302 Python suites**, **four PowerShell suites**.
`resources/migration-baseline.json` retains the source revision, definitions and actual
parameterized collection. It is audit data. The user explicitly waived exact native
case-count equality, so no test-name multiplication or synthetic one-to-one mapping is
used to claim parity.

Native CI preserves distinct gates for locked dependency resolution, behavioral tests,
actual renderer execution, Windows launchers, reviewed per-OS case inventories and base/renderer agreement on both CPU architectures,
coverage, advisory audit, static analysis, tree/history privacy, Gitleaks, container
build/persistence and Compose validation. `.github/native-verification.json` names actual
executed tests covering the critical behavior categories. Their assertions, not their
names, establish the practical limits of coverage.

Release builds run before behavioral gates. Artifacts can exist for a commit whose
other checks failed; read the full exact-head status set. Cancelled, skipped, missing,
wrong-commit or stale-attempt evidence never produces a successful custom status.
Each of the four Windows and Linux CI build artifacts includes `telegram-mcp`,
`emoji-studio` and `operator-plugin`, plus `build.json` containing per-binary SHA-256 and
commit/compiler/lock provenance after executable machine-header validation. The signed
GitHub Release archives use the `bin/` layout documented in [INSTALL.md](INSTALL.md),
with archive checksums, detached signatures and GitHub attestations; they do not contain
the CI artifact's `build.json`. Case reports record the actual operating system,
CPU architecture and native compiler host; all expected base/renderer legs are required.
The Docker image provides the shared server; it does not currently distribute the two
standalone helper CLIs.

Gitleaks scans the entire current tracked tree and the exact introduced PR/push range.
New branches use the verified merge-base with origin/main. Full-history Gitleaks is a
separate explicit manual audit: the inherited history produced 26 generic-api-key
findings during migration review, reported only as redacted locations. Those were not
silently suppressed or presented as new-change failures, and no history was rewritten.
The independent reachable-blob shape scanner retains its original narrower patterns
and exact verified Qt oracle-blob exception. Neither gate claims universal credential
or byte coverage.

The 85% threshold is retained for **combined executable application-core source-line
coverage** across base and renderer legs on Windows and Linux for x64 and arm64. The reviewed
file list and the SHA-256 of the original `pyproject.toml` are recorded in
`.github/coverage-scope.json`. The original collector explicitly measured deterministic
application helpers and omitted tool adapters and development/skill scripts. Native
helpers now located under `src/tools` remain measured: media transfer/send/preview,
buttons, messages/rich quotes, local state/proxies, visual rendering, rights patches
and global-search cursors. Mixed files are measured in full. New source files require
an explicit scope review; moving a helper into an adapter directory cannot silently
remove it from the denominator.

CI also publishes **all-production source-line coverage**, including the excluded API
adapters and native CI/distribution/Emoji Studio helpers, alongside the gated core
metric. Their behavioral suites still run. Neither test assertion lines nor dependency
source contributes to these percentages. This is an explicit metric change from the
old Python line/branch collector, not a claim that the two percentages measure
identical code. A clean post-review Linux checkpoint passed all **1,853 tests**, with
zero failures or skips and actual rlottie rendering required. Unique physical-line
coverage measured **88.06% reviewed core (16,999 of 19,303 executable source lines)**
and **85.57% all production (23,132 of 27,032 executable source lines)**. Exact source
fingerprints and original test-run identifiers remain in the
individual reports. This is single-platform evidence; the combined CI gate requires
actual reports from every configured OS/architecture leg. Reviewed inventories for
other targets are expectations until those targets execute them. Raw LLVM JSON region
totals can count repeated instantiations and are not substituted for this metric.

Every instrumented leg first runs `cargo llvm-cov clean --workspace`, preserving its
dependency cache while removing older workspace objects and raw profiles. Using
`--no-report` alone retains those artifacts. The collector accepts only reviewed Rust
source paths and positive DA lines within the current file; stale maps cannot silently
enter a coverage percentage.

Windows coverage uses cargo-llvm-cov 0.9.1's supported `--no-rustc-wrapper` mode.
The original wrapper loses instrumentation when Cargo passes compiler arguments through
a Windows response file. Direct-argument and response-file reproductions isolated the
failure; upstream fixed argument expansion in
[PR 533](https://github.com/taiki-e/cargo-llvm-cov/pull/533), commit
`dc927946e73a6ecf18b797f2995490e922662387`. The workflow fallback places the same
instrumentation flags inside Cargo's response file and still requires actual profiles.
On native Windows ARM64, Rust 1.100 with the default MSVC linker produced real profiles
that its bundled LLVM rejected with `symbol name is empty`, matching the symptom in
[rust-lang/rust#150123](https://github.com/rust-lang/rust/issues/150123).
CI probes both that linker and the same pinned host toolchain's bundled `rust-lld`,
retaining their binaries, raw profiles and diagnostics. A linker is accepted only
after two real native processes, profile merging, positive function counters and
positive executable/covered lines for the actual smoke source all succeed. The
accepted linker and hash are then required for coverage, case-recorder and documentation
builds; bundled LLD uses the stable `lld-link` flavor. Release packaging uses the normal
linker before the accepted coverage configuration is applied. If neither linker passes,
the leg fails. A release build alone cannot
establish that instrumentation works on a new target.

## Secret-chat store migration

Version 3.2.0 uses [`kiaro-secret-chat` 0.3.0](https://github.com/KiaroSama/Telethon-Secret-Chat)
for native MTProto 2.0 secret chats. Ordinary and secret chats share one connection
per authorization key. The server commits incoming encrypted updates to durable
storage before acknowledging them.

The first open of an account with a supported legacy secret-chat store takes an
exclusive lock and makes a byte-for-byte, read-only backup under
`<state_root>/secret-chats/legacy-backups/`. The server then migrates chats, keys,
sequence counters, settings, saved history and media metadata to the native v1 store
without loss. It leaves the original files intact. A successful reopen uses the
native store without rerunning migration or replacing the backup.

If an imported session lacks a cached self identity, startup first verifies its owner
with a bounded `InvokeWithoutUpdates` request. Backup, migration and custody
initialization must then succeed before the normal update consumer starts. A failure
refuses that startup. Keep the original files and backups private. Do not
delete them to bypass an error. The automatic importer covers supported legacy
layouts only. Use [`check_secret_custody`](COMMANDS.md#custody-pause-delivery-and-backend-control-9)
to check custody status.

## Telegram Operator plugin upgrade

`telegram-operator` 1.5.0 contains skills only. Upgrading removes obsolete bundled
MCP configuration. If your client used that configuration, [add the MCP server
separately](../README.md#connect-an-mcp-client).

## Deliberate limits and migration cautions

**Authorization identity:** cross-label authorization checks compare 256-byte keys
in constant time. The server zeroizes temporary comparison buffers and leaves
original configuration and persisted session stores unchanged.

**Session formats:** native grammers state is distinct from Telethon SQLite/string
sessions. Configured supported StringSession and SQLite schema7/8 authorization is
validated and imported into native state through a read-only source copy. The original
bytes and opaque secret-chat sidecars remain unchanged. Active/WAL, incompatible,
changed-generation or conflicting authorization refuses import rather than resetting
it. Staged login/repair additionally binds the actual configuration key/digest and
expected Telegram identity before committing a replacement.

**Windows persistence:** directory barriers open a writable directory handle, verify
that the same handle belongs to NTFS, then flush it. The barrier covers private journal
publication, affected move parents, configuration publication and recovery cleanup.
Unsupported filesystems and access failures are rejected before new transactions;
post-publication uncertainty preserves recovery intent. Microsoft
[MS-FSA 2.1.5.7](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/0de7dc40-9627-437e-a4df-c4696cdc3d02)
defines persistence of the directory structure, while
[footnote 80](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63#Appendix_A_80)
limits this guarantee to NTFS. A successful no-op on another filesystem is not treated
as durable. The implementation uses ordinary directory access rather than a privileged
whole-volume flush.

Journaled account creation, rename and removal report success after their data,
configuration and completion receipt are durable. A published operation whose
durability is still unconfirmed returns a recovery-pending error and retains its intent;
do not repeat the login or
mutation until recovery completes. Completed receipts only authorize cleanup, so a
receipt that reappears after a crash cannot undo an acknowledged operation or replace
newer account data.
Ordinary state writes confirm and retire completed rename/removal receipts automatically
under state ownership; prepared, malformed or foreign receipts remain fenced until
trusted recovery.

**Rendered evidence:** structured Telegram metadata and rendered images are separate.
Native window capture is Windows-specific; other platforms report that restriction.
Optional TGS decoding requires the configured rlottie shared library. CI's renderer
legs require actual rendering and must fail when the library is absent.

**Exports:** Rust exporters preserve bounded pagination, exact text/entity handling,
media-path containment and honest job outcomes. A readable native export is not proof
of byte-identical Telegram Desktop/Qt ICC/mozjpeg output. Keep any remaining formatting,
color-management or thumbnail differences explicit in the export tests and PR report.
Unread snapshots bind resume to both the configured account and the actual authenticated
`telegram_user_id`. Older snapshots lacking that identity require a new output path;
an unsuccessful resume preserves the original bytes. The 32 MiB limit includes pretty
JSON and its final newline. A saved partial snapshot reports a `byte_budget` cursor
and can resume without marking Telegram messages read.

**Service lifecycle:** account-store exclusivity, bearer authentication and explicit
unknown mutation outcomes apply to the native runtime. Historical Python supervisor
or Win32 process-tree guarantees must not be inferred without corresponding native
implementation and tests.

**Live Telegram proof:** offline tests use synthetic fixtures and mock transports.
Without credentials and explicit authorized live inputs, real sign-in, Premium-only
features, permissions, network routing and server responses remain untested.

## Official references

- [grammers source and README](https://codeberg.org/Lonami/grammers)
- [grammers 0.12.0 source revision](https://codeberg.org/Lonami/grammers/commit/02367f9711ca3589a8bbb02e4128aca1631c77c4)
- [kiaro-secret-chat source](https://github.com/KiaroSama/Telethon-Secret-Chat)
- [Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Current upstream Telegram MCP](https://github.com/chigwell/telegram-mcp)
- [CodeQL Rust build-mode support](https://docs.github.com/en/code-security/reference/code-scanning/codeql/build-options-for-compiled-languages#building-rust)
- [CodeQL 2.27.2 Rust improvements](https://codeql.github.com/docs/codeql-overview/codeql-changelog/codeql-cli-2.27.2/#rust)

GitHub's current documentation supports Rust CodeQL analysis with language `rust` and
build mode `none`; rust-analyzer still evaluates build scripts/macros. The release-build
jobs therefore remain necessary independently of CodeQL.
