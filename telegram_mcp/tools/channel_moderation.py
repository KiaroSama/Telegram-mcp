"""Aggressive anti-spam and the boost bypass for a supergroup's restrictions.

Phase 1 of `docs/api-coverage.md`, moderation group. Every write goes through
`channel_settings._toggle`, so a basic group gets a sentence and an admin-rights
refusal reads the same as the other channel settings. All three are reversible
or informational: the setting can be switched back, and a false-positive report
only tells Telegram's filter it was wrong.
"""

from telegram_mcp.runtime import *
from telegram_mcp.tools.channel_settings import _toggle

__all__ = [
    "report_anti_spam_false_positive",
    "set_anti_spam",
    "set_boosts_to_unblock",
]


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Anti Spam",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_anti_spam(chat_id: Union[int, str], enabled: bool, account: str = None) -> str:
    """
    Turn Telegram's aggressive anti-spam filter on or off for a supergroup.

    Args:
        chat_id: The supergroup ID or username.
        enabled: True to let Telegram delete likely spam automatically.

    Telegram offers this only above a member threshold. Reversible.
    """
    return await _toggle(
        "set_anti_spam",
        chat_id,
        account,
        lambda entity: functions.channels.ToggleAntiSpamRequest(channel=entity, enabled=enabled),
        lambda title: f"Aggressive anti-spam is {'on' if enabled else 'off'} in {title}.",
        supergroup_only=True,
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Report Anti Spam False Positive",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=False,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def report_anti_spam_false_positive(
    chat_id: Union[int, str], message_id: int, account: str = None
) -> str:
    """
    Tell Telegram its anti-spam filter wrongly deleted a message.

    Args:
        chat_id: The supergroup ID or username.
        message_id: The id of the anti-spam service message about the deletion
            (from get_recent_actions).

    The report does not restore the message; it trains the filter.
    """
    return await _toggle(
        "report_anti_spam_false_positive",
        chat_id,
        account,
        lambda entity: functions.channels.ReportAntiSpamFalsePositiveRequest(
            channel=entity, msg_id=int(message_id)
        ),
        lambda title: f"Reported message {int(message_id)} in {title} as an anti-spam mistake.",
        supergroup_only=True,
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Boosts To Unblock",
        openWorldHint=True,
        destructiveHint=False,
        idempotentHint=True,
        readOnlyHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def set_boosts_to_unblock(chat_id: Union[int, str], boosts: int, account: str = None) -> str:
    """
    Let members who boosted a supergroup ignore its slow mode and send restrictions.

    Args:
        chat_id: The supergroup ID or username.
        boosts: How many boosts a member needs to be exempt; 0 turns the exemption off.

    Reversible: call again with another number.
    """
    if isinstance(boosts, bool) or not isinstance(boosts, int) or boosts < 0:
        return f"boosts must be a whole number from 0 upwards, not {boosts!r}."
    return await _toggle(
        "set_boosts_to_unblock",
        chat_id,
        account,
        lambda entity: functions.channels.SetBoostsToUnblockRestrictionsRequest(
            channel=entity, boosts=boosts
        ),
        lambda title: (
            f"Members of {title} with {boosts} boost(s) now ignore its restrictions."
            if boosts
            else f"Boosting no longer exempts members of {title} from its restrictions."
        ),
        supergroup_only=True,
    )
