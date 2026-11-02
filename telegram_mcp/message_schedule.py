"""Shared recurrence and UTC vocabulary for message authoring and relay."""

from datetime import datetime, timezone
from typing import Optional, Union

REPEAT_PERIODS = {"daily": 86400, "weekly": 604800}
_PREMIUM_NOTE = (
    "Telegram gates the recurring-message period behind Premium: the period value itself is "
    "accepted, but a non-Premium account gets PREMIUM_ACCOUNT_REQUIRED. Schedule it without "
    "repeat, or use a Premium account."
)


def _repeat_seconds(repeat: Optional[str]) -> Union[int, None, str]:
    if repeat is None or str(repeat).lower() in ("", "none", "off"):
        return None
    period = REPEAT_PERIODS.get(str(repeat).lower())
    if period is None:
        return (
            f"repeat must be one of {', '.join(REPEAT_PERIODS)} (or omitted) - got {repeat!r}. "
            "Telegram validates the period against a fixed set and rejects anything else with "
            "SCHEDULE_REPEAT_PERIOD_INVALID."
        )
    return period


def _as_utc(value: Union[str, int]) -> datetime:
    if isinstance(value, (int, float)):
        return datetime.fromtimestamp(int(value), tz=timezone.utc)
    parsed = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    return parsed.replace(tzinfo=timezone.utc) if parsed.tzinfo is None else parsed
