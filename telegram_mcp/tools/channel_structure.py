"""What a channel IS: delete it, turn a supergroup into a broadcast group, place it, translate it.

Phase 1 of `docs/api-coverage.md`, structural group. Two of these cannot be
undone - `delete_channel` and `convert_to_gigagroup` - and are declared
`destructiveHint=True` so the safeguard asks the owner every time; their
docstrings say so because the approval preview is built from them.
`set_channel_location` and `set_autotranslation` are ordinary reversible
settings and go through `channel_settings._toggle` like the rest of Phase 1.
"""

from telegram_mcp.runtime import *
from telegram_mcp.tools.channel_settings import _require_channel, _toggle

__all__ = [
    "convert_to_gigagroup",
    "delete_channel",
    "set_autotranslation",
    "set_channel_location",
]


@mcp.tool(
    annotations=ToolAnnotations(
        title="Convert To Gigagroup",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def convert_to_gigagroup(chat_id: Union[int, str], account: str = None) -> str:
    """
    Turn a supergroup into a broadcast group: only admins can post, with no member limit.

    Args:
        chat_id: The supergroup ID or username.

    IRREVERSIBLE: this cannot be undone, not even by Telegram support. Always
    asks the owner first. Telegram offers it only to large supergroups.
    """
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        entity, refusal = await _require_channel(chat_id, cl)
        if refusal:
            return refusal
        title = sanitize_name(getattr(entity, "title", str(chat_id)))
        if not getattr(entity, "megagroup", False):
            return (
                f"{title} is a broadcast channel already; only a supergroup can become "
                "a broadcast group."
            )
        await cl(functions.channels.ConvertToGigagroupRequest(channel=entity))
        return f"{title} is now a broadcast group. This cannot be undone."
    except telethon.errors.rpcerrorlist.ChatAdminRequiredError:
        return "Cannot convert: only the owner of the supergroup can do this."
    except Exception as e:
        return log_and_format_error("convert_to_gigagroup", e, chat_id=chat_id)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Delete Channel",
        openWorldHint=True,
        destructiveHint=True,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def delete_channel(chat_id: Union[int, str], account: str = None) -> str:
    """
    Delete a channel or supergroup for everyone, with its whole history.

    Args:
        chat_id: The channel or supergroup ID or username.

    IRREVERSIBLE: every member loses the chat and every message in it; this
    cannot be undone. Always asks the owner first. Only the creator can do it.
    To leave without deleting, use leave_chat.
    """
    return await _toggle(
        "delete_channel",
        chat_id,
        account,
        lambda entity: functions.channels.DeleteChannelRequest(channel=entity),
        lambda title: f"{title} was deleted for everyone. This cannot be undone.",
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Channel Location",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_channel_location(
    chat_id: Union[int, str],
    latitude: float,
    longitude: float,
    address: str,
    account: str = None,
) -> str:
    """
    Set the place a location-based supergroup is shown at.

    Args:
        chat_id: The supergroup ID or username.
        latitude: -90 to 90.
        longitude: -180 to 180.
        address: The address shown with it.

    Telegram accepts this only for groups created as location-based. Reversible:
    set another place.
    """
    try:
        lat, long = float(latitude), float(longitude)
    except (TypeError, ValueError):
        return "latitude and longitude must be numbers."
    if not (-90 <= lat <= 90 and -180 <= long <= 180):
        return f"({lat}, {long}) is not a place: latitude is -90..90, longitude -180..180."
    place = str(address or "").strip()
    if not place:
        return "Give the address to show with the location."
    return await _toggle(
        "set_channel_location",
        chat_id,
        account,
        lambda entity: functions.channels.EditLocationRequest(
            channel=entity, geo_point=types.InputGeoPoint(lat=lat, long=long), address=place
        ),
        lambda title: f"{title} is now located at {sanitize_name(place)}.",
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Autotranslation",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_autotranslation(chat_id: Union[int, str], enabled: bool, account: str = None) -> str:
    """
    Let Telegram translate a channel's posts automatically for its readers.

    Args:
        chat_id: The channel ID or username.
        enabled: True to offer automatic translation.

    Telegram requires a boost level. Reversible.
    """
    return await _toggle(
        "set_autotranslation",
        chat_id,
        account,
        lambda entity: functions.channels.ToggleAutotranslationRequest(
            channel=entity, enabled=enabled
        ),
        lambda title: f"Automatic translation is {'on' if enabled else 'off'} for {title}.",
    )
