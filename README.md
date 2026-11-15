# Telegram MCP — Rust

[![Rust builds and tests](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/tests.yml/badge.svg?branch=codex%2Frust-rewrite)](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/tests.yml?query=branch%3Acodex%2Frust-rewrite)
[![Rust lint](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/rust-lint-format.yml/badge.svg?branch=codex%2Frust-rewrite)](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/rust-lint-format.yml)
[![CodeQL](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/codeql.yml/badge.svg?branch=codex%2Frust-rewrite)](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/codeql.yml)
[![Gitleaks](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/gitleaks.yml/badge.svg?branch=codex%2Frust-rewrite)](https://github.com/KiaroSama/Telegram-mcp/actions/workflows/gitleaks.yml)
[![Rust](https://img.shields.io/badge/Rust-1.100%2B-orange)](https://www.rust-lang.org/)
[![grammers](https://img.shields.io/badge/grammers-0.12%20%7C%20layer%20233-2CA5E0)](https://codeberg.org/Lonami/grammers)
[![MCP SDK](https://img.shields.io/badge/MCP%20Rust%20SDK-3.10-6E56CF)](https://github.com/modelcontextprotocol/rust-sdk)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

Telegram account tools for MCP clients, rewritten around **grammers** and the official
**Rust MCP SDK**. The application, protocol adapters, storage and CI helpers use Rust.
The existing Windows launchers and their PowerShell tests are retained by explicit
request and launch the compiled executable.

**Migration branch:** completion requires equivalent native behavior and the preserved
CI verification types, not merely a successful build. The historical baseline is 6,117
collected cases across 302 suites, plus four PowerShell suites. Exact test-count parity
was explicitly waived; meaningful Rust tests and honest capability reporting remain required. Read [migration status and scope](docs/MIGRATION.md) before replacing an
existing installation. Live Telegram behavior requires account credentials and is
reported separately from offline tests.

**Secret chats are deferred.** The owner is rewriting `Telethon-Secret-Chat` separately.
This branch does not connect the Python package or claim that grammers itself provides
its encryption, key custody or secret-chat history. Existing private session and key
stores must be preserved until a reviewed migration is available. Known owned
secret-chat sidecars block imported/legacy authorization before network startup;
a fresh separate native authorization can serve ordinary chats. See the
[custody gate](docs/MIGRATION.md#deliberate-limits-and-migration-cautions).

## Install and build

Install [Rust](https://rustup.rs/) and a native linker for your operating system, then:

```bash
git clone --branch codex/rust-rewrite https://github.com/KiaroSama/Telegram-mcp.git
cd Telegram-mcp
cargo build --locked --release --bin telegram-mcp --bin emoji-studio --bin operator-plugin
```

The executable is `target/release/telegram-mcp` on Linux/macOS and
`target/release/telegram-mcp.exe` on Windows. CI builds and uploads native release
artifacts for Linux x86-64, macOS ARM64 and Windows x86-64 on every push and PR.
Artifacts identify the actual commit, compiler and `Cargo.lock` digest in `build.json`.
They include all three native CLIs and their SHA-256 values. Follow
[artifact permission and media-tool setup](docs/INSTALL.md) after extraction.
No package is published to crates.io by this repository.

Copy `.env.example` to a private `.env` and replace the invalid example API credentials
with your own application ID and hash from [my.telegram.org/apps](https://my.telegram.org/apps).
Authorize interactively:

```bash
./target/release/telegram-mcp login --account default
./target/release/telegram-mcp accounts --json
```

Login codes and two-factor passwords belong in the interactive terminal. Never place
them in chat, MCP arguments, committed examples or logs. A native grammers session is
an authorization credential. Configured supported legacy sessions are validated and
imported without modifying their source; conflicting or active authorization is refused. See [installation](docs/INSTALL.md) for account and state details.

## Connect an MCP client

For a client that starts the service over stdio, point it to the **absolute compiled
binary path** and pass `serve`:

```json
{
  "mcpServers": {
    "telegram-mcp": {
      "command": "/absolute/path/to/Telegram-mcp/target/release/telegram-mcp",
      "args": ["--config", "/absolute/path/to/private.env", "serve"]
    }
  }
}
```

Windows uses the equivalent absolute `.exe` path. The server reserves stdout for the
MCP protocol. Its diagnostics and private runtime state are separate.

For shared clients, start the authenticated HTTP service:

```bash
./target/release/telegram-mcp serve --http 127.0.0.1:8765
```

Use its `/mcp` endpoint with the configured bearer token. A non-loopback listener
requires TLS with a matching `TELEGRAM_HTTP_TLS_CERT` / `TELEGRAM_HTTP_TLS_KEY`
PEM pair and an explicitly configured private token of at least 32 characters.
Clients must trust the certificate and use HTTPS. Keep tokens in private client
configuration, never the repository's integration examples. Transport setup and the
shared-service bridge are described in [installation](docs/INSTALL.md) and
[service operation](docs/service-supervision.md).

On Windows, the preserved launchers invoke the native executable:

```powershell
pwsh -NoProfile -File .\start-mcp.ps1
pwsh -NoProfile -File .\Manage-Accounts.ps1
```

## Tools and boundaries

Tools cover ordinary chats, contacts, messages, media, administration, forums, polls,
stories, saved content, Telegram Business and account diagnostics. Read the
[command catalogue](docs/COMMANDS.md), [API coverage](docs/api-coverage.md) and
[exposure filters](docs/exposed-tools-groups.md). Telegram access rights, Premium
requirements and server-side limits still apply.

Read operations preserve the distinction between fetching content and sending a seen
signal. IDs that exceed JavaScript's exact integer range are represented as decimal
strings. Message text and entity offsets preserve UTF-16 semantics; content received
from Telegram remains untrusted input. Local files are restricted to configured roots.
Mutations that need the owner's approval use a separate genuine terminal approval,
not an MCP argument that a model can approve for itself.

The [operator plugin](docs/operator-plugin.md) provides five portable instruction
workflows. Installing skills does not register an MCP connection, authenticate an
account or authorize any target action.

## Docker

```bash
docker build --tag telegram-mcp:local .
docker run --rm -i --env-file .env --volume telegram-mcp-state:/data telegram-mcp:local serve
```

For a shared HTTPS service, set a fresh private `TELEGRAM_HTTP_TOKEN` and absolute
host paths `TELEGRAM_HTTP_TLS_CERT` / `TELEGRAM_HTTP_TLS_KEY` in `.env`, then use
`docker compose up --build -d`. Compose mounts the certificate/key read-only,
publishes only `127.0.0.1:8765` and persists
all state in the `telegram-state` volume. Keep that volume when replacing containers;
removing it is account-state deletion. The runtime is non-root and contains the native
server binary, native FFmpeg/FFprobe, certificates and system runtime libraries.

## Development and verification

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --locked --release --bin telegram-mcp
```

CI additionally checks the lockfile, all three operating systems, native release
artifacts, collected-case agreement, critical behavioral-category evidence, combined
application-core line coverage of at least 85% alongside full-production coverage,
the preserved PowerShell suites, dependency
advisories, Rust/Actions CodeQL, publication privacy, Gitleaks and container persistence.
Skipped, missing, cancelled and failed jobs do not produce a successful commit status.
Each status is attached to the exact validated head commit so it is visible beside that
commit on GitHub. [Contributing](CONTRIBUTING.md) explains the evidence formats and
reproduction bundle.

## Sources and license

The implementation follows [grammers](https://codeberg.org/Lonami/grammers), pinned to
its reviewed 0.12.0 revision and API layer 233, and the
[official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk).
The [chigwell/telegram-mcp upstream](https://github.com/chigwell/telegram-mcp) is reviewed
for useful improvements while retaining this fork's account, approval and fidelity
contracts. Exact revisions and gaps are recorded in [migration notes](docs/MIGRATION.md).

GPL-3.0-or-later. Existing Apache-2.0 and Telegram Desktop/Qt attributions remain in
[NOTICE](NOTICE); a language rewrite does not remove their license obligations.
