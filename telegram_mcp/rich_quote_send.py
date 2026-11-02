"""Build native article quotes on the existing layer232 client."""

import random
from telethon import functions, types, errors, utils
from telegram_mcp import rich_quotes


async def send_text(client, entity, text, parse_mode, entities, effect, target, send_as=None):
    """Use the raw sender only when a reply header cannot fit a bare message ID."""
    if not isinstance(target, types.InputReplyToMessage):
        return await client.send_message(
            entity,
            text,
            parse_mode=parse_mode,
            formatting_entities=entities,
            message_effect_id=effect,
            reply_to=target,
            **({"send_as": send_as} if send_as is not None else {}),
        )
    if parse_mode and not entities:
        parser = utils.sanitize_parse_mode(parse_mode)
        if parser:
            text, entities = parser.parse(text)
    return await client(
        functions.messages.SendMessageRequest(
            peer=entity,
            message=text,
            random_id=random.randint(0, 2**62),
            reply_to=target,
            entities=entities or None,
            effect=effect,
            send_as=send_as,
        )
    )


async def reply_target(client, peer, message_id, text, offset, block, row, column, topic):
    try:
        fetched = await client(functions.messages.GetRichMessageRequest(peer=peer, id=message_id))
    except errors.MessageIdInvalidError:
        if any(value is not None for value in (block, row, column)):
            raise rich_quotes.QuoteError("Quote coordinates require a rich article/table source.")
        return None
    message = next(iter(getattr(fetched, "messages", []) or []), None)
    rich = getattr(message, "rich_message", None)
    if rich is None:
        if any(value is not None for value in (block, row, column)):
            raise rich_quotes.QuoteError("Quote coordinates require a rich article/table source.")
        return None
    fragments, location = rich_quotes.select(rich, text, offset, block, row, column)
    return types.InputReplyToMessage(
        message_id,
        quote_rich=fragments,
        top_msg_id=topic,
        quote_offset=location["offset"],
    )
