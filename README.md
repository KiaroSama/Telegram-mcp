<div align="center">

# Telegram MCP

**Use your own Telegram account from an MCP client.**

[![CI workflows on main](https://img.shields.io/badge/CI-Windows%20%7C%20Linux%2064--bit-16a34a)](.github/workflows/tests.yml)
[![GPL-3.0-or-later license](https://img.shields.io/badge/license-GPL--3.0--or--later-2563eb)](LICENSE)
[![Release v3.0.0](https://img.shields.io/badge/release-v3.0.0-ea580c)](Cargo.toml)
[![Core test coverage floor 85 percent](https://img.shields.io/badge/coverage-85%25%2B%20core-0d9488)](docs/api-coverage.md)
[![Built with Rust 1.100.0](https://img.shields.io/badge/rust-1.100.0-8b5cf6)](Cargo.toml)
[![Runs on Windows and Linux 64-bit](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%2064--bit-0891b2)](.github/workflows/release.yml)
[![Install from GitHub releases or Cargo](https://img.shields.io/badge/install-release%20%7C%20cargo-4f46e5)](docs/INSTALL.md)
[![Command and migration documentation](https://img.shields.io/badge/docs-commands%20%7C%20migration-0284c7)](docs/COMMANDS.md)
[![Repository source size: 7.05 MB](https://img.shields.io/badge/repo%20size-7.05%20MB-ca8a04)](Cargo.toml)
[![grammers 0.12 on Telegram layer 233](https://img.shields.io/badge/grammers-0.12%20%7C%20layer%20233-15803d)](docs/MIGRATION.md)
[![Official Rust MCP SDK rmcp 3.11](https://img.shields.io/badge/mcp%20sdk-rmcp%203.11-b45309)](Cargo.toml)
[![Supports 5 agent plugin formats](https://img.shields.io/badge/plugins-5%20formats-334155)](docs/operator-plugin.md)
[![352 native non-secret MCP tools](https://img.shields.io/badge/tools-352%20native-7c3aed)](docs/api-coverage.md)
[![CI includes CodeQL and Gitleaks workflows](https://img.shields.io/badge/security-CodeQL%20%7C%20Gitleaks-c2410c)](.github/workflows)
[![Support donations](https://img.shields.io/badge/Support-donations-d04a9a)](#donate)

</div>

Telegram MCP can read and search chats, send messages, handle media, and manage groups and channels within the account's Telegram permissions. Version 3.0.0 uses `grammers` 0.12.0 from [Codeberg](https://codeberg.org/Lonami/grammers) at Telegram API layer 233 and the [official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk) (`rmcp` 3.11). The server does not need Python or a virtual environment.

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

For stdio, configure your client to start the executable with `serve`. Use the absolute path to the executable in `bin/` for a release archive or in `target/release/` for a Cargo build. On Windows use the `.exe` and escape backslashes in JSON.

```json
{
  "mcpServers": {
    "telegram-mcp": {
      "command": "/absolute/path/to/Telegram-mcp/bin/telegram-mcp",
      "args": ["--config", "/absolute/path/to/private.env", "serve"]
    }
  }
}
```

For a shared service, set `TELEGRAM_HTTP_TOKEN` in the private config, then run:

```bash
telegram-mcp --config .env serve --http 127.0.0.1:8765
```

Point the client at `http://127.0.0.1:8765/mcp` and configure the bearer token in the client. A non-loopback listener requires TLS with `TELEGRAM_HTTP_TLS_CERT` and `TELEGRAM_HTTP_TLS_KEY` and a private token of at least 32 characters. Use HTTPS and keep certificate verification enabled. See [shared HTTP setup](docs/INSTALL.md#authenticated-shared-http-service) for host allowlists and the native bridge. After connecting, check the MCP tool list and call `list_accounts` to verify the intended account.

## Logs and private state

Run `telegram-mcp state-path` to locate the private state directory. Direct server launches write `mcp.log` there, and the logger rotates it to `mcp.log.1` when it reaches its size limit. `TELEGRAM_MCP_LOG_FILE=off` sends diagnostics to stderr. The server reserves stdout for MCP traffic.

On Windows, the launchers write `logs/start-mcp_YYYY-MM-DD_HH-mm-ss_UTC.log` and `logs/Manage-Accounts_YYYY-MM-DD_HH-mm-ss_UTC.log` inside the private state directory. They use UTC timestamps and add a suffix if a name is taken. Each launcher keeps ten logs; `start-mcp.ps1` also keeps ten rotated parts. Runtime diagnostics exclude raw tool arguments, Telegram message text, authorization keys and login codes. Review excerpts before sharing them. See [service operation](docs/service-supervision.md#logs-and-recovery-limits) for logging and recovery limits.

## Tools and access

The [command catalogue](docs/COMMANDS.md) and [API coverage](docs/api-coverage.md) describe the 352 native tools for ordinary chats, including standalone `transcribe_audio`. Use [exposure filters](docs/exposed-tools-groups.md) to choose which tools a client can call. Telegram permissions, Premium requirements and server limits still apply.

Fetching content and marking it read are separate operations. Integers that JavaScript cannot represent exactly are returned as decimal strings; message entity offsets use UTF-16. File tools are limited to configured roots. Actions that require owner approval use a separate terminal prompt.

### Secret chats

Secret-chat support is deferred while `Telethon-Secret-Chat` is rewritten separately in Rust. The native server does not use the old Python backend for secret-chat encryption, key custody or history. Preserve existing private session and key stores. If the importer finds known secret-chat sidecars, it blocks the affected legacy authorization before connecting. Use a fresh native authorization for ordinary chats. See [migration cautions](docs/MIGRATION.md#deliberate-limits-and-migration-cautions) before moving account state.

## Telegram Operator plugin

`telegram-operator` ships one skill pack in five formats: Portable Agent Plugins 1.0.0, Claude Code, Codex, Cursor and Antigravity. The skills cover chat migration, backup and restore, access audits, bot and Mini App reconstruction, and reproducing message formatting and custom emoji. Configure the MCP connection and authorize the account separately. See [plugin installation](docs/operator-plugin.md).

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

The repository-size badge reports source tree size in decimal MB. The disk footprint above measures the installed runtime.

## Development

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features -- --test-threads=1
```

[CI](.github/workflows/tests.yml) checks the four release targets, enforces an 85% application-core line coverage floor, and runs the Windows launcher tests. Offline tests do not establish live Telegram behavior. See [contributing](CONTRIBUTING.md) for the CI evidence and the reproduction bundle.

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

## License and sources

Telegram MCP is licensed under [GPL-3.0-or-later](LICENSE). [NOTICE](NOTICE) records the Apache-2.0 and Telegram Desktop/Qt attributions. See the [migration notes](docs/MIGRATION.md) for pinned dependencies and changes drawn from [chigwell/telegram-mcp](https://github.com/chigwell/telegram-mcp).
