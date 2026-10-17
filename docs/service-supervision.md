# Automatic shared-service recovery

Run the installed checkout's transport-only supervisor:

```powershell
& .\.venv\Scripts\telegram-mcp.exe supervise
```

Linux uses `.venv/bin/telegram-mcp supervise`. Configuration comes from this checkout's
`.env`/transport environment: loopback only, explicit `MCP_PORT`, default8765. There are
no credentials in the command. It never installs or synchronizes the environment.

Cold `bridge` startup elects this supervisor automatically. A healthy existing server
is verified and reused without launching another authenticated client. Closing a bridge
closes only its transport session, not the elected supervisor or shared service.

## Recovery contract

- One lifetime OS lease per normalized checkout and port covers startup, health checks,
  retirement and restart delay. A second supervisor exits75 without starting a generation.
- Windows starts the existing launcher hidden/suspended, assigns a fresh noninherited
  Job Object before resuming, and retains that job across the entire generation.
  Ordinary descendants cannot break out. Launcher exit alone is not tree exit.
- Startup grace is300seconds. Afterwards a bounded OPTIONS probe runs about once per
  minute; three consecutive misses trigger retirement. Each supervisor HTTP probe has a10second
  timeout. Windows monitoring reads the native TCP owner table and verifies the retained
  Job Object membership before and after the probe; it does not spawn PowerShell/CIM.
  Cold/warm bridge attachment retains its separate strict checkout identity checks.
  A successful probe resets misses. This proves responsive HTTP, not Telegram connectivity.
- Cleanup requests termination of the complete owned generation and waits up to30seconds
  per attempt for positive exit evidence. Unconfirmed cleanup retains its boundary and
  election, logs the blocked state, and retries confirmation about once per minute.
  **No replacement starts until complete retirement is confirmed.**
- Rapid exits back off10,20,40,80,160,300seconds;300seconds is the cap. Stable uptime
  resets the delay to5seconds. An unknown or separately owned listener is left untouched;
  supervision waits for it to disappear rather than adopting or killing it.

The account session/store leases remain unchanged and are never deleted. Outstanding
Telegram mutations are not replayed across a restart. Forced termination is not proof
that pending database or secret-state writes were flushed; inspect failure logs and
preserve stores rather than restoring stale cryptographic counters after traffic.

## Permanent Windows startup

Use a per-user Windows scheduled task to invoke the absolute installed command with
`supervise`, hidden, from the checkout. Preserve an existing task's identity, network
context and logon/wake/retry triggers when replacing a legacy keepalive wrapper. Set
multiple instances to IgnoreNew and execution time limit to unlimited (`PT0S`). Task
settings do not replace the supervisor's OS election.

Stop and confirm the old owner and all its descendants are gone before replacing its
wrapper or syncing packages. Back up the existing wrapper/task definition locally first.
Do not run the old taskkill loop alongside the new supervisor. Manual launcher takeover
remains separate: the supervisor never kills that separately owned replacement.

## Repeated disconnections despite recovery

The locked runtime includes `cryptg`, Telethon's native AES-IGE backend. Without a
usable native backend, Telethon falls back to pure-Python decryption on the same
event loop as HTTP; large incoming batches can delay probes and connector requests.
Install the complete locked environment while the owned service is stopped, then
restart it. Do not delete session locks or open a second authenticated client.
A successful restart alone does not establish that this bottleneck is resolved.

## Logs and limits

Each supervisor run creates a private UTF-8 file under the state directory's `logs/`,
`supervise_YYYY-MM-DD_HH-mm-ss_UTC.log` (collision suffix if needed). Entries are UTC
`[timestamp] [LEVEL] [supervise] Message`; fixed statuses and numeric health/backoff
counts only, no argv, environment, exception bodies, credentials or protocol payloads.
Logging failure falls back to stderr; logs are not automatically pruned. Share only
reviewed, sanitized excerpts. Launcher diagnostics remain in their existing per-run logs.

Windows abrupt supervisor death closes its last job handle and terminates ordinary owned
descendants; external scheduling starts a fresh supervisor. This does not cover remote
or WMI-spawned children or kernel I/O stalls. Linux retains its existing process-group
cleanup with `/proc` exit evidence; descendants that escape the group and abrupt parent
death are not equivalent to Windows job containment. No universal hang-free claim is made.

After deployment verify real MCP initialize, tools/list and transport-session closure;
then native bridge discovery/EOF and exactly one shared listener. Synthetic CI fixtures
exercise recovery without opening Telegram accounts or sending messages.
