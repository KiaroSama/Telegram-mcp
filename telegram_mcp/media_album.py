"""One request, the messages Telegram will actually accept.

`media_send` decides WHICH entries can share a message; this performs the sends
and says what happened. The two are separate because the first is a pure question
about the vocabulary and is tested as one, while this one needs a client.

It takes an already-resolved client and entity rather than resolving them itself.
That is deliberate: `tools/media.py` owns account resolution and is where the
tests patch it, and a second module resolving its own client would be a second
place for that to go wrong.

It is a module of its own because `tools/media.py` stood at 776 lines when this
was written, and the project closes a file to new code at about 700.
"""

from telegram_mcp import media_send

__all__ = ["send_planned", "describe"]


async def send_planned(
    *, client, entity, sources, kinds, caption=None, reply_to=None, posting_as=None
):
    """Send one message per group, in the order the caller wrote.

    `sources` and `kinds` are parallel lists. Returns the receipts in order, plus
    the plan that produced them, so the caller can name every message it sent.

    The caption rides the FIRST message only. Telegram already shows an album's
    caption on its first item, and repeating it on each split message would put
    text in the chat the caller never wrote.
    """
    plan = media_send.group_sends(kinds)
    receipts = []
    for position, (indices, flags) in enumerate(plan):
        group = [sources[index] for index in indices]
        receipts.append(
            await client.send_file(
                entity,
                # A single file is sent as itself, not as a one-member album:
                # Telethon treats a list as a media group and a group of one is
                # not what a lone voice note or sticker is.
                group if len(group) > 1 else group[0],
                caption=caption if position == 0 else None,
                reply_to=reply_to,
                **({"send_as": posting_as} if posting_as is not None else {}),
                **flags,
            )
        )
    return receipts, plan


def describe(chat_id, names, kinds, plan):
    """Every message sent and the kind each carried - never just the first.

    A caller who asked for a photo and a file gets two messages back; a reply
    naming one of them is a reply that hides half of what happened.
    """
    messages = [
        ", ".join(f"{names[index]} as {kinds[index]}" for index in indices) for indices, _ in plan
    ]
    count = f"{len(messages)} message" + ("" if len(messages) == 1 else "s")
    return f"Sent {count} to chat {chat_id}: " + "; ".join(messages) + "."
