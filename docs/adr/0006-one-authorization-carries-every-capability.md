# 6. One authorization carries every capability

> Historical decision. The tool count, source paths and storage limits below
> describe the 2026-09-21 migration. For the current Rust implementation, see
> the [secret-chat store migration](../MIGRATION.md#secret-chat-store-migration).

Date: 2026-09-21. Status: Accepted. ADR 0005 records the rich-message part of
this decision.

## Context

The ordinary client held one account authorization. TDLib, Telegram's C++
client, held another for secret chats and rich messages. Its roughly 12 MB
binary required 1,826 lines of Python wrapper code. Operators saw a second
device in Telegram and had to sign in separately. The manylinux wheels ruled
out a musl container and platforms without a wheel. Moving the state directory
without TDLib's database left the secret-chat keys and history behind.

An ordinary message read did not return the rich body, but the legacy client's
layer 229 schema exposed `messages.GetRichMessageRequest`, `Message.rich_message`
and 58 page-block types. The server could fetch that body on the existing
connection (ADR 0005). A separate secret-chat library also demonstrated MTProto
2.0 secret chats over the same authorization. Neither feature needed a second
Telegram device.

## Decision

Remove TDLib and use the account's existing authorization for every capability.
The twenty tools exposed at the time kept their names, arguments, defaults and
response shapes. Before removing TDLib, the team captured its responses as a
baseline and exchanged encrypted messages between two accounts using the new
backend. There was no separate TDLib installation after the switch. The native
Rust server now uses [Grammers Secret Chat](https://github.com/KiaroSama/Grammers-Secret-Chat)
on the shared grammers connection.

## Consequences

- **Identifiers.** Callers could supply `chat_id` or `secret_chat_id`; the package
  used only the latter. Two observed pairs had the same offset. The adapter kept
  both public fields so callers could still use an ID they had saved.
- **Incoming requests.** TDLib had completed the handshake itself. The server
  took over automatic acceptance because the tool catalogue then had no explicit
  `accept_secret_chat` call. Otherwise a request could stay pending until expiry.
- **History.** The library held incoming messages in memory. To preserve history
  across restarts, the server stored incoming and outgoing records beside the key
  store with owner-only access. `clear_secret_history` and `delete_secret_message`
  removed the corresponding records.
- **Replies.** Layer 45 introduced `decryptedMessage.reply_to_random_id`. The
  library already had the field in its schema, but its send paths did not pass it
  through. The upstream fix preserved replies on the wire.
- **Peer layer.** Measurements taken seconds apart in the same ready chat showed
  layer 46 on one side and layer 144 on the other. The peer can announce a higher
  layer after the chat opens, so the server reads the layer at send time and treats an unknown layer
  conservatively when deciding which features to send.
- **Media after restart.** TDLib had cached downloaded bytes. Its replacement at
  the time held each file key in the received message in memory. If the receiving
  process restarted, `save_secret_media` could no longer use that key. The server
  reported the limit rather than keeping decryption keys on disk indefinitely.
- **Errors and upgrades.** Refusals kept their response shapes, but the new
  backend supplied different error text. Callers could not rely on the old
  wording. `secret_backend.py` was the legacy client's sole import point, with
  a contract test covering the calls it used. For current Cargo pins and checks,
  see [Updating the native secret-chat backend](../updating-the-secret-chat-package.md).

## Alternatives considered

Keeping TDLib for a few difficult tools would still require a second login and
a second key store. Dropping secret chats would break existing tools despite a
working replacement. Persisting file decryption keys would restore media access
after restart but change what an operator keeps on disk. That choice was deferred
rather than made on the operator's behalf.
