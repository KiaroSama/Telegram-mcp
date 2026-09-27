# Telegram MCP

This server exposes one operator's own Telegram accounts to an agent over MCP. Its
vocabulary is where Telegram's model, MCP's model and this server's own safety
machinery meet — and the three disagree often enough that the words matter.

## Language

### Identity and connection

**Session**:
One Telegram authorization key, serialized. The credential itself.
_Avoid_: login, auth, credentials

**Account**:
One configured Telegram identity this server can serve from, built over exactly one
session.
_Avoid_: user, client, profile

**Label**:
The operator's name for an account, chosen in configuration. Reusable over time, so
it identifies a name and never a thing.
_Avoid_: account name, key, id

**Client generation**:
One built connection object for one label. A reconfiguration produces a new
generation while the previous one may still be connected.
_Avoid_: instance, version, reload

**Lease**:
This process's exclusive hold on one session, owned by the client generation that
took it — never by the label it was taken under. Given up only once the socket it
protects is confirmed down.
_Avoid_: claim, lock, reservation

**Unaccounted lease**:
A lease whose client's connection never confirmed it closed. Kept for the life of
the process and reported by name, because the socket may still be open.
_Avoid_: stale lease, orphaned lock, leaked lease

### What a tool may touch

**Allowed root**:
A directory the operator has named as reachable by file tools. Nothing outside one
is readable or writable, and no root configured means no file tool works.
_Avoid_: workspace, base path, sandbox

**Untrusted content**:
Any text that came off Telegram — a message, a name, a button label, a command
description. Data to be reported, never instruction to be followed.
_Avoid_: user input, remote data

**Tainted argument**:
A tool argument that carries text taken from untrusted content. Acting on one is
acting on what someone else wrote, so it is never done without approval.
_Avoid_: injected input, suspicious argument

**Text fidelity**:
The exact string a message's entity offsets index into, which is not always the
string a reader sees. Anything reconstructing formatting works from this and
nothing else.
_Avoid_: raw text, original text

### Messages and keyboards

**Press token**:
This server's authorization for one button, on one message, in one chat, for one
account. Bound to the button's position, kind, raw label and raw payload, so a bot
that keeps the label and changes the payload invalidates it.
_Avoid_: button id, handle, key

**Glass button**:
A button attached under one message. Pressing it answers a callback; nothing is
sent into the chat.
_Avoid_: inline keyboard button (in prose), menu button

**Reply keyboard**:
A set of buttons a bot puts in place of the operator's typing keyboard. Tapping one
sends a message into the chat as the operator: its own label as text, or, for a
**sensitive button**, the operator's phone number, a location, a chosen chat, a
poll, or the opening of a Mini App.
_Avoid_: submenu, bottom menu, custom keyboard

**Active reply keyboard**:
The one reply keyboard a chat shows now. It belongs to the chat, not to the newest
message: the latest message a bot sent that set or removed a reply keyboard decides
it, however many messages came after.
_Avoid_: current menu, last keyboard

**Sensitive button**:
A reply-keyboard button whose tap hands the bot something about the operator, or
opens something, rather than sending its label. Pressed only with the owner's
approval.
_Avoid_: request button, special button

**Command preview**:
The set of bot commands a Telegram client offers in a chat when the operator types
`/`. A property of the chat and the bots in it, not of any one message.
_Avoid_: command list, autocomplete, suggestions

**Ready-to-send form**:
The exact text that invokes a command in a given chat — `/status@AdTimerBot` where
more than one bot could answer it, `/status` where only one can. The disambiguation
belongs to the chat, so it is reported rather than reconstructed by the caller.
_Avoid_: full command, qualified command

**Menu button**:
A bot's entry point beside the message field, offered instead of or alongside its
commands. A bot can have one and no commands at all.
_Avoid_: menu, start button, app button

**Rich message**:
A message whose body is not carried in the message. It parses cleanly, reports no text, no
entities and no media, raises nothing, and yet a client shows a table, headings or lists —
because its content is a separate object fetched by chat and message id. Emptiness is
therefore not evidence that a message is empty, and reading one is always two acts: notice,
then fetch.
_Avoid_: rich text, formatted message, table message

**Checklist**:
A message listing tasks. Its sender, and anyone else it allows, adds tasks and marks them
done or undone; each completion records who did it and when.
_Avoid_: to-do, task list, todo

