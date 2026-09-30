"""`export_chat_history`: one chat exported exactly as Telegram Desktop exports it (spec 030).

The export itself is `telegram_mcp.tdexport` (its own suites). What this tool adds is
the dialog: the owner chooses the format, the media kinds, the size limit and the period
- in the client's form when it has one, otherwise through the agent, and never by a
silent default - and the folder must sit inside the allowed roots, judged before any
network call.

The installation folder is the test's own (conftest `_folders_are_the_tests_own`).
"""

from datetime import datetime
from types import SimpleNamespace

import pytest
from telethon.tl import types

from telegram_mcp import export_jobs
from telegram_mcp.safeguard import folders
from telegram_mcp.tdexport.fetch import ExportResult
from telegram_mcp.tdexport.settings import Format, MediaSettings
from telegram_mcp.tools import chat_export as mod

CHANNEL = types.Channel(id=555, title="News", photo=None, date=None, broadcast=True)
INPUT = types.InputPeerChannel(channel_id=555, access_hash=7)
M = MediaSettings.Type

EVERY_OPTION = dict(
    format="html",
    photos=True,
    videos=False,
    voice_messages=False,
    video_messages=False,
    stickers=False,
    gifs=False,
    files=False,
    size_limit_mb=8,
)


class FakeClient:
    def __init__(self):
        self.asked = []

    async def __call__(self, request):
        return SimpleNamespace(me_url_prefix="https://t.me/")

    async def get_input_entity(self, entity):
        self.asked.append(entity)
        return INPUT


class Session:
    """The client's session: elicitation is offered when `answer` is given."""

    def __init__(self, answer=None):
        self.answer = answer
        self.forms = []
        caps = SimpleNamespace(elicitation=object()) if answer else SimpleNamespace()
        self.client_capabilities = caps

    async def elicit_form(self, message, schema, related_request_id=None):
        self.forms.append((message, schema))
        return self.answer


@pytest.fixture
def export(wire_client, monkeypatch):
    """Wires a fake client and captures what the tool hands to tdexport."""
    client = FakeClient()
    wire_client(mod, client, entity=CHANNEL)
    runs = []

    async def _export(cl, settings, writer, environment=None):
        runs.append(SimpleNamespace(settings=settings, writer=writer))
        return ExportResult(settings.path + "ChatExport_2026-09-30/", True, "", 12, 3)

    monkeypatch.setattr(mod, "export_single_chat", _export)
    monkeypatch.setattr(mod.export_dialog, "writer_for", lambda fmt: f"writer:{fmt.name}")
    return SimpleNamespace(client=client, runs=runs)


@pytest.mark.asyncio
async def test_without_a_form_every_choice_must_come_from_the_owner(export):
    said = await mod.export_chat_history("@news", photos=True, format="html")

    assert export.runs == [] and export.client.asked == []
    for option in ("videos", "voice_messages", "video_messages", "stickers", "gifs"):
        assert option in said
    assert "files" in said and "size_limit_mb" in said and "Ask the owner" in said


@pytest.mark.asyncio
async def test_explicit_choices_become_telegram_desktop_settings(export):
    answer = await mod.export_chat_history(
        "@news", **{**EVERY_OPTION, "stickers": True, "size_limit_mb": 20, "format": "json"}
    )

    await export_jobs.settle()
    settings = export.runs[0].settings
    assert settings.single_peer == INPUT
    assert settings.format is Format.Json and export.runs[0].writer == "writer:Json"
    assert settings.media.types == M.Photo | M.Sticker
    assert settings.media.size_limit == 20 * 1024 * 1024
    assert settings.path.startswith(str(folders.default_download_dir()).replace("\\", "/"))
    assert "/ChatExport_" in settings.path  # the default folder always gets one
    assert '"status": "running"' in answer and '"job_id"' in answer
    await export_jobs.settle()
    status = await mod.export_status()
    assert '"messages": 12' in status and '"takeout": true' in status


@pytest.mark.asyncio
async def test_a_chosen_folder_is_used_as_is(export):
    await mod.export_chat_history("@news", destination="downloads/mine", **EVERY_OPTION)

    await export_jobs.settle()
    settings = export.runs[0].settings
    assert settings.path.rstrip("/").endswith("downloads/mine")
    assert settings.force_sub_path is False


@pytest.mark.asyncio
async def test_dates_start_at_the_local_day_like_the_desktop_calendar(export):
    await mod.export_chat_history(
        "@news", date_from="2026-09-01", date_to="2026-09-10 18:30", **EVERY_OPTION
    )

    await export_jobs.settle()
    settings = export.runs[0].settings
    assert settings.single_peer_from == int(datetime(2026, 9, 1).timestamp())
    assert settings.single_peer_till == int(datetime(2026, 9, 10, 18, 30).timestamp())


@pytest.mark.asyncio
async def test_a_till_before_the_from_moves_ten_minutes_after_it(export):
    await mod.export_chat_history(
        "@news", date_from="2026-09-10", date_to="2026-09-01", **EVERY_OPTION
    )

    await export_jobs.settle()
    settings = export.runs[0].settings
    assert settings.single_peer_till == settings.single_peer_from + 600


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change, word",
    [
        ({"format": "pdf"}, "format"),
        ({"size_limit_mb": 0}, "size_limit_mb"),
        ({"size_limit_mb": 4001}, "size_limit_mb"),
        ({"date_from": "yesterday"}, "date_from"),
    ],
)
async def test_a_bad_choice_is_refused_before_any_request(export, change, word):
    said = await mod.export_chat_history("@news", **{**EVERY_OPTION, **change})

    assert word in said and export.runs == [] and export.client.asked == []


@pytest.mark.asyncio
async def test_the_form_shows_the_desktop_defaults_and_its_answer_wins(export):
    session = Session(
        SimpleNamespace(
            action="accept",
            content={**EVERY_OPTION, "format": "html_and_json", "gifs": True},
        )
    )

    await mod.export_chat_history("@news", ctx=SimpleNamespace(session=session))

    schema = session.forms[0][1]["properties"]
    assert schema["photos"]["default"] is True and schema["gifs"]["default"] is False
    assert schema["size_limit_mb"]["default"] == 8 and schema["format"]["default"] == "html"
    await export_jobs.settle()
    settings = export.runs[0].settings
    assert settings.format is Format.HtmlAndJson and settings.media.types == M.Photo | M.GIF


@pytest.mark.asyncio
@pytest.mark.parametrize("action", ["decline", "cancel"])
async def test_a_dismissed_form_exports_nothing(export, action):
    session = Session(SimpleNamespace(action=action, content=None))

    said = await mod.export_chat_history("@news", ctx=SimpleNamespace(session=session))

    assert "cancelled" in said.lower() and export.runs == []


@pytest.mark.asyncio
async def test_a_folder_outside_every_root_is_refused_before_any_network_call(export, tmp_path):
    said = await mod.export_chat_history(
        "@news", destination=str(tmp_path / "elsewhere"), **EVERY_OPTION
    )

    assert "outside" in said.lower()
    assert export.client.asked == [] and not (tmp_path / "elsewhere").exists()


@pytest.mark.asyncio
async def test_a_path_trick_is_refused(export):
    said = await mod.export_chat_history("@news", destination="downloads/*", **EVERY_OPTION)

    assert export.runs == [] and said
