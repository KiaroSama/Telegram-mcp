# Telegram Operator
Five original workflows for the existing Telegram-mcp server:
- telegram-bot-reconstruction: observable bot/Mini App -> independent build -> parity tests.
- telegram-chat-migration: one-time migration or explicitly selected resumable sync.
- telegram-backup-restore: content export integrity and separate restore rehearsal.
- telegram-access-audit: read-only access/settings review.
- telegram-format-reconstruction: text/entities/rich content and rendered emoji.

Instructions are not a second server, bot framework or running sync daemon.
Installation does not register MCP, authenticate, start services, change permissions
or authorize target mutations. Executions establish account/target/action scope.
No hidden-source recovery or literal historical/account clone is promised.

## Claude Code
From the parent checkout, validate without installing:
```bash
claude plugin validate plugins/telegram-operator --strict --json
```
Session-only terminal loading:
```bash
claude --plugin-dir ./plugins/telegram-operator
```
Skills are namespaced by telegram-operator. Persistent plugin/catalog installation
is a separate operator action; no settings edited automatically.

## Cursor / Codex
Use portable archive/source root plugin.json (Agent Plugins1.0.0) through the local
plugin/marketplace import supported by the installed client. Verify all five names
in a fresh session. Managed environments can prohibit import. Do not use the
Antigravity variant. Alternative standalone discovery: real-copy the five complete
skill folders into Cursor .cursor/skills or Codex .agents/skills, preserving existing
folders and resources. Choose one route, not both plugin and standalone copies.

## Antigravity
Use antigravity archive, whose minimal root manifest replaces incompatible portable
metadata. Extract telegram-operator into workspace .agents/plugins; check loaded
skills in Customizations. CLI alternative from extracted directory:
```bash
agy plugin install .
```
No automatic global installation. Do not import portable source root directly.

## Claude Desktop / MCP
Desktop gets connection guidance, not an invented native instruction plugin.
Follow parent docs/INSTALL.md and integrations/claude-desktop example. Use the
installed telegram-mcp executable with bridge argument, resolved on the actual
machine. It attaches to existing shared service, not another authenticated client.
No machine-specific paths, credentials or connection config bundled in plugin.
Other clients register their existing MCP connection separately.

## Build from parent repository
```bash
python scripts/package_operator_plugin.py --client portable --output dist/telegram-operator-portable.zip
```
```bash
python scripts/package_operator_plugin.py --client antigravity --output dist/telegram-operator-antigravity.zip
```
Builder refuses existing output and unexpected/link files. Both variants include
identical skills/resources and LICENSE/NOTICE; no runtime/config/hooks/credentials.
It uses existing private per-run UTF-8 logging with UTC entries, sanitized statuses,
closed handlers and stderr fallback. Logs never contain chat content.

## Evidence and license
Offline metadata/archive tests are not native loading or live workflow proof.
Synthetic scenarios cannot prove real bot parity or restoration. Parent
 docs/operator-plugin.md records actual evidence/untested boundaries.
Original prose GPL-3.0-or-later, matching parent. Agensi-MCP informed structure
only; no acquired runtime or GPL-only guidance copied. Formats checked2026-10-12:
https://agentskills.io/specification,
https://agent-plugins.org/schemas/1.0.0/plugin.schema.json,
https://code.claude.com/docs/en/plugins-reference,
https://cursor.com/docs/plugins,
https://developers.openai.com/plugins/build/plugins,
https://antigravity.google/docs/plugins.md.
