# 6. One authorization carries every capability

Date: 2026-09-21

## Status

Accepted. Supersedes the dependency this project carried for secret chats and rich
messages; ADR 0005 recorded the rich-message half of the same removal.

## Context

Until today this server ran on two clients. Telethon held the account's one
authorization, and TDLib — Telegram's own C++ library, 1,826 lines of Python wrapper
over a ~12 MB native binary — held a **second** one, with its own database, its own
sign-in and its own copy of the account's identity.

That second authorization bought two things Telethon genuinely could not do: secret
chats, because Telethon never implemented MTProto 2.0, and rich messages, whose body
does not arrive on an ordinary message read. Everything else about it was cost:

- **the account showed two devices.** Not a cosmetic wart: this project's first
  principle is one auth key, one connection, and a second device on the account is
  the exact shape that principle exists to prevent. It was tolerated because the
  alternative was losing the features.
- **a separate one-time login per account**, which every secret-chat tool had to
  detect and explain, and which a script existed solely to perform.
- **a platform floor.** The native wheels are manylinux-only, so the container could
  not be built on a musl base and a platform without a wheel could not install this
  project at all.
- **an unrecoverable failure mode.** The secret-chat keys lived in that database, and
  a database left behind by a moved state directory took its chats' history with it.

Two measurements this month removed the reason to keep paying. Telethon 1.45
announces TL layer 229 and carries `messages.GetRichMessageRequest`, the 58 page-block
types and `Message.rich_message` — so the rich body was never unreachable, only in a
different place (ADR 0005). And the owner's own `telethon-secret-chat` package
implements MTProto 2.0 **on an existing Telethon client**, so the encryption needs no
connection of its own.

## Decision

**TDLib is removed. Every capability this server offers runs on the one Telethon
authorization, and nothing native is installed.**

The twenty tools keep their names, arguments, defaults and answer shapes. A backend
swap that a caller can see is a backend swap that broke something.

The ordering was a safety property rather than a preference, and it held: the package
was hardened and proved to carry a real encrypted conversation between two of this
owner's accounts BEFORE anything here depended on it, and the previous backend's
answers were captured while it was still installed, because a baseline cannot be taken
afterwards.

## Consequences

**The account shows one device.** Principle I is now satisfied by construction rather
than by tolerating a documented exception, and the constitution's sentence permitting
an optional native component is deleted rather than reinterpreted (1.0.1).

**Four things had to be built, because the package is a library and this is a
server.** Each is a place the migration could have quietly lost something:

1. **Two ids for one chat.** The published surface has `chat_id` and
   `secret_chat_id`; the package has only the second. They turned out to be a fixed
   offset apart — measured on this account's own chats, both pairs agreeing — so both
   fields survive and an id written down before the migration still resolves.
2. **Incoming chats are accepted automatically.** The previous backend completed the
   handshake inside itself; the package hands the decision back, which is right for a
   library. With the tool surface frozen there is no `accept_secret_chat` to reach
   for, so an unanswered invitation would have sat at `pending` until it expired with
   every tool correctly refusing to send into it.
3. **History is this server's own, durable, and both directions.** The package keeps
   arrivals in memory — the right scope for a library, which should not write
   decrypted plaintext somewhere the operator never chose. This server published a
   two-directional history that survived restarts, so the decision to keep one is made
   here and the file lands beside the key store, owner-readable, and is emptied by
   `clear_secret_history` and `delete_secret_message`.
4. **Replies.** `decryptedMessage` has carried `reply_to_random_id` since layer 45 and
   the package implemented the schema but passed neither send path, so every reply
   would have arrived as an ordinary message. Fixed upstream, in the package, where it
   belongs.

**The layer is read at send time, and an unknown layer rounds down.** Measured during
the proof: two sides of the same ready chat reported layer 46 and 144 seconds apart,
because the layer is the PEER's capability and rises only when they announce it. A
report built from the layer at open time would tell a caller their custom emoji was
dropped from a chat that in fact carries it — a true-sounding statement that is false,
which is worse than an error.

**One capability is genuinely narrower, and it is stated rather than worked around.**
A secret chat's file key travels inside its message. The previous backend had already
downloaded the bytes into its database, so a save worked after a restart; here the
message that carries the key is in memory, so `save_secret_media` works while the
server that received the message is still running and reports the limit plainly
afterwards. Keeping it working would mean writing per-file decryption keys to disk
indefinitely — a materially worse posture for an end-to-end encrypted chat than the
thing being removed — so it is the owner's call to make, not one to make silently.

**Refusal wording changed.** A refusal used to quote TDLib's error text and now quotes
the package's typed exceptions. The shapes are identical; the sentences are not, and
nothing should match on them.

**Updating the package is one edit.** `secret_backend.py` is the only module that
imports it and a contract test pins what this server relies on, so a newer version is
a line in `pyproject.toml` plus `uv lock --upgrade-package telethon-secret-chat`.

## Alternatives considered

**Keep TDLib for the four or five tools that were hardest to port.** Rejected: the
second authorization is the whole cost, and paying all of it for a fraction of the
surface is the worst of both.

**Accept losing secret chats.** Rejected by the requirement the work was given, and
unnecessary once the package carried a real conversation.

**Persist file decryption keys so media survives a restart.** Deferred, not rejected —
see above. It restores a real capability at a real cost, and the person whose account
this is should decide which they want.
