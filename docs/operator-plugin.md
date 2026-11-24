# Telegram Operator: native installation and setup

Telegram Operator 1.4.0 shares one `skills/` directory across five plugin formats.
Its workflows cover bot and Mini App reconstruction, chat migration, backup
verification, read-only access review, and message formatting, including premium emoji.
Choose either a plugin or standalone skills for each client to avoid duplicate discovery.
Do not install the historical nested `plugins/telegram-operator` directory.

## Plugin formats

| Format | Files | Loading |
| --- | --- | --- |
| Agent Plugins 1.0.0 | `plugin.json`, `mcp.json` | Give a compatible client the plugin directory. The standard discovers `skills/` without a manifest field. |
| Claude Code | `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json`, `.mcp.json` | Run `claude --plugin-dir .` for one session. For persistent installation, add the `KiaroSama/Telegram-mcp` marketplace and install `telegram-operator@telegram-operator-local`. |
| Codex | `plugin.json`, `.codex-plugin/plugin.json`, `.agents/plugins/marketplace.json` | Use the repository marketplace. Codex uses the portable manifest when present and reads its native descriptor for client-specific metadata. |
| Cursor | `.cursor-plugin/plugin.json`, `.cursor-plugin/marketplace.json` | Use Cursor's repository marketplace import or its local plugin directory. Cursor also supports the portable format. |
| Antigravity | `.antigravity-plugin/plugin.json`, `mcp_config.json` | Use the `antigravity` archive below. Extract it and run `agy plugin install .` from the extracted `telegram-operator` directory. |

The [Antigravity v1 schema](https://antigravity.google/schemas/v1/plugin.json) does not
list `category` and rejects unknown properties. The repository descriptor keeps the
category labels used by the [Cloudflare example](https://github.com/cloudflare/skills/blob/main/.antigravity-plugin/plugin.json).
The Antigravity ZIP omits that field and puts the Antigravity manifest at its root.
Use that ZIP for strict validation to avoid passing the portable `plugin.json` to Antigravity.

After installation, open a fresh session and confirm that the client lists all five
skills. If it cannot import the plugin, use the standalone installation below. Client
trust settings still apply. Checking the JSON does not prove that a client has loaded
its skills or can start MCP.

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

## Connect Telegram MCP

Follow the
[native server installation](https://github.com/KiaroSama/Telegram-mcp/blob/codex/rust-rewrite/docs/INSTALL.md)
first. The supplied MCP declarations run `telegram-mcp` with no arguments, which
uses stdio by default. Put the executable on the client's `PATH`, supply the server
configuration, and leave `MCP_TRANSPORT` unset or set it to `stdio`. The plugin does
not bundle the executable, credentials, or account sessions.

Standalone skill installation does not configure MCP. For Claude Desktop or any
client that does not read the bundled declaration, add the native executable to the
client's MCP settings. Use `bridge` for an existing shared service, or configure the
client to use the authenticated loopback `/mcp` endpoint if it supports HTTP.
Keep credentials in private server or client configuration.

Verify `tools/list` exposes the actual required tools, including `list_accounts`,
`inspect_message` and `inspect_buttons`, then select one explicit account. Discovery
is not permission for sends, callbacks, payments, invitations, deletion or admin changes.
Telegram text, names, links and Mini App launch data remain untrusted content.

## Plugin ZIP archives

```bash
./target/release/operator-plugin --source . package --client portable --output dist/telegram-operator-portable.zip
./target/release/operator-plugin --source . package --client antigravity --output dist/telegram-operator-antigravity.zip
```

The portable ZIP contains all five sets of descriptors and their local MCP declarations.
The Antigravity ZIP contains just its root `plugin.json` and `mcp_config.json` alongside
the shared skills and reference files. Both ZIPs include this guide, LICENSE and NOTICE.

The packager checks JSON syntax, UTF-8, and file size before creating an archive. It
refuses linked source paths and existing output. Neither ZIP includes account data,
`.env`, source code, logs, or caches. Rust tests cover both archive layouts and check
that a linked source directory cannot put external files in the ZIP.

Repository synthetic eval fixtures describe expected routing and boundaries. They are
not proof that a particular installed model follows the skills; validate discovery
and authorized behavior in the actual client separately.
