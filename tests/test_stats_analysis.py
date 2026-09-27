"""Statistics analysis (spec 015 US2), on data shaped like a live supergroup's.

The graphs arrive as Telegram sends them: daily series with millisecond x values,
a 24-point hours graph (this week, last week), and a weekdays graph whose series are
the seven days, bucketed by week. What these pin: the period totals and the change
against the equal period before, the peak and low day, shares, a level graph (total
members) read as start/end rather than summed, the busiest hour and weekday, and a
deleted account marked as one.
"""

import json
from types import SimpleNamespace

import pytest
from telethon.tl import types as tl

from telegram_mcp.tools import stats_analysis as mod

DAY = 86_400_000
START = 1_782_518_400_000  # 2026-06-27


def _daily(names, *series):
    points = len(series[0])
    columns = [["x"] + [START + i * DAY for i in range(points)]]
    columns += [[f"y{i}"] + list(values) for i, values in enumerate(series)]
    return {"status": "loaded", "series": names, "columns": columns}


def _described():
    joined = [1] * 10 + [2] * 10  # previous 10 days: 10, current 10 days: 20
    left = [0] * 10 + [1] * 9 + [5]
    return {
        "members": {"current": 120.0, "previous": 100.0, "delta": 20.0},
        "growth_graph": _daily(["Total members"], list(range(100, 120))),
        "members_graph": _daily(["Joined", "Left"], joined, left),
        "top_hours_graph": {
            "status": "loaded",
            "series": ["This week", "Last week"],
            "columns": [
                ["x"] + list(range(24)),
                ["y0"] + [1] * 20 + [9, 1, 1, 1],
                ["y1"] + [2] * 24,
            ],
        },
        "weekdays_graph": _daily(
            ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
            [5, 5],
            [1, 1],
            [1, 1],
            [1, 1],
            [1, 1],
            [1, 1],
            [9, 9],
        ),
        "actions_graph": {"status": "error", "error": "not enough data"},
        "top_posters": [{"user_id": 7, "name": "", "messages": 89}],
    }


def test_a_flow_graph_is_totalled_and_compared_with_the_period_before():
    result = mod.analyze(_described(), days=10, include_series=False)
    joined = result["members_graph"]["series"]["Joined"]
    assert joined["total"] == 20 and joined["previous_total"] == 10
    assert joined["change_percent"] == 100.0
    left = result["members_graph"]["series"]["Left"]
    assert left["peak"]["value"] == 5 and left["peak"]["date"] == "2026-07-16"
    assert joined["share_percent"] == round(100 * 20 / 34, 1)


def test_a_level_graph_is_read_as_start_and_end():
    growth = mod.analyze(_described(), days=10, include_series=False)["growth_graph"]
    total = growth["series"]["Total members"]
    assert (total["start"], total["end"], total["change"]) == (110, 119, 9)
    assert "total" not in total


def test_busiest_hour_and_weekday():
    result = mod.analyze(_described(), days=14, include_series=False)
    assert result["top_hours_graph"]["busiest_hour"] == 20
    assert result["weekdays_graph"]["busiest_weekday"] == "Sunday"
    assert result["weekdays_graph"]["quietest_weekday"] in {"Tuesday", "Wednesday"}


def test_a_graph_telegram_refused_is_reported_not_invented():
    result = mod.analyze(_described(), days=10, include_series=False)
    assert result["actions_graph"] == {"status": "error", "error": "not enough data"}


def test_raw_series_only_on_request():
    assert "columns" not in mod.analyze(_described(), 10, False)["members_graph"]
    assert "columns" in mod.analyze(_described(), 10, True)["members_graph"]


def test_too_short_a_history_says_there_is_no_previous_period():
    joined = mod.analyze(_described(), days=15, include_series=False)["members_graph"]
    assert joined["series"]["Joined"]["previous_total"] is None


def test_counters_carry_their_percent_change():
    assert mod.analyze(_described(), 10, False)["members"]["change_percent"] == 20.0


def test_a_deleted_account_is_marked():
    rows = [{"user_id": 7, "name": ""}, {"user_id": 8, "name": "Sara"}]
    users = [tl.User(id=7, deleted=True), tl.User(id=8, first_name="Sara")]
    marked = mod.mark_deleted(rows, users)
    assert marked[0]["deleted_account"] is True and marked[0]["name"] == "Deleted Account"
    assert "deleted_account" not in marked[1]


@pytest.mark.asyncio
async def test_the_tool_refuses_a_chat_without_statistics(monkeypatch):
    async def _resolve(value, cl=None, account=None):
        return tl.User(id=1, first_name="x")

    async def _connected(cl=None):
        return None

    monkeypatch.setattr(mod, "get_client", lambda account=None: SimpleNamespace())
    monkeypatch.setattr(mod, "resolve_entity", _resolve)
    monkeypatch.setattr(mod, "ensure_connected", _connected)
    text = await mod.analyze_chat_statistics("user")
    assert "statistics" in text.lower()
    with pytest.raises(json.JSONDecodeError):
        json.loads(text)
