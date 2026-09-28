"""Telegram Business settings: hours, away and greeting messages, intro, location (plan 018).

The work-hours validator is the part with real logic, so it is tested against
every worked example on https://core.telegram.org/api/business ("Opening hours")
before anything else: sort, merge overlapping or touching intervals modulo the
week, split the Sunday->Monday case that would end past 8 days, clamp to one
week, and refuse out-of-bounds input in words BEFORE any request.
"""

from datetime import datetime, timezone

import pytest
from telethon.errors import RPCError
from telethon.tl import functions, types

from telegram_mcp.tools import business as mod

DAY = 24 * 60
WEEK = 7 * DAY
MON, THU, SUN = 0, 3 * DAY, 6 * DAY


def hm(day, hours, minutes=0):
    return day + hours * 60 + minutes


class Recorder:
    def __init__(self, answer=True):
        self.sent = []
        self.answer = answer

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(self.answer, Exception):
            raise self.answer
        return self.answer


# --- the validator, against the documentation's own examples ---------------


@pytest.mark.parametrize(
    "given,expected",
    [
        # Monday 00:00-00:01 and 00:01-00:10 touch: one interval.
        ([[hm(MON, 0), hm(MON, 0, 1)], [hm(MON, 0, 1), hm(MON, 0, 10)]], [(0, 10)]),
        # Overlapping.
        ([[hm(MON, 0), hm(MON, 0, 5)], [hm(MON, 0, 1), hm(MON, 0, 10)]], [(0, 10)]),
        # Contained.
        ([[hm(MON, 0), hm(MON, 1)], [hm(MON, 0, 1), hm(MON, 0, 10)]], [(0, 60)]),
        # Sunday 16:00-Monday 01:00 and Monday 01:00-03:00: merged across the week end.
        (
            [[hm(SUN, 16), WEEK + 60], [hm(MON, 1), hm(MON, 3)]],
            [(hm(SUN, 16), WEEK + hm(MON, 3))],
        ),
        # The special case: merging would end past 8 days, so it splits at the week.
        (
            [[hm(SUN, 16), WEEK + 60], [hm(MON, 0, 30), hm(THU, 3)]],
            [(0, hm(THU, 3)), (hm(SUN, 16), WEEK)],
        ),
        (
            [[hm(SUN, 16), WEEK + 60], [hm(MON, 1), hm(THU, 3)]],
            [(0, hm(THU, 3)), (hm(SUN, 16), WEEK)],
        ),
        # Unsorted input comes back sorted; separate intervals stay separate.
        ([[hm(THU, 9), hm(THU, 17)], [hm(MON, 9), hm(MON, 17)]], [(540, 1020), (4860, 5340)]),
        # Longer than a week is clamped to exactly one week.
        ([[10, 8 * DAY]], [(10, 10 + WEEK)]),
    ],
)
def test_work_hours_are_normalized_as_documented(given, expected):
    normalized, refusal = mod.normalize_work_hours(given)

    assert refusal is None
    assert normalized == expected


@pytest.mark.parametrize(
    "given",
    [
        [[-1, 10]],  # start below 0
        [[WEEK + 1, WEEK + 10]],  # start past 7 days
        [[0, 8 * DAY + 1]],  # end past 8 days
        [[10, 10]],  # shorter than one minute
        [[10, 5]],
        [[1, 2, 3]],
        [["9:00", "17:00"]],
        [[True, 5]],
        [[i * 100, i * 100 + 10] for i in range(29)],  # more than 28 intervals
    ],
)
def test_bad_intervals_are_refused_in_words(given):
    normalized, refusal = mod.normalize_work_hours(given)

    assert normalized is None
    assert refusal


# --- the tools -------------------------------------------------------------


