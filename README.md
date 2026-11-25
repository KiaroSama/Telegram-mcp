<div align="center">

# Telegram MCP

**Use your own Telegram account from an MCP client.**

[![CI status on main](https://img.shields.io/github/actions/workflow/status/KiaroSama/Telegram-mcp/tests.yml?branch=main&style=flat-square&color=16a34a&label=CI)](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/tests.yml)
[![GPL-3.0-or-later license](https://img.shields.io/github/license/KiaroSama/Telegram-mcp?style=flat-square&color=2563eb)](#license)
[![Latest release tag](https://img.shields.io/github/v/tag/KiaroSama/Telegram-mcp?style=flat-square&color=ea580c&label=tag)](#releases-and-tags)
[![Core test coverage floor 85 percent](https://img.shields.io/badge/coverage-85%25%2B%20core-0d9488)](docs/api-coverage.md)
[![Built with Rust 1.100.0](https://img.shields.io/badge/rust-1.100.0-8b5cf6)](Cargo.toml)
[![Runs on Windows and Linux 64-bit](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%2064--bit-0891b2)](.github/workflows/release.yml)
[![Install from GitHub releases or Cargo](https://img.shields.io/badge/install-release%20%7C%20cargo-4f46e5)](docs/INSTALL.md)
[![Command and migration documentation](https://img.shields.io/badge/docs-commands%20%7C%20migration-0284c7)](docs/COMMANDS.md)
[![Repository size](https://img.shields.io/github/repo-size/KiaroSama/Telegram-mcp?style=flat-square&color=ca8a04)](#development)
[![grammers 0.12 on Telegram layer 234](https://img.shields.io/badge/grammers-0.12%20%7C%20layer%20234-15803d)](docs/MIGRATION.md)
[![390 native MCP tools](https://img.shields.io/badge/tools-390%20native-7c3aed)](docs/api-coverage.md)
![MCP Badge](https://badge.mcpx.dev)
[![M8ven Score](https://m8ven.ai/badge/mcp/kiarosama/telegram-mcp)](https://m8ven.ai/mcp/kiarosama/telegram-mcp)
[![M8ven Agent Verified](https://m8ven.ai/api/agent-verify/badge?score=75)](https://m8ven.ai/verified/verify?id=6bf8a0d29e8c8bae)
[![M8ven Verified](https://m8ven.ai/badge/mcp/kiarosama/telegram-mcp?variant=verified)](https://m8ven.ai/mcp/kiarosama/telegram-mcp)
[![Support donations](https://img.shields.io/badge/Support-donations-d04a9a)](#donate)

</div>

Telegram MCP reads and searches chats, sends messages, handles media, and manages groups and channels within your account's Telegram permissions. Version 3.2.0 uses [`grammers`](https://codeberg.org/Lonami/grammers) 0.12.0 (Telegram API layer 234), [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk) 3.11, and `kiaro-secret-chat` 0.3.0 for native MTProto 2.0 secret chats. The server does not need Python or a virtual environment.

## Releases and tags

The tag badge updates automatically from [GitHub tags](https://github.com/KiaroSama/Telegram-mcp/tags). [GitHub Releases](https://github.com/KiaroSama/Telegram-mcp/releases) hosts the signed archives, each with a `.sha256` checksum and a `.sig` signature. Each release also includes `SHA256SUMS`, `SHA256SUMS.sig`, `publisher-cert.pem`, and `publisher-key.pem`. GitHub records build provenance attestations for the archives and signed checksum files. See the [installation guide](docs/INSTALL.md#build-or-obtain-a-native-artifact) for verification instructions.

## Install

Choose an archive from [GitHub Releases](https://github.com/KiaroSama/Telegram-mcp/releases) for Windows or Linux on x64 or arm64. After extraction, the server is at `bin/telegram-mcp.exe` on Windows and `bin/telegram-mcp` on Linux. If the Linux file is not executable, run `chmod +x bin/telegram-mcp`.

To build from source, install [Rust 1.100.0 through rustup](https://rustup.rs/) and a native linker. Windows needs the MSVC build tools and Windows SDK. Linux needs its C compiler, linker and system development libraries. In a checkout of the release source, run:

```bash
cargo build --locked --release --bin telegram-mcp --bin emoji-studio --bin operator-plugin
```

Cargo puts the executables in `target/release/`. See [installation](docs/INSTALL.md) for optional media dependencies, artifact verification and state configuration.

### Authorize an account

Copy the source tree's [.env.example](.env.example) to a private `.env`. Set `TELEGRAM_API_ID` and `TELEGRAM_API_HASH` using your application credentials from [my.telegram.org/apps](https://my.telegram.org/apps). From a terminal with `telegram-mcp` on `PATH`, run:

```bash
telegram-mcp --config .env login --account default
telegram-mcp --config .env accounts --json
```

Enter login codes and two-factor passwords at the terminal prompt. Keep the `.env` and native session files private. If you already use a Python installation, read the [migration notes](docs/MIGRATION.md) before replacing it. The native importer validates supported legacy sessions without changing their source files and refuses conflicting or active authorization.

### Windows launchers

Run these with PowerShell 7 from the extracted Windows archive or a source checkout with a release build. If the binary is stored elsewhere, set `TELEGRAM_MCP_BINARY` to its full path.

```powershell
pwsh -NoProfile -File .\start-mcp.ps1
pwsh -NoProfile -File .\Manage-Accounts.ps1
```

`start-mcp.ps1` launches the compiled server. `Manage-Accounts.ps1` uses the native CLI to manage accounts.

## Connect an MCP client

Choose one of these stdio configurations. Replace `/path/to/Telegram-mcp` with your installation's absolute path. For Cargo builds, use `target/release/` instead of `bin/`. On Windows, use `telegram-mcp.exe` and double each backslash (`\\`) in JSON strings.

### Standard configuration (default system temp and cache)

```json
{
  "mcpServers": {
    "telegram-mcp": {
      "command": "/path/to/Telegram-mcp/bin/telegram-mcp",
      "args": ["--config", "/path/to/Telegram-mcp/.env", "serve"]
    }
  }
}
```

### Isolated temp and cache configuration

Create `tmp/` and `.cache/` inside the installation folder. The server stores private state in `.state/`. Programs that honor the temp and cache variables use the paths below. On Linux, set `TMPDIR` to the same `tmp/` path for programs that use it.

```json
{
  "mcpServers": {
    "telegram-mcp": {
      "command": "/path/to/Telegram-mcp/bin/telegram-mcp",
      "args": ["--config", "/path/to/Telegram-mcp/.env", "serve"],
      "env": {
        "TELEGRAM_STATE_DIR": "/path/to/Telegram-mcp/.state",
        "TEMP": "/path/to/Telegram-mcp/tmp",
        "TMP": "/path/to/Telegram-mcp/tmp",
        "XDG_CACHE_HOME": "/path/to/Telegram-mcp/.cache"
      }
    }
  }
}
```

### Shared HTTP service and stdio bridge

To share one service across clients, set `TELEGRAM_HTTP_TOKEN` in the private `.env`, then run:

```bash
telegram-mcp --config .env serve --http 127.0.0.1:8765
```

Point HTTP clients at `http://127.0.0.1:8765/mcp` and set the bearer token in the client. To attach a stdio client to the local service, use `bridge` instead of `serve` in its arguments. A non-loopback listener requires TLS with `TELEGRAM_HTTP_TLS_CERT` and `TELEGRAM_HTTP_TLS_KEY` and a private token of at least 32 characters. Use HTTPS and keep certificate verification enabled. See [shared HTTP setup](docs/INSTALL.md#authenticated-shared-http-service) for host allowlists and bridge behavior. After connecting, check `tools/list` and call `list_accounts` to verify the intended account.

## Logs and private state

Run `telegram-mcp state-path` to locate the private state directory. Direct server launches write `mcp.log` there, and the logger rotates it to `mcp.log.1` when it reaches its size limit. `TELEGRAM_MCP_LOG_FILE=off` sends diagnostics to stderr. The server reserves stdout for MCP traffic.

On Windows, the launchers write `logs/start-mcp_YYYY-MM-DD_HH-mm-ss_UTC.log` and `logs/Manage-Accounts_YYYY-MM-DD_HH-mm-ss_UTC.log` inside the private state directory. They use UTC timestamps and add a suffix if a name is taken. Each launcher keeps ten logs; `start-mcp.ps1` also keeps ten rotated parts. Runtime diagnostics exclude raw tool arguments, Telegram message text, authorization keys and login codes. Review excerpts before sharing them. See [service operation](docs/service-supervision.md#logs-and-recovery-limits) for logging and recovery limits.

## Tools and access

The server has 390 native tools: 356 ordinary tools, including standalone `transcribe_audio`, and 34 secret-chat tools. See the [command catalogue](docs/COMMANDS.md) and [API coverage](docs/api-coverage.md). Use [exposure filters](docs/exposed-tools-groups.md) to choose which tools a client can call. Telegram permissions, Premium requirements and server limits still apply.

`view_video` returns sampled frames with an optional local Whisper transcript. The stdio bridge reconnects after owner restart or session expiry. If the owner may have executed a mutation but its reply was lost, inspect the result before retrying.

Fetching content and marking it read are separate operations. The server returns integers that JavaScript cannot represent exactly as decimal strings. Message entity offsets use UTF-16. File tools stay within configured roots. Actions that require owner approval use a separate terminal prompt.

### Secret chats

[`kiaro-secret-chat` 0.3.0](https://github.com/KiaroSama/Telethon-Secret-Chat) provides native MTProto 2.0 secret chats over the same account connection as ordinary chats. The server commits incoming encrypted updates to durable storage before acknowledging them. All [34 secret-chat tools](docs/COMMANDS.md#secret-chat-tools) are available through MCP, subject to the same exposure filters and safeguards.

On first open of an account with a supported legacy secret-chat store, the server verifies the authorization identity, makes a read-only backup and migrates the store before starting normal update consumption. An imported session without a cached self identity may first need a request that suppresses updates to verify its owner. See [secret-chat store migration](docs/MIGRATION.md#secret-chat-store-migration).

## Telegram Operator plugin

`telegram-operator` 1.5.0 ships five workflow skills in five formats: Portable Agent Plugins 1.0.0, Claude Code, Codex, Cursor and Antigravity. The skills cover chat migration, backup and restore, access audits, bot and Mini App reconstruction, and reproducing message formatting and custom emoji. The plugin bundles no MCP server configuration. [Configure the MCP server separately](#connect-an-mcp-client) and authorize the account. For plugin setup, see [installation](docs/operator-plugin.md).

## Docker

```bash
docker build --tag telegram-mcp:local .
docker run --rm -i --env-file .env --volume telegram-mcp-state:/data telegram-mcp:local serve
```

Keep the state volume when replacing the container. For a shared HTTPS service, set the token and certificate paths before running `docker compose up --build -d`. Compose mounts the certificate and key read-only, binds port `8765` to loopback, and keeps private state in its `telegram-state` volume. See [Docker installation](docs/INSTALL.md#docker).

## Benchmarks

Both measurements come from five cold runs on the same Windows 11 x64 host. The Python baseline uses [main at `72a4aed`](https://github.com/KiaroSama/Telegram-mcp/commit/72a4aed8901fcce97b97797342aad3dcdfdc530a), where startup measures module import and tool registration (`3,440 ms` import; `4,830 ms` process wall) and disk footprint includes `.venv/`, package source and scripts. The Rust 3.0.0 release build measures cold stdio `initialize` plus `tools/list` for all 352 native tools, live process working-set RSS via `GetProcessMemoryInfo`, 50,000 `serde_json` UTF-8 string serializations, and the compiled executable size (`34.73 MB` for `telegram-mcp.exe`; `51.42 MB` for all three binaries combined).

| Measurement | Python baseline | Rust 3.0.0 | Delta |
| --- | ---: | ---: | ---: |
| Median cold startup and tool listing | 3,440.09 ms | 36.53 ms | -98.94% (94.2x faster) |
| Median working-set RSS | 124.32 MB | 15.98 MB | -87.15% (7.8x lower) |
| Median time to serialize 50,000 JSON strings | 126.97 ms | 6.77 ms | -94.67% (18.8x faster) |
| Runtime disk footprint | 206.01 MB | 34.73 MB | -83.14% (5.9x smaller) |

The repository-size badge shows the size GitHub reports for the repository. The disk footprint above measures the installed runtime.

## Development

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features -- --test-threads=1
```

[CI](.github/workflows/tests.yml) checks the four release targets, enforces an 85% application-core line coverage floor, and runs the Windows launcher tests. CI also includes [CodeQL](.github/workflows/codeql.yml) and [Gitleaks](.github/workflows/gitleaks.yml) workflows. Offline tests do not establish live Telegram behavior. See [contributing](CONTRIBUTING.md) for the CI evidence and the reproduction bundle.

## Author

[KiaroSama](https://github.com/KiaroSama) ([Kiaro.Sama.Dev@gmail.com](mailto:Kiaro.Sama.Dev@gmail.com))

## License

Copyright KiaroSama. Telegram MCP is licensed under [GPL-3.0-or-later](LICENSE); see [NOTICE](NOTICE) for third-party attributions.

## Donate

Send donations only on the network listed for the address.

| Currency | Network | Address |
| --- | --- | --- |
| Bitcoin (BTC) | Bitcoin | `bc1qmth5m03pu5hujw5xw5jmywam3jj3sqwqupesdt` |
| USDT, BNB, USDC, etc. | BEP20 | `0x0Bd0BA443a8B9cf15922bf7f0Bb0a4b495fD06Ef` |
| USDT, TRX, USDC, etc. | TRC20 | `TWBA3xFTqgZAeAYMxqo85xWnzvty3DcAhw` |
| Ethereum (ETH) | ERC20 | `0x0Bd0BA443a8B9cf15922bf7f0Bb0a4b495fD06Ef` |
| TON | TON | `UQCN8Umo_OfOWqImZetQsrNStPcmLkMAKajFyiCOhso23NDb` |
| Litecoin (LTC) | LTC | `ltc1qntqnnrunadurnw4cshv3qgspywrueyyeyngwuy` |
| Solana (SOL) | Solana | `7B2wkczUjmkDhETwQuknBL8sUsbuV7nErxc317TmQuwR` |
| Polygon (POL) | Polygon | `0x0Bd0BA443a8B9cf15922bf7f0Bb0a4b495fD06Ef` |
