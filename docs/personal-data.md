# Personal data and private state

The Rust runtime separates install files, private account state and explicitly allowed
content files. Use `telegram-mcp state-path` to inspect the actual state location.
`TELEGRAM_STATE_DIR` overrides it directly; otherwise `XDG_STATE_HOME/telegram-mcp`,
`$HOME/.local/state/telegram-mcp` on all platforms, falling back to `USERPROFILE`
when `HOME` is unavailable, preserves the existing installation’s state location.

Authorization sessions, account configuration, aliases, pending approvals, private event
data and logs belong in the private state directory. They never belong in the public
checkout, fixtures, plugin archives or CI artifacts. A session is a credential to the
account. A message-content backup is not authorization-state export permission.

## File access

`TELEGRAM_FILE_ROOTS` contains the intentional local directories tools may read/write.
A JSON array avoids path-list separator ambiguity across operating systems. Allowed
roots must exist. Without a custom setting, the service creates a private `files`
directory under its working directory.

Native path validation resolves existing files and destination ancestors, rejects
traversal and symlink escapes, and protects private state, dotenv files and repository
metadata even under a broad allowed root. A tool that supports folder grants can use
an outside path only after the server grants access for that request or path. Protected
paths stay blocked even while the owner has `/bypass` enabled.

Aliases and event feeds are account-scoped. Updates must preserve unrelated fields,
write valid UTF-8 and fail on unreadable/corrupt state instead of resetting it silently.
Atomic replacement and owner-only storage protect normal operation; a malicious process
already running as the same OS user remains outside that guarantee.

## Backups, deletion and containers

Preserve existing Telethon and secret-chat stores when installing the native version.
Their formats are not interchangeable with grammers state. Supported legacy secret-chat
stores are backed up and imported by the pinned native backend after authorization
identity is verified; see [secret-chat store migration](MIGRATION.md#secret-chat-store-migration).
Unsupported or conflicting custody is refused. Lost encryption keys cannot be recreated
by signing in again, and a message-content export does not replace a private key-store backup.

The Docker image stores all persistent state under `/data`, runs as a non-root user
and creates the state directory with private permissions. Keep the named volume when
replacing a container. Deleting the volume is deliberate account-state deletion and
must never be advertised as routine upgrade cleanup.

The Git and Docker ignore rules exclude credentials, sessions, local memory, logs and
personal content roots. CI also checks the tracked tree and reachable history; an
ignore rule alone does not protect a file previously forced into Git. Gitleaks and the
project's credential-shape scan provide evidence within their documented patterns,
not a universal guarantee that no private data exists.
