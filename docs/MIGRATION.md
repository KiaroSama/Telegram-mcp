# Rust migration scope and evidence

The Rust branch starts from `KiaroSama/Telegram-mcp` commit
`bd4b454c5392ef084ab265b92ca89ebc04d596df`. No previous Rust implementation was
found among the accessible branches or PR history at intake. Work remains in a
separate PR; this migration does not authorize merging it into `main`.

## Authorized scope

- Rewrite the application, local helpers and automated behavior tests in Rust.
- Preserve and update `start-mcp.ps1`, `Manage-Accounts.ps1`, `account-manager/*.ps1`
  and their PowerShell tests as the explicit launcher exception.
- Replace Telethon with grammers and implement missing application adapters locally.
- Preserve verification types and CI quality; exact legacy test-count equality was
  explicitly waived in favor of meaningful tests appropriate to Rust.
- Build Linux, macOS and Windows releases on every push/PR, and expose exact-head
  statuses. Build success alone is not complete validation.
- Review useful upstream improvements and current dependency documentation.
- Defer secret-chat integration until the owner supplies the separate Rust rewrite of
  `Telethon-Secret-Chat`. Do not retain its Python runtime as an invisible dependency.

## Source revisions reviewed

The following official sources were read on **2026-11-15**. Dates describe the
referenced source commit or release, rather than claiming every project released on
the review date. Historical byte-oracle versions remain distinct from current sources.