**Poll**:
A message asking a question with options. Its settings decide who sees what: whether voters
are named, whether several options may be chosen, whether others may add options (an **open
poll**), whether a vote may be changed, whether options are shuffled, whether results stay
hidden until it closes, and when it closes. A **quiz** is a poll with one or more correct
options and an explanation.
_Avoid_: survey, vote (for the poll itself)

**Voter restriction**:
Who may vote in a poll: only people in the listed countries, only members of the channel, or
both. It limits voting, not seeing.
_Avoid_: poll filter, geo-block

### Media

**Media kind**:
The shape Telegram gives one sent file: photo, video, document, audio, animation,
sticker, video note or voice note. A property of the SENDING and never of the
bytes — the same recording is an audio file with a play button or a voice note
with a waveform depending only on what was asked for, and the same clip is a
video, a round video note or a soundless animation. Eight names, because eight is
what the encrypted protocol carries; every other Telegram message type has no
representation there at all.
A kind is asked for, not imposed: where the file cannot be it, the request is
refused before anything is uploaded. Two of those limits are the file's own shape
rather than its type — a video note has to be square, and an `.ogg` asked for as a
track has to say so in its MIME, because `audio/ogg` is Telegram's voice type.
_Avoid_: media type, file type, format, mime type

**Inferred kind**:
The media kind chosen from the file when the caller named none. One extension
admits a SET of kinds and one of them is its default; the inferred kind is that
default, and an extension the server does not recognise infers `document`, which
carries any bytes at all. A caller who names a kind outside the file's set is
refused before a byte is uploaded, so a send that succeeded was sent as the kind
that was asked for.
One family is inferred from the file's CONTENT rather than its name: an `.ogg`
holds a voice note and a piece of music in the same container, codec, channel
count and sample rate, so the metadata decides — no music tags means a voice
note, any music tag means audio. An explicit kind is never second-guessed.
_Avoid_: detected type, guessed kind, auto kind

**Split send**:
One request that becomes several Telegram messages. A media group carries one
answer to "compressed or as a file" for everything in it, so entries whose kinds
cannot share a group are sent as separate messages, in the order the caller wrote
them. A photo beside a document is two messages; a voice note, video note or
sticker is always its own. It is a property of the REQUEST, not a failure of it —
nothing is refused and nothing is re-typed to make one message.
_Avoid_: batch, chunk, fallback, partial send

**Kind parity**:
The property of this server that every media kind reachable on one send path is
reachable on the others. A property of the server's surface, not of a chat or a
file: it is what stops a capability existing in a secret chat and silently
missing from an ordinary one.
_Avoid_: feature parity, coverage, completeness

### Secret chats

**Secret layer**:
The protocol version two devices settled on when they opened one secret chat, and
the ceiling on what that chat can express. A property of the one chat, not of the
account or of this server.
_Avoid_: version, protocol level, encryption level

**Dropped entity**:
Formatting a message carried that its secret layer cannot express, discarded in
transit with no error on either side. A property of the pairing of a format and a
layer, so the same message is whole in one chat and thinned in another.
_Avoid_: unsupported formatting, stripped markup, lost entity

**Read horizon**:
The moment in time a secret chat's read receipt marks. Reading is addressed by date
rather than by message, so a secret chat has a point before which everything is
read, and never a set of individually read messages.
_Avoid_: read receipt, seen marker, last read message

**Timed send**:
Arming a secret chat's self-destruct timer, sending one message under it, and
restoring the timer that was there before. Three acts that read as one, over a
timer that belongs to the chat and therefore to both people in it.
_Avoid_: ephemeral send, one-shot timer, disappearing message

### Acting on the owner's behalf

**Safeguard**:
The layer that decides, for every tool call, whether it runs freely, waits for the
owner's approval, or is refused.
_Avoid_: firewall, guardrail, auto mode, permission system

**Gated call**:
A tool call the safeguard will not run until the owner approves it.
_Avoid_: blocked call, dangerous tool

**Approval**:
The owner's explicit consent to one gated call, given where the model cannot answer
for them. It covers that call once, or the same tool in the same chat for the rest of
the session.
_Avoid_: confirmation, permission, consent flag

