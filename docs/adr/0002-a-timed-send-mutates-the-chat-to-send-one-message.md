# A timed send mutates the chat to send one message

An ordinary chat carries a self-destruct timer on each piece of media, so a caller
sends one disappearing photo and nothing about the chat changes. A secret chat has
no such thing — Telegram answers a per-message timer there with "Messages can
self-destruct only in private chats" — and its only timer belongs to the chat. So
`send_timed_secret_message` and `send_timed_secret_media` arm the chat's timer,
send, and restore the previous value, because the owner asked for the capability
their client gives them and this is the only mechanism underneath it.

The sequence is not atomic and cannot be made atomic. Anything the other person
sends inside the window is caught by the timer too, which is a consequence of the
timer being the chat's rather than the message's, and no amount of care here
prevents it. The tools say so in their own docstrings rather than leaving a caller
to discover it from a vanished message.

## Considered options

Refusing to build it was recommended and rejected by the owner, whose call it is
about their own account. Leaving the timer armed afterwards — simpler, one fewer
failure mode — was rejected because it turns a request to send one timed message
into a standing change to the conversation.

## Consequences

The restore runs in a `finally`, so an exception between arming and sending still
disarms. When the restore itself fails the reply says so in the loudest terms it
has and names the timer the chat has been left on: a silent restore failure is the
real harm here, because it leaves the conversation armed with nobody aware, and it
is strictly worse than the send having failed outright. Under Principle III that
outcome is `unconfirmed` rather than either success or failure — the message did
go, and the chat's state is not what the caller asked for.
