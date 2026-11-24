# Native Rust installation

Telegram MCP v3.0.0 supports Windows and Linux on x64 and arm64. If you are replacing a Python installation, read the [migration notes](MIGRATION.md) before moving any account state. Keep existing private account and encryption stores intact.

## Build or obtain a native artifact

Choose the archive for your operating system and CPU on [GitHub Releases](https://github.com/KiaroSama/Telegram-mcp/releases).

| System | Architecture | Archive |
| --- | --- | --- |
| Windows | x64 | `telegram-mcp-v3.0.0-windows-x64.zip` |
| Windows | arm64 | `telegram-mcp-v3.0.0-windows-arm64.zip` |
| Linux | x64 | `telegram-mcp-v3.0.0-linux-x64.tar.gz` |
| Linux | arm64 | `telegram-mcp-v3.0.0-linux-arm64.tar.gz` |

Each release includes a `.sha256` checksum and a detached `.sig` for each archive, along with `SHA256SUMS`, `SHA256SUMS.sig`, `publisher-cert.pem` and `publisher-key.pem`. Check the certificate and public key against [the pinned publisher metadata](signing/publisher.json) from a trusted copy of the repository. Before extracting, use the pinned public key to verify the archive's `.sig`, or verify `SHA256SUMS.sig` and compare the archive's hash with its entry in `SHA256SUMS`. To calculate the hash, use `Get-FileHash -Algorithm SHA256` on Windows or `sha256sum` on Linux, followed by the archive path. On Windows, verify the Authenticode signatures on the `.exe` files in `bin/` against the pinned certificate. To check GitHub build provenance, run `gh attestation verify <archive> --repo KiaroSama/Telegram-mcp`.

All three executables (`telegram-mcp`, `emoji-studio` and `operator-plugin`) are in `bin/`. Windows executables have the `.exe` extension. The Windows archive keeps `start-mcp.ps1`, `Manage-Accounts.ps1` and `account-manager/` at its root. If a Linux extraction tool drops executable permissions, run `chmod +x bin/telegram-mcp bin/emoji-studio bin/operator-plugin` in the extracted directory.

To build from source, install [Rust through rustup](https://rustup.rs/). The repository selects Rust 1.100.0. Windows needs the MSVC build tools and Windows SDK. Native Windows arm64 builds also need arm64 `clang` and `clang-cl` on `PATH`. Linux needs its C compiler, linker and system development libraries.

```bash
git clone --branch v3.0.0 https://github.com/KiaroSama/Telegram-mcp.git
cd Telegram-mcp
cargo build --locked --release --bin telegram-mcp --bin emoji-studio --bin operator-plugin
```

The source build puts the executables in `target/release/`. Add the directory that contains `telegram-mcp` to `PATH` for the commands below, or invoke the executable by its full path. The server does not require Python, uv or a virtual environment. The Windows launchers require PowerShell 7.

### Optional media tools

For video and animation decoding or audio/video metadata, put native `ffmpeg` and `ffprobe` on `PATH`. Install your distribution's `ffmpeg` package on Linux. On Windows, choose a build for your CPU from [FFmpeg's download page](https://ffmpeg.org/download.html) or the [reviewed arm64 build](https://github.com/tordona/ffmpeg-win-arm64). CI checks the pinned Windows archives by SHA-256 and checks executable format, version and codec support, including VP9 and Opus. PDF, text and ordinary image inspection use the Rust pipeline. Optional native rendering needs the rlottie shared library; see [visual access](visual-structured-access.md). Local Whisper needs a configured native `whisper-cli` and model. The server does not download the model.

Hosted transcription requires HTTPS for remote services and all API keys. Credential-free HTTP is accepted only for literal loopback endpoints such as `http://127.0.0.1:8080/v1`; `localhost` aliases are not trusted as loopback destinations.
Local HTTP transcription connects directly and bypasses inherited system proxies.
HTTPS services retain normal proxy support and certificate validation.

Proxy URLs support SOCKS4, SOCKS5, HTTP CONNECT, verified HTTPS CONNECT and MTProxy. Use `https://proxy.example:443` and private username/password settings for authenticated remote HTTP proxies; HTTPS checks the hostname and operating system trust store. Authenticated plaintext HTTP/SOCKS is accepted only when the actual connected socket is loopback, for an owner-managed local tunnel. Anonymous remote HTTP/SOCKS routes remain available.

PDF parsing runs in a separate process with a 25 MiB input budget, bounded decompression and output, a 20-second CPU limit and a 40-second parent deadline. Linux applies a 512 MiB address-space limit. Windows applies a 512 MiB process memory limit. The watchdog terminates only its own parser process.

## Configure credentials and authorize an account

Copy the [example configuration](https://github.com/KiaroSama/Telegram-mcp/blob/v3.0.0/.env.example) to a private `.env` in the directory where you run setup. The example is in the source tree, separate from the binary archives. Set `TELEGRAM_API_ID` and `TELEGRAM_API_HASH` to your application credentials from [my.telegram.org/apps](https://my.telegram.org/apps). Process environment variables take precedence over the selected file.
`--config /absolute/path/to/private.env` (or `TELEGRAM_ENV_FILE`) selects an explicit
file for headless clients; without it ordinary commands use `.env` from their working
directory. Login and account-manager flows also provide `--env-file` for their selected
transaction source.

```bash
telegram-mcp --config .env login --account default
telegram-mcp --config .env accounts --json
telegram-mcp --config .env health --json
```

Use another explicit label with `login --account work` for a second account. Select the
intended label in tool arguments; never infer that one account's aliases, event feed or
permission state belongs to another. Login prompts are interactive. Keep the phone,
login code and two-factor password in the genuine local terminal; do not give them to
an MCP client or paste them into chat.

Native grammers and Telethon SQLite/string sessions have different storage formats.
For an explicitly configured legacy source, the native runtime validates and imports
supported Telethon SQLite schema7/8 or StringSession authorization into its own private
store. SQLite input is read through a read-only copy; the source bytes are not upgraded
or overwritten. An active owner, unexpected schema, WAL state, changed source generation
or conflicting native authorization is refused rather than reset. Existing secret-chat
keys/sidecars are preserved and are not interpreted by this import.

A configured/imported legacy authorization associated with a known owned secret-chat
sidecar is blocked **before any network connection**. grammers' full difference flow
can implicitly acknowledge qts even without an explicit ReceivedQueue call, so reusing
that authorization while its encryption backend is deferred could consume encrypted
updates. A fresh, separate native authorization remains supported. Keep the original
files; deleting a sidecar to bypass this gate would defeat its custody protection.
Detection covers the known owned-store layouts inspected by the runtime and does not
claim to discover unknown or external key stores.

The account manager uses staged login transactions bound to the selected configuration
key/digest and expected account identity. A failed or mismatched login preserves the
original authorization. `login --env-file PATH` selects its actual private configuration;
repair/replacement arguments are intended for the manager’s verified transaction flow,
not a reason to guess identity or replace an active session.
An explicit subcommand `--env-file` takes precedence over `--config` and
`TELEGRAM_ENV_FILE`. Process configuration values continue to override file values.

To retire an account, use
`telegram-mcp remove-account --account LABEL --env-file FILE --key KEY`. This archives its private records and removes only that exact file-owned
key. Original authorizations and encryption custody remain preserved. The command
refuses active accounts and process-owned or ambiguous configuration sources.

`telegram-mcp --config FILE bind-address` prints the same resolved address used by
the shared service. Set `TELEGRAM_MCP_LOG_FILE=off` for stderr-only runtime logging;
this explicit sentinel also works in PowerShell versions that remove empty values.

## Persistent private state

`telegram-mcp state-path` prints the actual private state directory without revealing
its contents. Resolution follows:

1. `TELEGRAM_STATE_DIR`, when explicitly configured;
2. `XDG_STATE_HOME/telegram-mcp`;
3. All operating systems: `$HOME/.local/state/telegram-mcp`, falling back to
   `%USERPROFILE%/.local/state/telegram-mcp` when `HOME` is unavailable.

The service stores authorization sessions, account-scoped data, pending approvals and
logs there. Keep the directory private to the owner. Back it up only through a reviewed
local process that protects authorization credentials; a content export is a separate
artifact. Do not delete locks or private stores to solve a second-instance error.

On Windows, keep private state and editable account configuration on NTFS, with
directory write access for the account running the service. Transactions check this
capability before publishing recovery intent or moving account files. Other file
systems cannot provide the directory durability guarantee used by this backend.

`TELEGRAM_FILE_ROOTS` defines the local directories tools may access. Prefer a JSON
array for an unambiguous cross-platform value, for example `["/path/to/telegram/files"]`.
Platform path-list syntax is also supported. The directories must already exist.
Without an override, the service creates a private `files` directory under its working
directory. Private state, credentials and repository metadata remain protected even
when a broad ancestor is allowed. See [personal data](personal-data.md).

## Stdio MCP clients

Use the absolute path to the executable in `bin/` for a release archive or in `target/release/` for a Cargo build. Pass `serve` in the client's arguments. For example:

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

On Windows, use a path such as `C:/path/to/Telegram-mcp/bin/telegram-mcp.exe`.
Replace the example path with the one on your machine. Use an explicit private configuration path or your client's supported secret environment
configuration. Do not assume a client starts in the repository's working directory or
discovers the repository's `.env`. Process values retain precedence over file values.

For a server shared by several MCP clients, use the authenticated HTTP service and
transport-only bridge described below. Avoid starting multiple authenticated services
against the same account store.

## Authenticated shared HTTP service

Start the service locally:

```bash
telegram-mcp --config .env serve --http 127.0.0.1:8765
```

The MCP endpoint is `/mcp`. Configure the same private bearer token on clients. A
private per-installation token protects the service; an explicit `TELEGRAM_HTTP_TOKEN`
can select it. A non-loopback listener requires an explicitly configured token of at
least 32 characters, together with `TELEGRAM_HTTP_TLS_CERT` and
`TELEGRAM_HTTP_TLS_KEY`. Both PEM files must form a matching certificate/key pair;
malformed, mismatched or incomplete configuration fails before the listener opens.
There is no plaintext fallback. The server must not be published as an
unauthenticated Telegram account-control endpoint.

For TLS, obtain a server certificate from a CA trusted by the clients and include
the DNS name or IP address clients will use in its subject alternative names.
Set the two TLS variables to the PEM certificate-chain and private-key files,
then configure clients with `https://host:port/mcp` and their private bearer header.
For a remote hostname, set `TELEGRAM_HTTP_ALLOWED_HOSTS` to the exact hostname or
`host:port` authorities clients use, separated by commas. The default MCP Host
allowlist accepts loopback hosts only.
For a private CA, install that CA in each client's trust store. Do not disable
certificate verification. Keep private keys outside the repository and restrict
their filesystem access to the service operator.

The native bridge accepts loopback HTTP and HTTPS. For HTTPS it validates the
hostname and operating-system trust roots, plus the certificate chain published
by its privately owned service discovery. The certificate must cover the
loopback address or exact `localhost` name used by discovery. Existing TLS
services are never downgraded, and certificate verification cannot be disabled.

Local discovery uses a fresh credential for each service generation, separate
from the configured external bearer. Health checks authenticate the service
without sending either raw credential. Restart an older shared service when
upgrading its native clients so both use the same authenticated health protocol.

The native `bridge` command attaches stdio clients to the local HTTP shared service. If no verified
service exists, that explicit command can start the same native executable using the
selected private configuration. Startup is serialized with an owner lock, bounded by
a timeout and checked against the published PID, executable, creation identity and
authenticated health response. An occupied port or unverified process is refused.
Other shared API clients attach to an existing service; they do not implicitly start
one. The bridge does not replay failed mutations.

HTTP clients such as Gemini or Antigravity must use their supported private header/token
configuration. Repository examples show endpoint shape only and do not embed tokens.
A successful TCP/HTTP response establishes transport reachability, not Telegram account
connectivity. Verify actual MCP initialize and tools/list plus the intended account.

## Windows launchers

From PowerShell 7, run these in the extracted Windows archive or in a source checkout with a release build:

```powershell
pwsh -NoProfile -File .\start-mcp.ps1
pwsh -NoProfile -File .\Manage-Accounts.ps1
```

`start-mcp.ps1` launches the compiled service. `Manage-Accounts.ps1` manages accounts through the native CLI and uses the `.env` beside the script. Keep the `account-manager/` directory with the launchers. Set `TELEGRAM_MCP_BINARY` to the full path to `telegram-mcp.exe` when using a binary stored elsewhere. The four launcher test suites run on Windows in CI.

## Docker

The Docker image builds the locked Rust application, then copies the compiled server
into a non-root Debian runtime with native FFmpeg/FFprobe. Persistent state resolves
under `/data/state`.

```bash
docker build --tag telegram-mcp:local .
docker run --rm -i --env-file .env --volume telegram-mcp-state:/data telegram-mcp:local serve
```

For the shared HTTPS service, replace `TELEGRAM_HTTP_TOKEN` in private `.env` with
a fresh random token of at least 32 characters. Set `TELEGRAM_HTTP_TLS_CERT` and
`TELEGRAM_HTTP_TLS_KEY` to absolute host paths for the matching PEM files, then run
`docker compose up --build -d`. Compose mounts both files read-only and refuses
nonexistent source paths. The container's non-root service user must have read access
to the certificate and key; grant only that user access through ownership or a host
ACL, and keep the private key outside the repository. Clients must trust the issuer
and use a server name or IP listed in the certificate.
Compose binds inside the container to `0.0.0.0:8765`, requires TLS and the explicit
token, and publishes only `127.0.0.1:8765` on the host. The `telegram-state` volume persists
account state across replacement. Do not use `docker compose down --volumes` unless
you intentionally want to delete that state.

## Verify the installed revision

```bash
telegram-mcp --version
telegram-mcp --config .env state-path
telegram-mcp --config .env accounts --json
telegram-mcp --config .env health --json
```

Then verify MCP initialize, tools/list and an authorized read-only call on the selected
account. CI uses synthetic data and mock Telegram transports; it does not sign in to
real accounts or prove live Premium/admin/server-side behavior. Review the exact head
commit's complete job results and [migration notes](MIGRATION.md) before deployment.