| Source | Reviewed revision | Source date | Review and adaptation scope |
| --- | --- | --- | --- |
| This fork | `bd4b454c5392ef084ab265b92ca89ebc04d596df` | Migration base | Existing tools, account state, approvals and fidelity contracts |
| [grammers](https://codeberg.org/Lonami/grammers/commit/e61cf8585e1e9969ab81eda9fb4725a1500e2d86) | `e61cf8585e1e9969ab81eda9fb4725a1500e2d86`, 0.12.0 | 2026-11-08 | Native Telegram transport, layer 233 TL schema, update and authorization custody |
| [chigwell/telegram-mcp](https://github.com/chigwell/telegram-mcp/commit/eed9d699643116cb769d9faf6cb12c8a7e734041) | `eed9d699643116cb769d9faf6cb12c8a7e734041` | 2026-11-14 | Chat/topic access filters, resumable metadata events, cursor readers and useful tool additions |
| [Official MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk/commit/0e2396f73a88617141b31f645de6f1171ed5f993) | `0e2396f73a88617141b31f645de6f1171ed5f993`; rmcp 3.10 API | 2026-11-15 | Stdio, authenticated streamable HTTP, shared-session bridge and cancellation |
| [Telegram Desktop](https://github.com/telegramdesktop/tdesktop/releases/tag/v7.4.4) | v7.4.4, `bf4f714036f21c750d535dab199c811b3f9bfdd4` | 2026-11-13 | Current export changes: date cursors, migrated groups and JSON/HTML escaping |
| [Qt qtbase](https://github.com/qt/qtbase/tree/v6.12.0) | v6.12.0, `025bdad181de241e81bf853c8a2d7bf3d19261a9` | 2026-09-26 | Current JPEG allocation and ICC validation changes; Qt 5.15.19 remains the historical oracle |
| [mozjpeg](https://github.com/mozilla/mozjpeg/tree/v4.1.5) | v4.1.5, `6c9f0897afa1c2738d7222a0a9ab49e8b536a267`; master `08265790774cd0714832c9e675522acbe5581437` | 2023-10-12; 2025-06-23 | Latest stable tag and build changes; native Rust encoder does not claim mozjpeg bitstream parity |
| [Samsung/rlottie](https://github.com/Samsung/rlottie/commit/ea06d2f29ba01b8d06c00a838d107f5e484ae59b) | `ea06d2f29ba01b8d06c00a838d107f5e484ae59b` | 2026-09-30 | Allocation/stride guards and pinned external shared library for real animated-sticker rendering |

The grammers GitHub mirror is stale relative to Codeberg; the live Codeberg source was
read directly. Search excerpts that still named 0.10.0 were not used as version proof.
The pinned grammers schema is layer 233; the former Telethon source pin announced layer
232. Matching or newer schema coverage does not imply equal high-level behavior.

Upstream changes are adapted to this fork's validation, account isolation, approvals,
filesystem and exact-text contracts. A blind merge of upstream Python code would not
satisfy those contracts or the Rust requirement. Attribution remains in `NOTICE`.

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

## Verification baseline

The original locked environment was collected with Python 3.13 and all extras:
**6,117 collected cases**, **302 Python suites**, **four PowerShell suites**.
`resources/migration-baseline.json` retains the source revision, definitions and actual
parameterized collection. It is audit data. The user explicitly waived exact native
case-count equality, so no test-name multiplication or synthetic one-to-one mapping is
used to claim parity.

Native CI preserves distinct gates for locked dependency resolution, behavioral tests,
optional renderer execution, Windows launchers, reviewed per-OS case inventories and base/renderer agreement,
coverage, advisory audit, static analysis, tree/history privacy, Gitleaks, container
build/persistence and Compose validation. `.github/native-verification.json` names actual
executed tests covering the critical behavior categories. Their assertions, not their
names, establish the practical limits of coverage.

Release builds run before behavioral gates. Artifacts can exist for a commit whose
other checks failed; read the full exact-head status set. Cancelled, skipped, missing,
wrong-commit or stale-attempt evidence never produces a successful custom status.
Each operating-system release package includes `telegram-mcp`, `emoji-studio` and
`operator-plugin`, plus per-binary SHA-256 and commit/compiler/lock provenance. The
Docker image provides the shared server; it does not currently distribute the two
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
coverage** across base and renderer legs on all three operating systems. The reviewed
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
identical code. A source-stable local Linux run passed all 1,494 tests with actual rlottie rendering
required and measured **80.39% all-production / 82.78% reviewed-core** coverage. This
single-platform result is below the core floor; it is not a successful six-leg CI
coverage result. Raw LLVM JSON region totals can count repeated instantiations and are
not substituted for the unique physical-line metric. The 85% gate remains enforced.


## Deliberate limits and migration cautions

**Secret chats:** no connected encryption backend, secret-chat key migration, encrypted
message delivery or secret-history parity is claimed. Preserve existing stores. The
future adapter must review that backend's key/counter persistence, peer erasure,
recovery, cancellation and custody contracts before activation.

**Legacy authorization custody gate:** known owned secret-chat sidecars block use of
an imported/legacy authorization before network startup. The reviewed grammers update
flow can implicitly acknowledge qts through full differences even without ReceivedQueue;
leaving the encryption tools disabled alone would not protect those updates. Fresh,
separate native authorization works, and legacy session/key files remain unchanged.
This gate covers detected owned layouts, not unknown external stores. Do not delete
state to bypass it; connect the owner's reviewed Rust secret backend when available.

**Session formats:** native grammers state is distinct from Telethon SQLite/string
sessions. Configured supported StringSession and SQLite schema7/8 authorization is
validated and imported into native state through a read-only source copy. The original
bytes and opaque secret-chat sidecars remain unchanged. Active/WAL, incompatible,
changed-generation or conflicting authorization refuses import rather than resetting
it. Staged login/repair additionally binds the actual configuration key/digest and
expected Telegram identity before committing a replacement.

**Rendered evidence:** structured Telegram metadata and rendered images are separate.
Native window capture is Windows-specific; other platforms report that restriction.
Optional TGS decoding requires the configured rlottie shared library. CI's renderer
legs require actual rendering and must fail when the library is absent.

**Exports:** Rust exporters preserve bounded pagination, exact text/entity handling,
media-path containment and honest job outcomes. A readable native export is not proof
of byte-identical Telegram Desktop/Qt ICC/mozjpeg output. Keep any remaining formatting,
color-management or thumbnail differences explicit in the export tests and PR report.

**Service lifecycle:** account-store exclusivity, bearer authentication and explicit
unknown mutation outcomes apply to the native runtime. Historical Python supervisor
or Win32 process-tree guarantees must not be inferred without corresponding native
implementation and tests.

**Live Telegram proof:** offline tests use synthetic fixtures and mock transports.
Without credentials and explicit authorized live inputs, real sign-in, Premium-only
features, permissions, network routing and server responses remain untested.

## Official references

- [grammers source and README](https://codeberg.org/Lonami/grammers)
- [grammers 0.12.0 source revision](https://codeberg.org/Lonami/grammers/commit/e61cf8585e1e9969ab81eda9fb4725a1500e2d86)
- [Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Current upstream Telegram MCP](https://github.com/chigwell/telegram-mcp)
- [CodeQL Rust build-mode support](https://docs.github.com/en/code-security/reference/code-scanning/codeql/build-options-for-compiled-languages#building-rust)
- [CodeQL 2.27.2 Rust improvements](https://codeql.github.com/docs/codeql-overview/codeql-changelog/codeql-cli-2.27.2/#rust)

GitHub's current documentation supports Rust CodeQL analysis with language `rust` and
build mode `none`; rust-analyzer still evaluates build scripts/macros. The release-build
jobs therefore remain necessary independently of CodeQL.
