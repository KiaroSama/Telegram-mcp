# Native tool exposure and chat restrictions

The server validates all 386 native tools before exposing them: 352 ordinary tools
and 34 secret-chat tools. Startup fails if a configured tool name is unknown or a
catalogue entry has no native route.

## Exposure modes

`TELEGRAM_EXPOSED_TOOLS` supports the current native syntax:

| Value | Result |
| --- | --- |
| `all` | All 386 native tools, including secret-chat tools |
| `read-only` | Only tools annotated as read-only |
| `read-only+send_message,reply_to_message` | Read-only tools plus the explicitly named additions |
| `all-delete_message,clear_secret_history` | All tools except the named exclusions |
| `read-only+send_message-list_secret_chats` | Read-only base, explicit additions, then exclusions |

Use actual names from MCP `tools/list` or [COMMANDS.md](COMMANDS.md). Unknown names
are errors; they are not ignored. Filtering applies to discovery and invocation, so
calling a hidden name directly does not bypass it. Tool annotations describe effects;
they do not grant owner approval, account access or Telegram administrator rights.

## Secret-chat groups

The [secret-chat catalogue](COMMANDS.md#secret-chat-tools) lists all 34 native
MTProto 2.0 tools from `kiaro-secret-chat` 0.3.0:

| Group | Tools |
| --- | ---: |
| [Lifecycle and status](COMMANDS.md#lifecycle-and-status-6) | 6 |
| [Messaging and media](COMMANDS.md#messaging-and-media-5) | 5 |
| [Timed ephemeral messaging](COMMANDS.md#timed-ephemeral-messaging-2) | 2 |
| [Actions and search](COMMANDS.md#actions-and-search-6) | 6 |
| [Auto-save, deletion and export](COMMANDS.md#auto-save-deletion-and-export-6) | 6 |
| [Custody, pause, delivery and backend control](COMMANDS.md#custody-pause-delivery-and-backend-control-9) | 9 |

The same exposure filters apply to these tools. Hiding them does not turn off
secret-chat ingress or migration. On first open, the server makes a read-only
backup and migrates supported legacy stores losslessly before connecting. See
[secret-chat store migration](MIGRATION.md#secret-chat-store-migration).

## Allowed chats and forum topics

`TELEGRAM_ALLOWED_CHAT_IDS` is a comma-separated allowlist of explicit peers. A peer
may optionally name a positive forum-topic ID using `chat:topic`, for example
`-1001234567890:42,@team:7`. A chat entry without a topic permits that chat as a whole.
Multiple topic entries for the same chat are combined.

Peer type and marked IDs are resolved before a request is allowed. A topic-restricted
chat requires a permitted `topic_id` and an operation whose adapter can enforce that
restriction. Unsupported operations fail explicitly rather than treating topic access
as access to the entire chat. Source and destination peers are checked for relays.
Account-wide searches and exports that cannot safely enforce the allowlist are refused.
Returned chat/dialog collections are filtered, and filtered output is identified.

The allowlist is adapted from the current upstream topic-policy work while preserving
this fork's stricter refusal behavior. It is an application access boundary, not a
replacement for Telegram's permissions or an exhaustive account-data isolation claim.
