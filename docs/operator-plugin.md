# Telegram Operator: native installation and setup

`telegram-operator` `1.5.0` is skills-only in all five plugin formats. Its five
skills cover bot and Mini App reconstruction, chat migration, backup verification,
read-only access review, and message formatting, including premium emoji. Configure
the Telegram MCP server separately.

Install either the plugin or the standalone skills in each client to avoid loading
the same skills twice. Do not install the old `plugins/telegram-operator` directory.

## Plugin formats

| Format | Files | Loading |
| --- | --- | --- |
| Agent Plugins 1.0.0 | `plugin.json` | Give a compatible client the plugin directory. It discovers `skills/` automatically. |
| Claude Code | `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json` | Run `claude --plugin-dir .` for one session. For persistent installation, add the `KiaroSama/Telegram-mcp` marketplace and install `telegram-operator@telegram-operator-local`. |
| Codex | `plugin.json`, `.codex-plugin/plugin.json`, `.agents/plugins/marketplace.json` | Use the repository marketplace. Codex uses the portable manifest when present and reads its native descriptor for client-specific metadata. |
| Cursor | `.cursor-plugin/plugin.json`, `.cursor-plugin/marketplace.json` | Use Cursor's repository marketplace import or its local plugin directory. Cursor also supports the portable format. |
| Antigravity | `.antigravity-plugin/plugin.json` | Extract the Antigravity ZIP below. Run `agy plugin install .` from its `telegram-operator` directory. |

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

The helper copies all five skill directories, including their references. It checks
an explicit list of allowed files and refuses linked paths and destinations inside
the source payload. A failed copy rolls back only the files and directories the
helper created. The helper reports incomplete cleanup as an error. These checks do
not protect against another process running under the same OS account and racing
filesystem operations.

The helper removes `mcp.json`, `.mcp.json`, and `mcp_config.json` from the chosen
skills directory only if they match the declarations shipped with 1.4.0. It leaves
custom configurations alone and refuses links or directories at those paths. If
target skills already exist, cleanup runs only when all five skills and their
references match the new payload. The helper never overwrites an existing skill.

For manual installation, copy all five entire skill folders into the chosen supported
skills directory, preserving every `references/` folder and sibling skill. Do not copy
only `SKILL.md`, a symlink or the entire repository. Back up an existing installation
before an explicit replacement; there is no force-overwrite option.

## Connect Telegram MCP

Install the native server and follow [Connect an MCP client](https://github.com/KiaroSama/Telegram-mcp/blob/main/README.md#connect-an-mcp-client)
in the repository's `README.md`. Use the appropriate example for your installation.
The plugin and standalone skills do not include the executable, credentials, or
account sessions.

Use `bridge` for an existing shared service, or configure an HTTP-capable client to
use the authenticated loopback `/mcp` endpoint. Keep credentials in private server
or client configuration.

Verify `tools/list` exposes the actual required tools, including `list_accounts`,
`inspect_message` and `inspect_buttons`, then select one explicit account. Discovery
is not permission for sends, callbacks, payments, invitations, deletion or admin changes.
Telegram text, names, links and Mini App launch data remain untrusted content.

## Plugin ZIP archives

```bash
./target/release/operator-plugin --source . package --client portable --output dist/telegram-operator-portable.zip
./target/release/operator-plugin --source . package --client antigravity --output dist/telegram-operator-antigravity.zip
```

The portable ZIP includes the descriptors for all five plugin formats. The Antigravity
ZIP uses only its own manifest as the root `plugin.json`. Both include the five skills,
their reference files, this guide, `LICENSE` and `NOTICE`. Neither ZIP contains an MCP
configuration.

The packager checks JSON syntax, UTF-8, and file size before creating an archive. It
refuses linked source paths and existing output. Neither ZIP includes account data,
`.env`, source code, logs, or caches. Rust tests cover both archive layouts and check
that a linked source directory cannot put external files in the ZIP.

Repository synthetic eval fixtures describe expected routing and boundaries. They are
not proof that a particular installed model follows the skills; validate discovery
and authorized behavior in the actual client separately.