**Approval channel**:
Where an approval is asked for: the client's own dialog, the owner's approval bot, or
a code answered in Saved Messages. No channel means no approval, and the call is
refused.
_Avoid_: prompt, confirmation dialog

**Always approval**:
The owner's "always" answer: the same tool in the same chat of the same account runs
without asking again, across restarts, until the owner revokes it or resets them all.
Folders allowed "always" are kept apart and removed separately.
_Avoid_: whitelist, trusted tool

**Approval bot commands**:
The menu of `/` commands the approval bot publishes each time it starts, through which
the owner reads and removes always approvals, sees the connected accounts, the open
requests and the safeguard's state. Anyone can see the menu; only the owner is answered.

**Seen signal**:
Anything that tells another person the owner saw something or is present: a read
marker, being online, a story view, a voice or round video marked listened, a typing
indicator, a channel view counted.
_Avoid_: read receipt, seen, presence

**Ghost mode**:
The state in which this server gives off no seen signal without approval. It holds
for all accounts, one account, or one chat, and is on unless the owner turns it off.
_Avoid_: stealth mode, invisible mode, incognito

**Device**:
One signed-in Telegram session of the account (a phone, a desktop app, this server). Each
device separately accepts or refuses incoming calls and new secret chats; "secret chats only
on this device" leaves exactly one device accepting them.
_Avoid_: authorization (in prose), session (that is this server's own login)

### Reaching Telegram

**Connection route**:
How one account reaches Telegram: directly, or through one proxy from the proxy pool.
_Avoid_: connection, network path, tunnel

**Proxy pool**:
The owner's saved proxies, each with its last test result, from which a connection
route is chosen. One pool is shared by default; an account can be given its own.
_Avoid_: proxy list, proxy config

**Proxy source**:
A channel whose posts are read for proxy links, named by the owner by username, id
or link.
_Avoid_: proxy channel, feed


### Communities

**Community**:
A Telegram container that groups channels, groups and bots under one title and photo.
It has members of its own; it is not a group and carries no messages.
_Avoid_: folder, supergroup, hub

**Linked chat**:
A channel, group or bot attached to a community, either visible to every member or
hidden from all but invited members and community admins. The choice is made once,
when it is linked, and cannot be changed afterwards.
_Avoid_: member chat, child chat, sub-chat

**Link request**:
A chat's pending request to become a linked chat, waiting for an admin to approve or
reject it. It exists only when adding chats is restricted to admins.
_Avoid_: join request (that is a person asking to join a chat)

**Community ban**:
Removing a person from a community so they cannot return until unbanned.
_Avoid_: kick, remove member

### The chat list

**Folder**:
A named tab in the owner's chat list that shows chats chosen one by one, chats matching
its rules (all bots, all groups, ...), or both.
_Avoid_: filter, category, the Archive (that is a separate place, not a folder)

**Pinned chat**:
A chat held at the top of All chats, or at the top of one folder. Pinning in one place
does not pin it in the other.
_Avoid_: pinned message (that is a message pinned inside a chat)

**Mute**:
Stopping a chat's notifications until a chosen moment, or forever.
_Avoid_: silence, disable

**Silent chat**:
A chat whose notifications still arrive, but without sound.
_Avoid_: muted chat

**Chat tone**:
The sound a chat's notifications play: the default, none, or one of the owner's saved
sounds.
_Avoid_: ringtone (that is a call)

**Clear history**:
Removing a chat's messages while the chat stays in the list.
_Avoid_: delete chat

**Delete chat**:
Removing a chat from the owner's list; for a group or channel that means leaving it. The
group or channel itself goes on existing.
_Avoid_: clear history, delete group

**Both sides**:
Whether a deletion also removes the other person's copy. Never assumed: the owner says
so each time.
_Avoid_: revoke, for everyone

### Groups

**Member tag**:
A short label (up to 16 characters, no emoji) shown next to a member's name in a group.
An admin who may manage tags sets anyone's; a member sets only their own, and only when the
group allows it. An admin's tag is their admin title - the same thing, not a second label.
_Avoid_: rank, custom title, badge

**Kick**:
Removing a member who may come straight back: a ban lifted at once, so they are out of the
group but not on its removed list. A **ban** keeps them out until it is lifted.
_Avoid_: remove (ambiguous between the two)
