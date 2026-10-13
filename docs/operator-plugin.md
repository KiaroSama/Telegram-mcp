# Telegram Operator: installation and setup

Version **1.2.0**. Install from the **repository root**, not the legacy
`plugins/telegram-operator` directory. Root `.claude-plugin/`, `.codex-plugin/`
and `.cursor-plugin/` descriptors share one canonical `skills/` payload.

Five workflows: bot/Mini App reconstruction, chat migration, verified backup/restore,
read-only access audit and formatting/premium-emoji reconstruction. Instructions are
not a running clone or sync daemon. Installation never authenticates, registers MCP,
starts Telegram, edits permissions or authorizes target mutations.

## Get the checkout

```bash
git clone https://github.com/KiaroSama/Telegram-mcp.git
```

```bash
cd Telegram-mcp
```

Keep account configuration private. Choose **plugin OR standalone skills** per client,
not both, to avoid duplicate discovery.

## Native plugins

### Claude Code
Session-only loading from the repository root:

```bash
claude --plugin-dir .
```

For persistent installation, in Claude Code run
`/plugin marketplace add KiaroSama/Telegram-mcp`, then install
`telegram-operator@telegram-operator-local`. The suffix is the actual marketplace
JSON name, not the repository name. Check all five namespaced skills in a fresh session.
Descriptor validation without installation:

```bash
claude plugin validate . --strict --json
```

Validation alone is not loading or model behavior. Managed clients can prohibit plugins.

### Cursor
Import `https://github.com/KiaroSama/Telegram-mcp` through the supported plugin/local
marketplace interface in the installed client. Root `.cursor-plugin/plugin.json`
points at `./skills/`. Restart and inspect all five names. If local plugin import is
unavailable, use standalone skills below; no undocumented CLI command is required.

### Codex
Import the repository/local root through the installed Codex plugin interface.
Root `.codex-plugin/plugin.json` points at `./skills/`. Plugin APIs vary by version;
use standalone `.agents/skills` when local import is unavailable. Verify all five names
in a fresh session, rather than inferring discovery from a valid descriptor.

### Antigravity
Use the **Antigravity archive**, not the portable repository root `plugin.json`;
their schemas are incompatible. Extract the archive, enter its `telegram-operator`
directory, then explicitly install:

```bash
agy plugin install .
```

```bash
agy plugin list --json
```

Check skills in Customizations/a fresh session. Current Antigravity has native plugins;
package listing alone is not workflow execution. Project standalone installation is
an alternative that avoids IDE-versus-CLI global directory ambiguity.

## Standalone skills: all eight clients

The helper copies all five complete skill folders as real directories, including
sibling resources. It refuses any existing target skill, even identical content,
before copying. It never overwrites, links, authenticates, edits client configuration
or registers MCP. Unrelated destination files remain untouched. Failed copies roll
back only owned new files; incomplete rollback is reported. This is not containment
against a hostile same-owner filesystem race. Use the installed project Python;
the helper never installs dependencies.

| Client | Project skill directory | Discovery check |
|---|---|---|
| Claude Code | `.claude/skills` | Fresh session; available skills |
| Codex | `.agents/skills` | Fresh session; skill list |
| Cursor | `.cursor/skills` | Settings/skills or fresh agent session |
| Kiro | `.kiro/skills` | Restart and inspect skills |
| Cline | `.cline/skills` | Restart and inspect Skills settings |
| Hermes | `.hermes/skills` | Fresh project session; project trust may be required |
| OpenCode | `.opencode/skills` | Fresh session; available skill tool |
| Antigravity | `.agents/skills` | Customizations/skills in a fresh workspace |

From this checkout, replace `hermes` with ONE client from the table and specify the
actual target workspace:

```bash
python scripts/install_operator_skills.py --client hermes --project "/path/to/target workspace"
```

To deliberately choose a user/global skill directory:

```bash
python scripts/install_operator_skills.py --skills-dir "/path/to/chosen/skills"
```

There is no implicit home/global destination. OpenCode global is
`~/.config/opencode/skills`; Hermes is `~/.hermes/skills` (respect profile overrides).
Kiro/Cline/Claude use their `.kiro/skills`, `.cline/skills`, `.claude/skills` user roots.
Antigravity IDE uses `~/.gemini/config/skills`, CLI uses
`~/.gemini/antigravity-cli/skills`; `~/.agents/skills` is not its current global root.

