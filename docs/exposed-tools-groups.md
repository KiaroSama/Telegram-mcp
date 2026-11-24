# Native tool exposure and chat restrictions

The native server validates its complete ordinary-chat catalogue before exposing it.
Unknown tool names or catalogue entries without a native route fail startup. Secret-chat
tools remain deferred and are not silently routed through ordinary cloud-chat APIs.

## Exposure modes

`TELEGRAM_EXPOSED_TOOLS` supports the current native syntax:

| Value | Result |
| --- | --- |
| `all` | All implemented nonsecret tools |
| `read-only` | Only tools annotated as read-only |
| `read-only+send_message,reply_to_message` | Read-only tools plus the explicitly named additions |
| `all-delete_messages,clear_history` | All tools except the named exclusions |
| `read-only+send_message-delete_messages` | Read-only base, explicit additions, then exclusions |

Use actual names from MCP `tools/list` or [COMMANDS.md](COMMANDS.md). Unknown names
are errors; they are not ignored. Filtering applies to discovery and invocation, so
calling a hidden name directly does not bypass it. Tool annotations describe effects;
they do not grant owner approval, account access or Telegram administrator rights.

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
