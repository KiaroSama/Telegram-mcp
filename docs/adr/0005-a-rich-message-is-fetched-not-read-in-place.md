# 5. A rich message is fetched, not read in place

Date: 2026-09-21

## Status

Accepted

## Context

A message composed with Telegram's rich formatting — a table, headings, lists — arrives
through an ordinary read **completely empty**: no text, no entities, no media, and no
error. Nothing failed to parse. There is simply nothing in the fields a client reads.

For most of this project's life the explanation on record was that the content type did
not exist in the TL layer the client library announces, and that only a second backend
speaking a newer layer could see it. That explanation drove a real dependency: two
rich-message tools ran on TDLib, which keeps its own authorization and therefore showed
the operator a second device for one account.

Measured again on 2026-09-21, the explanation was **stale in one crucial respect and
correct in another**, and the difference is the whole decision.

- Still true: there is no rich message content type on an ordinary message object. The
  message really does come back empty, and no amount of reading it harder will help.
- No longer true: that the client library cannot reach the content at all. It carries
  `messages.GetRichMessageRequest(peer, id)`, the 58 page-block types, the inline-file
  type the blocks name — and `Message.rich_message`, the field the body rides on.

So the body was never unreachable. It was never in the place everything was looking.

## Decision

**A rich message is fetched by `(chat, message id)` and rendered from what comes back.
It is never read in place, and it does not need a second backend.**

`read_rich_message` therefore issues `GetRichMessageRequest`, takes `rich_message` off
the returned message, and renders its blocks. The tool's arguments and its answer are
unchanged, because the caller's act is unchanged: notice that a message is empty, then
ask for it by id. That was always the shape — the previous backend was one way of
performing the second half, not the reason the second half exists.

The glossary records the property this rests on, deliberately naming no backend:
*"Emptiness is not evidence that a message is empty, and reading one is always two acts:
notice, then fetch."*

## Consequences

**The second authorization loses one of its two reasons.** Rich messages were one of
them; secret chats are the other, and their replacement is a separate piece of work.
Both must go before the account stops showing two devices, but this half is done.

**Rendering became this project's job.** The two protocols describe the same blocks
differently, and `telegram_mcp/rich_blocks.py` absorbs every difference so that nothing
above it can tell. Three of those differences were capabilities about to be lost
silently, and each is now a test:

- a cell that chose no alignment is published as `left`/`middle`, spelled out, because
  that is what the answer carried — returning a null would have been a faithful-looking
  translation of a field nobody asked to change;
- `type` keeps its published spelling, and a quote or a list item stays NESTED blocks
  rather than becoming a bare string, because a caller can switch on both;
- **formatting is re-emitted as markers** — `**bold**`, `[label](url)`, `$maths$`,
  `||spoiler||` and `glyph<tg-emoji id=N>` — because the text of a cell is not the whole
  of what the cell says. Dropping them would have lost every custom-emoji id, the exact
  loss an earlier fix in this project was written for.

**The proof is the method, not the conclusion.** A baseline was captured from the old
backend first, and the new renderer was compared against it field by field until the
difference count reached zero. Three regressions were caught that way and none of them
would have been caught by reading the new output and finding it plausible. That ordering
— capture, then replace, then compare — is why the baseline is taken while the old
backend is still installed.

## Alternatives considered

**Keep the second backend for the two rich-message tools alone.** Rejected: the second
authorization is the entire cost being removed, and paying it for two tools buys a
fraction of the benefit.

**Accept losing rich messages.** Rejected by the requirement the work was given — no
capability lost — and unnecessary once the fetch was found.

**Read the content off the ordinary message.** Not possible, and this is the part of the
old explanation that survives: there is no rich content type on a message object. The
fetch is not an optimisation, it is the only route.