@pytest.mark.asyncio
async def test_set_hours_sends_the_normalized_week(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_hours("Asia/Tehran", [[600, 700], [650, 900], [60, 120]])

    request = client.sent[0]
    assert isinstance(request, functions.account.UpdateBusinessWorkHoursRequest)
    hours = request.business_work_hours
    assert hours.timezone_id == "Asia/Tehran"
    assert [(w.start_minute, w.end_minute) for w in hours.weekly_open] == [(60, 120), (600, 900)]


@pytest.mark.asyncio
async def test_none_clears_the_hours(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_hours("Asia/Tehran", None)

    assert client.sent[0].business_work_hours is None


@pytest.mark.parametrize(
    "timezone_id,intervals", [("", [[0, 10]]), ("Asia/Tehran", []), ("UTC", [[5, 5]])]
)
@pytest.mark.asyncio
async def test_hours_refusals_happen_before_any_request(wire_client, timezone_id, intervals):
    client = Recorder()
    wire_client(mod, client)

    result = await mod.set_business_hours(timezone_id, intervals)

    assert client.sent == []
    assert result


@pytest.mark.parametrize(
    "schedule,dates,expected",
    [
        ("always", {}, types.BusinessAwayMessageScheduleAlways),
        ("outside_hours", {}, types.BusinessAwayMessageScheduleOutsideWorkHours),
        (
            "custom",
            {"start_date": "2026-10-01T00:00:00+00:00", "end_date": "2026-10-08T00:00:00+00:00"},
            types.BusinessAwayMessageScheduleCustom,
        ),
    ],
)
@pytest.mark.asyncio
async def test_away_message_points_at_a_quick_reply(wire_client, schedule, dates, expected):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_away_message(12, schedule=schedule, offline_only=True, **dates)

    message = client.sent[0].message
    assert isinstance(message, types.InputBusinessAwayMessage)
    assert message.shortcut_id == 12
    assert isinstance(message.schedule, expected)
    assert message.offline_only is True
    recipients = message.recipients
    assert (
        recipients.existing_chats,
        recipients.new_chats,
        recipients.contacts,
        recipients.non_contacts,
    ) == (True, True, True, True), "no recipients named means everyone"
    if schedule == "custom":
        assert message.schedule.start_date == datetime(2026, 10, 1, tzinfo=timezone.utc)


@pytest.mark.parametrize(
    "kwargs",
    [
        {"schedule": "weekends"},
        {"schedule": "custom"},
        {"schedule": "custom", "start_date": "tomorrow", "end_date": "later"},
        {
            "schedule": "custom",
            "start_date": "2026-10-08T00:00:00+00:00",
            "end_date": "2026-10-01T00:00:00+00:00",
        },
        {"recipients": ["friends"]},
    ],
)
@pytest.mark.asyncio
async def test_away_message_refusals_happen_before_any_request(wire_client, kwargs):
    client = Recorder()
    wire_client(mod, client)

    result = await mod.set_business_away_message(12, **kwargs)

    assert client.sent == []
    assert result


@pytest.mark.asyncio
async def test_greeting_and_away_clear_with_none(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_greeting(None)
    await mod.set_business_away_message(None)

    greeting, away = client.sent
    assert isinstance(greeting, functions.account.UpdateBusinessGreetingMessageRequest)
    assert greeting.message is None
    assert isinstance(away, functions.account.UpdateBusinessAwayMessageRequest)
    assert away.message is None


@pytest.mark.asyncio
async def test_greeting_names_its_recipients_and_inactivity(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_greeting(5, no_activity_days=14, recipients=["new_chats"])

    message = client.sent[0].message
    assert isinstance(message, types.InputBusinessGreetingMessage)
    assert (message.shortcut_id, message.no_activity_days) == (5, 14)
    assert message.recipients.new_chats is True
    assert not message.recipients.contacts


@pytest.mark.asyncio
async def test_greeting_days_are_one_of_four(wire_client):
    client = Recorder()
    wire_client(mod, client)

    result = await mod.set_business_greeting(5, no_activity_days=10)

    assert client.sent == []
    assert "7, 14, 21 or 28" in result


@pytest.mark.asyncio
async def test_intro_with_and_without_a_sticker(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_intro("Hi", "We answer within a day.", 111, 222)
    await mod.set_business_intro(None)

    first, cleared = client.sent
    intro = first.intro
    assert (intro.title, intro.description) == ("Hi", "We answer within a day.")
    assert (intro.sticker.id, intro.sticker.access_hash) == (111, 222)
    assert cleared.intro is None


@pytest.mark.asyncio
async def test_a_sticker_needs_both_halves(wire_client):
    client = Recorder()
    wire_client(mod, client)

    result = await mod.set_business_intro("Hi", "x", sticker_document_id=111)

    assert client.sent == []
    assert "access_hash" in result


@pytest.mark.asyncio
async def test_location_set_address_only_and_cleared(wire_client):
    client = Recorder()
    wire_client(mod, client)

    await mod.set_business_location(35.7, 51.4, "Valiasr St")
    await mod.set_business_location(None, None, "Online only")
    await mod.set_business_location(None, None, None)

    point, address_only, cleared = client.sent
    assert (point.geo_point.lat, point.geo_point.long, point.address) == (35.7, 51.4, "Valiasr St")
    assert address_only.geo_point is None and address_only.address == "Online only"
    assert cleared.geo_point is None and cleared.address is None


@pytest.mark.parametrize(
    "lat,long,address",
    [(35.7, 51.4, None), (35.7, None, "x"), (95, 0, "x"), (0, 0, "x" * 97)],
)
@pytest.mark.asyncio
async def test_location_refusals_happen_before_any_request(wire_client, lat, long, address):
    client = Recorder()
    wire_client(mod, client)

    result = await mod.set_business_location(lat, long, address)

    assert client.sent == []
    assert result


@pytest.mark.asyncio
async def test_a_premium_refusal_is_a_sentence(wire_client):
    client = Recorder(RPCError(request=None, message="PREMIUM_ACCOUNT_REQUIRED", code=403))
    wire_client(mod, client)

    result = await mod.set_business_intro("Hi", "there")

    assert "Telegram Premium" in result
    assert "PREMIUM_ACCOUNT_REQUIRED" not in result


@pytest.mark.asyncio
async def test_settings_read_maps_each_field_and_null_when_unset(wire_client):
    full = types.UserFull(
        id=1,
        settings=types.PeerSettings(),
        notify_settings=types.PeerNotifySettings(),
        common_chats_count=0,
        business_work_hours=types.BusinessWorkHours(
            timezone_id="Asia/Tehran",
            weekly_open=[types.BusinessWeeklyOpen(start_minute=540, end_minute=1020)],
            open_now=True,
        ),
        business_location=types.BusinessLocation(address="Valiasr​ St"),
        business_away_message=types.BusinessAwayMessage(
            shortcut_id=3,
            schedule=types.BusinessAwayMessageScheduleOutsideWorkHours(),
            recipients=types.BusinessRecipients(contacts=True),
        ),
    )
    client = Recorder(types.users.UserFull(full_user=full, chats=[], users=[]))
    wire_client(mod, client)

    result = await mod.get_business_settings()

    assert isinstance(client.sent[0], functions.users.GetFullUserRequest)
    assert '"timezone_id": "Asia/Tehran"' in result
    assert '"weekly_open": [[540, 1020]]' in result
    assert '"address": "Valiasr St"' in result
    assert '"schedule": "outside_hours"' in result
    assert '"recipients": ["contacts"]' in result
    assert '"greeting": null' in result and '"intro": null' in result


def test_every_write_is_destructive_and_the_read_is_not():
    from telegram_mcp.runtime import mcp

    get = mcp._tool_manager.get_tool
    assert get("get_business_settings").annotations.read_only_hint is True
    for name in (
        "set_business_hours",
        "set_business_away_message",
        "set_business_greeting",
        "set_business_intro",
        "set_business_location",
    ):
        assert get(name).annotations.destructive_hint is True, name
