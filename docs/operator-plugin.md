# Telegram Operator plugin

The [plugin guide](../plugins/telegram-operator/README.md) covers installation,
separate MCP connection setup and the five original workflows. It uses the existing
Telegram-mcp runtime; no extra Telegram login, bridge or service is introduced.

## Deliverables
- Bot/Mini App behavioral reconstruction through independent implementation/parity tests.
- Channel/group migration, or explicitly approved checkpointed synchronization.
- Verified content backup and separately approved restoration rehearsal.
- Read-only settings/access review with visibility gaps.
- Text/entity/rich-block reconstruction and visual custom-emoji selection.

No target was supplied for actual cloning in this package-authoring task. Skills
are execution guidance, not a prebuilt clone of an unspecified bot. Hidden logic,
original IDs/dates/authorship, private APIs and unobserved paths are not recoverable
merely by browsing. Content-protection and authorization boundaries remain intact.

## Formats and evidence
| Surface | Package | Evidence |
|---|---|---|
| Claude Code | .claude-plugin descriptor | CLI2.1.295 strict manifest passed; native eval stopped auth_failed/Not logged in |
| Cursor | Agent Plugins1.0.0 portable root | Binary not located in bounded installed-client search; native loading blocked |
| Codex | Agent Plugins1.0.0 portable root | Bundled app-server discovered all five standalone skills; strict isolation failed (user roots also read); root plugin untested |
| Antigravity | Generated minimal root variant | Installed2.22.0 has no verified sandbox-preserving isolated offscreen path; not launched |
| Claude Desktop | Existing bridge guidance | Not claimed as a native instruction-plugin format |

Antigravity root accepts only name/description in the published full schema,
whereas portable Agent Plugins requires its schema identifier. Separate artifacts
avoid ambiguous format precedence. Five skill/resource payloads remain identical.
Current docs show Antigravity schema annotation examples that conflict with its
additionalProperties:false listing; minimal required metadata is used conservatively.

## Validation
Offline tests in tests/test_operator_plugin.py cover discoverability/resources,
identical payload across flavors, explicit package inventory and overwrite refusal.
Workflow evaluation fixtures in tests/fixtures/operator-plugin/evals.json are
synthetic and non-sensitive; they do not assert live target parity or native loading.
Native eval fixtures are under tests/fixtures/operator-plugin/native-evals. The
actual guarded eval was partial/auth_failed before completing its first case; no
behavioral pass or baseline delta is claimed. No credentials were copied to repair
the isolated child. Codex observation proves standalone discovery, not root-plugin
acceptance or strict isolation. These checks need authenticated isolated Claude
evals, a verified Cursor installation and safe isolated Codex/Antigravity surfaces.

Full integrated verification belongs to the existing CI matrix, not a duplicate
local suite. No client configuration, live Telegram state or permission mode changed.

Build instructions and source URLs are in the plugin guide. Package creator logs
fixed stage/status text through the existing private UTF-8 UTC per-run logger;
no secret/chat content. Generated artifacts contain only guidance and licensing,
not credentials, exports, operational state, hooks or MCP registrations.
