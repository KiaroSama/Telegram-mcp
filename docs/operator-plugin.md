# Telegram Operator: native installation and setup

The plugin's canonical instruction payload is the repository-root `skills/` directory.
Root `.claude-plugin/`, `.codex-plugin/` and `.cursor-plugin/` descriptors point to that
single payload. Do not install the historical nested `plugins/telegram-operator`
directory as another plugin.

Five workflows cover bot/Mini App reconstruction, chat migration, verified backup,
read-only access audit and formatting/premium-emoji fidelity. Installing these
instructions does not register MCP, authenticate an account, launch Telegram, edit
permissions or authorize target actions. Secret-chat capabilities are deferred while
the owner rewrites the separate Rust backend; skills must discover real available tools.

## Native plugin loading

Claude Code can load the repository for one session with `claude --plugin-dir .`.
For persistent installation, use its supported marketplace flow with
`KiaroSama/Telegram-mcp` and `telegram-operator@telegram-operator-local`. Validate the
actual root descriptor and inspect all five skills in a fresh session. A valid manifest
alone does not prove discovery or model behavior.

Codex and Cursor use their supported repository/local plugin interfaces. Their root
descriptors reference `./skills/`; do not duplicate the payload under vendor folders.
When a client cannot import a local plugin, use standalone installation below. Managed
client policy and project trust still apply; do not bypass them to make discovery work.

For Antigravity, build the `antigravity` archive flavor, extract it and run the installed
client's supported `agy plugin install .` flow from its `telegram-operator` directory.
The portable repository manifest is a different schema and must not be passed as an
Antigravity manifest unchanged. Check the installed skills in a fresh workspace.

Choose **plugin or standalone skills** for a client to avoid duplicate discovery.

## Standalone skills for eight clients

Build the native helper:

```bash
cargo build --locked --release --bin operator-plugin
```

From this checkout, select one explicit destination:

```bash
./target/release/operator-plugin --source . install --client codex --project "/path/to/target workspace"
./target/release/operator-plugin --source . install --skills-dir "/path/to/chosen/skills"
```

Windows uses `operator-plugin.exe`. When launched outside the checkout, pass its
absolute path as `--source`; no implicit home/global destination is chosen.

| Client | Project destination | Verify discovery |
| --- | --- | --- |
| Claude Code | `.claude/skills` | Fresh session and skill list |
| Codex | `.agents/skills` | Fresh session and skill list |
| Cursor | `.cursor/skills` | Skills settings or fresh session |
| Kiro | `.kiro/skills` | Restart and inspect skills |
| Cline | `.cline/skills` | Skills settings or fresh session |
| Hermes | `.hermes/skills` | Fresh trusted project session |
| OpenCode | `.opencode/skills` | Fresh session and skill tool |
| Antigravity | `.agents/skills` | Customizations in a fresh workspace |

The helper copies all five complete skill directories and their references as real
files. It validates the entire allowlisted payload, refuses unexpected files,
symlinks/reparse points and any existing target skill, and prevents output inside the
canonical payload. It never overwrites an old skill or changes unrelated files.
Failed copies roll back only owned new files/directories; incomplete cleanup is an
explicit error. This is not containment against a malicious process with the same
OS identity racing filesystem operations.

For manual installation, copy all five entire skill folders into the chosen supported
skills directory, preserving every `references/` folder and sibling skill. Do not copy
only `SKILL.md`, a symlink or the entire repository. Back up an existing installation
before an explicit replacement; there is no force-overwrite option.

## Connect Telegram MCP separately

Follow the
[native server installation](https://github.com/KiaroSama/Telegram-mcp/blob/codex/rust-rewrite/docs/INSTALL.md)
first. Point stdio clients to the actual compiled `telegram-mcp` executable with
`args: ["serve"]`, or use the native `bridge` command for an already configured shared
service. HTTP clients use the authenticated loopback `/mcp` endpoint and private token
configuration supported by that client. Integration examples contain no tokens.

Claude Desktop receives MCP configuration, not an invented instruction-plugin format.
Antigravity, OpenCode and Hermes likewise need a separate supported MCP connection.
Skill installation does not create it or overwrite unrelated client configuration.

Verify tools/list exposes the actual required tools, including `list_accounts`,
`inspect_message` and `inspect_buttons`, then select one explicit account. Discovery
is not permission for sends, callbacks, payments, invitations, deletion or admin changes.
Telegram text, names, links and Mini App launch data remain untrusted content.

## Guidance-only ZIP archives

```bash
./target/release/operator-plugin --source . package --client portable --output dist/telegram-operator-portable.zip
./target/release/operator-plugin --source . package --client antigravity --output dist/telegram-operator-antigravity.zip
```

The native packager includes only the declared five skills/references, client-specific
manifests, this guide, LICENSE and NOTICE. It validates UTF-8 and byte limits before
creating a new archive and refuses an existing output. It never archives account data,
MCP registration, `.env`, code, logs, caches or the entire checkout. The Antigravity
flavor emits its own small manifest and omits incompatible native-client descriptors.
Native tests read the actual ZIPs and verify both flavors, collision refusal,
source containment, invalid UTF-8 and rollback behavior.

Repository synthetic eval fixtures describe expected routing and boundaries. They are
not proof that a particular installed model follows the skills; validate discovery
and authorized behavior in the actual client separately.