For archive-only installation without Python, copy the five entire
`telegram-operator/skills/<name>` directories into exactly one chosen skill directory.
Preserve every `references/` folder and all sibling skills. Do not copy just SKILL.md,
a link or the whole repository. Back up an old install before an explicit replacement;
the helper deliberately has no force-overwrite option. Grant project trust only when
intended; never disable client trust checks to make skill discovery succeed.

## Connect the existing Telegram MCP separately

Follow [server installation](https://github.com/KiaroSama/Telegram-mcp/blob/main/docs/INSTALL.md) first. Use the actual installed checkout
executable with the single argument `bridge`; it attaches to the shared service,
not another authenticated Telegram client. The plugin bundles no MCP registration.

Claude Desktop gets MCP setup, not an invented native instruction-plugin format.
Its server entry uses the actual executable path and `args: ["bridge"]`; see
[Desktop setup](https://github.com/KiaroSama/Telegram-mcp/blob/main/docs/INSTALL.md#claude-desktop-automatic-shared-service-bridge).
Command/args MCP clients can use the same pair. HTTP clients use the actual loopback
`/mcp` endpoint from private project configuration; do not assume an illustrative port
is your configured one. Keep account/session/token values out of manifests and reports.

Verify the connected server exposes `list_accounts`, `inspect_message` and
`inspect_buttons`. Select one explicit account before executing workflows. Discovery
never authorizes destructive commands, payments, invitations or admin changes.
Antigravity/OpenCode/Hermes MCP connections use their supported client UI/config;
skill installation does not create them. Existing connection examples are in
`integrations/`; never replace unrelated entries wholesale.

## Build guidance-only archives

From the repository root:

```bash
python scripts/package_operator_plugin.py --client portable --output dist/telegram-operator-1.2.0-portable.zip
```

```bash
python scripts/package_operator_plugin.py --client antigravity --output dist/telegram-operator-1.2.0-antigravity.zip
```

Only explicit public descriptors, five skills/resources, this guide and LICENSE/NOTICE
ship. Runtime, private configuration, logs and accounts never enter the archive.
Existing output, linked/reparse payload paths and unexpected files inside owned payload
trees are refused. Other root repository files are deliberately excluded.
Both scripts reuse private per-run UTF-8 UTC logs in the existing state `logs/` directory,
`<component>_YYYY-MM-DD_HH-mm-ss_UTC.log`. Fixed stage/status text only, stderr fallback
on init failure, no overwrite, closed handlers and no automatic retention cleanup.

## Exactness and safe execution

Reconstruction separates stored text/entities/button declarations, viewed native
appearance and behavior/effects. Management/selfbot command cases preserve exact
invocation, actor/role and readback; menu discovery is not execution approval.
Write/destructive cases need explicitly approved disposable targets. User-account
automation is not permission to export sessions, clone devices or bypass safeguards.
Button colors use supported primary/success/danger or app default. Native glass
render depends on client/theme. Ordinary text uses supported entities, not arbitrary
font family/color or CSS; Mini App CSS is separate. Premium emoji require visible
frames/contextual render. Downloader queues, extraction, transcoding, authorization,
caching and limits remain hypotheses until approved evidence distinguishes them.
Unknown internals remain unknown, not recovered source code.

## Evidence and limitations

Root descriptors and standalone routes are documentation/source-backed. Offline tests
check paths, versions, complete copies, refusals/rollback and archive identity; they
are not proof of loading in every installed client. Earlier Codex discovery observed
five skills but failed strict profile isolation. Claude evaluation was authentication-
blocked; Cursor was not installed; Antigravity safe-offscreen loading was unverified.
Those remain owner-deferred, not silently passed. No live bot/group target was supplied.

Bridge ownership-overwrite repair passed four Windows cases and installed
initialize/356-tool discovery/EOF. A later Windows CIM query hit its existing
10-second bound; instruction packaging does not fix that separate timeout.
HTTP health was verified after installed Starlette 1.8.0 was observed.

Original guidance GPL-3.0-or-later. Revayat-Novel-Skill informed root structure only;
no acquired installer/runtime/private content copied. Sources checked 2026-10-13:
[Claude](https://code.claude.com/docs/en/plugins-reference),
[Cursor](https://cursor.com/docs/reference/plugins),
[Codex](https://developers.openai.com/plugins/build/plugins),
[Antigravity](https://antigravity.google/docs/skills),
[OpenCode](https://opencode.ai/docs/skills/),
[Hermes](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills/),
[Kiro](https://kiro.dev/docs/skills/), [Cline](https://docs.cline.bot/features/skills).
