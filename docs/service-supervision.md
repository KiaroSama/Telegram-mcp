# Operating the native shared service

Run the compiled binary from the intended installation and private configuration:

```bash
telegram-mcp serve --http 127.0.0.1:8765
```

The `/mcp` endpoint uses bearer authentication. Configure an explicit private
`TELEGRAM_HTTP_TOKEN` when required; a non-loopback bind needs a token of at least
32 characters and a matching PEM pair in `TELEGRAM_HTTP_TLS_CERT` and
`TELEGRAM_HTTP_TLS_KEY`. Non-loopback HTTP is refused; it never falls back from TLS
to plaintext. Clients must validate the service certificate and supply the bearer
over HTTPS. The native bridge attaches stdio clients to a loopback HTTP or validated HTTPS service rather
than creating an independent authenticated Telegram client per application.

`telegram-mcp bridge` is also the explicit native startup entry point. If discovery
does not identify a live owner, it serializes startup, verifies that the configured
loopback port is free and launches the same executable with the selected configuration.
It waits at most 30 seconds for owner identity and authenticated readiness. Failed
startup cleans up its own child. Existing services are checked using the process image
and creation identity around attachment, so a reused PID is insufficient. Other shared
API clients attach only. No caller may replace an unrelated occupied listener.

## Ownership and health

Private discovery carries a fresh local credential for each service generation,
separate from the configured external bearer. Native clients authenticate health
responses before sending MCP credentials; a stale listener cannot forge that
readiness proof. Restart the shared service when upgrading from an older health
protocol, and keep its discovery file private.

Authorization stores are account-scoped and exclusive. A second process must not
ignore or delete a live account lease. Closing an MCP transport session does not mean
the account store should be erased. Use `telegram-mcp accounts --json` and
`telegram-mcp health --json` to inspect locally recorded state without revealing keys.

HTTP reachability and Telegram connectivity are separate facts. A responsive listener
may still be offline from Telegram; a transport-only probe cannot establish delivery,
admin rights, Premium status or complete update recovery. After changing service setup,
verify MCP initialize, tools/list and an authorized read-only account call.

A mutation timeout has an unknown outcome: inspect the relevant Telegram state before
retrying. Do not replay writes simply because a service process restarted or an HTTP
connection closed.

## Windows and containers

`start-mcp.ps1` and `Manage-Accounts.ps1` remain supported PowerShell entry points and
invoke the native Rust binary. The account-manager helpers use native account/health
commands. Their tests run separately on Windows. They do not install Python or uv.

Docker Compose starts the native HTTPS service with an explicitly configured token,
publishes the host port only on loopback and persists account state in a named volume.
The container's `0.0.0.0` listener requires TLS even though the host port maps to
`127.0.0.1`. Configure the token and absolute host certificate/private-key paths
in private `.env`; Compose mounts both files read-only and refuses missing paths.
Use a certificate trusted by the clients and containing their selected server name
or IP address. Install a private issuer's CA in each client's trust store; do not
disable validation. No test certificate or private key is shipped. Preserve the
state volume across replacement.

## Logs and recovery limits

The Rust service writes private diagnostic logs while reserving stdout for MCP stdio.
Logs must describe fixed states and errors without raw tool arguments, Telegram content,
authorization keys, phone codes or environment dumps. Share only reviewed excerpts.
A log entry is evidence of what that component observed, not independent confirmation
that a remote action completed.

The native bridge implements the bounded startup/attachment behavior described above.
Historical Python supervision also had additional process-tree, restart-backoff and
generation-retirement rules; those broader guarantees do not follow from bridge startup.
The native account shutdown path stops the secret-chat manager before closing its
sender, and encrypted updates use durable ingress during operation. A forced process
termination is not proof that graceful shutdown completed. Preserve custody files and
recovery records; never stop an unknown listener or delete a state lock to obtain a
clean health report.
