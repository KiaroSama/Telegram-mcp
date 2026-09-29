"""How a channel looks: its colour, emoji status and sticker sets.

Phase 1 of `docs/api-coverage.md`, appearance group. Every write goes through
`channel_settings._toggle`. Telegram gates colours and emoji status on the
channel's boost level; its refusal is passed through as it comes rather than
guessed at here. Every setting is reversible: `None` clears it.
"""

from telegram_mcp.runtime import *
from telegram_mcp.tools.channel_settings import _toggle

__all__ = [
    "set_channel_color",
    "set_channel_emoji_pack",
    "set_channel_emoji_status",
    "set_channel_stickers",
]


def _sticker_set(set_short_name: Optional[str]):
    """A short name (or a pasted t.me/addstickers link), or the empty set for None."""
    name = str(set_short_name or "").strip().rstrip("/").rsplit("/", 1)[-1]
    if not name:
        return types.InputStickerSetEmpty()
    return types.InputStickerSetShortName(short_name=name)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Channel Color",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_channel_color(
    chat_id: Union[int, str],
    color: Optional[int] = None,
    background_emoji_id: Optional[int] = None,
    for_profile: bool = False,
    account: str = None,
) -> str:
    """
    Set a channel's accent colour and the custom emoji patterned behind it.

    Args:
        chat_id: The channel or supergroup ID or username.
        color: The palette index, or None to reset the colour.
        background_emoji_id: A custom emoji document id, or None for no pattern.
        for_profile: True to set the profile page colour instead of the one used
            for messages and link previews.

    Telegram requires a boost level for most colours. Reversible.
    """
    if color is not None and (isinstance(color, bool) or not isinstance(color, int) or color < 0):
        return f"color must be a palette index from 0 upwards, or None; got {color!r}."
    where = "profile" if for_profile else "name"
    return await _toggle(
        "set_channel_color",
        chat_id,
        account,
        lambda entity: functions.channels.UpdateColorRequest(
            channel=entity,
            for_profile=True if for_profile else None,
            color=color,
            background_emoji_id=background_emoji_id,
        ),
        lambda title: (
            f"The {where} colour of {title} is reset."
            if color is None and background_emoji_id is None
            else f"The {where} colour of {title} is set."
        ),
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Channel Emoji Status",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_channel_emoji_status(
    chat_id: Union[int, str], custom_emoji_id: Optional[int], account: str = None
) -> str:
    """
    Show a custom emoji next to a channel's name.

    Args:
        chat_id: The channel or supergroup ID or username.
        custom_emoji_id: A custom emoji document id, or None to remove the status.

    Telegram requires a boost level. Reversible.
    """
    status = (
        types.EmojiStatus(document_id=int(custom_emoji_id))
        if custom_emoji_id
        else types.EmojiStatusEmpty()
    )
    return await _toggle(
        "set_channel_emoji_status",
        chat_id,
        account,
        lambda entity: functions.channels.UpdateEmojiStatusRequest(
            channel=entity, emoji_status=status
        ),
        lambda title: (
            f"{title} now shows emoji status {int(custom_emoji_id)}."
            if custom_emoji_id
            else f"{title} no longer shows an emoji status."
        ),
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Channel Stickers",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_channel_stickers(
    chat_id: Union[int, str], set_short_name: Optional[str], account: str = None
) -> str:
    """
    Set the sticker set every member of a supergroup can use in it.

    Args:
        chat_id: The supergroup ID or username.
        set_short_name: The set's short name (or its t.me/addstickers link), or
            None to remove the group's set.

    Telegram allows this only above a member threshold. Reversible.
    """
    stickerset = _sticker_set(set_short_name)
    return await _toggle(
        "set_channel_stickers",
        chat_id,
        account,
        lambda entity: functions.channels.SetStickersRequest(
            channel=entity, stickerset=stickerset
        ),
        lambda title: _describe_set("sticker set", title, stickerset),
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Channel Emoji Pack",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_channel_emoji_pack(
    chat_id: Union[int, str], set_short_name: Optional[str], account: str = None
) -> str:
    """
    Set the custom emoji pack every member of a supergroup can use in it.

    Args:
        chat_id: The supergroup ID or username.
        set_short_name: The emoji pack's short name (or its t.me/addemoji link),
            or None to remove the group's pack.

    Telegram requires a boost level. Reversible.
    """
    stickerset = _sticker_set(set_short_name)
    return await _toggle(
        "set_channel_emoji_pack",
        chat_id,
        account,
        lambda entity: functions.channels.SetEmojiStickersRequest(
            channel=entity, stickerset=stickerset
        ),
        lambda title: _describe_set("emoji pack", title, stickerset),
    )


def _describe_set(kind: str, title: str, stickerset) -> str:
    if isinstance(stickerset, types.InputStickerSetEmpty):
        return f"{title} no longer has a group {kind}."
    return f"The group {kind} of {title} is now {sanitize_name(stickerset.short_name)}."
