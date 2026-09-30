"""`export_recent_actions`: the admin log written as a Telegram Desktop chat export (spec 033 R10).

The sentences are `test_admin_log_sentences`. What these pin: the log is read back page by
page through max_id until Telegram has no more, oldest first; the filters reach the request;
each event lands in `messages.html` and `result.json` through tdexport's own writers - the
sentence as a service line (escaped in HTML, a JSON string in result.json), the deleted
message as an ordinary bubble with its text and its photo downloaded under the export's
media rules; and the tool follows `export_chat_history`'s dialog: the owner's choices,
refusals before any request, a background job. Synthetic Telethon objects, no network.
"""

import asyncio
import json
from datetime import date as Date
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon import errors
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp import admin_log_export as export
from telegram_mcp import admin_log_text, export_jobs
from telegram_mcp.tdexport import files as tdfiles
from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.settings import Format, MediaSettings, Settings
from telegram_mcp.tools import admin_log_export as tool
from telegram_mcp.tools import chat_export

GROUP = tl.Channel(
    id=777, title="Test Group", photo=tl.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=5
)
NEWS = tl.Channel(id=555, title="News", photo=None, date=None, broadcast=True, access_hash=6)
INPUT = tl.InputPeerChannel(channel_id=777, access_hash=5)
SARA = tl.User(id=42, first_name="Sara", username="sara_admin", access_hash=4)
BOB = tl.User(id=7, first_name="Bob", access_hash=3)
T0 = datetime(2026, 10, 1, 9, 0, tzinfo=timezone.utc)


@pytest.fixture(autouse=True)
def pinned(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)
    monkeypatch.setattr(model_format, "_COUNTRIES", [])
    monkeypatch.setattr(admin_log_text, "_today", lambda: Date(2026, 10, 1))

    class _Today(Date):
        @classmethod
        def today(cls):
            return Date(2026, 10, 1)

    monkeypatch.setattr(tdfiles, "Date", _Today)


def at(minutes):
    return datetime.fromtimestamp(T0.timestamp() + minutes * 60, timezone.utc)


def event(event_id, action, minutes=0, user_id=42):
    return tl.ChannelAdminLogEvent(id=event_id, date=at(minutes), user_id=user_id, action=action)


def deleted_photo_message():
    photo = tl.Photo(
        id=31,
        access_hash=1,
        file_reference=b"r",
        date=T0,
        sizes=[tl.PhotoSize(type="y", w=10, h=10, size=3)],
        dc_id=2,
    )
    return tl.Message(
        id=12,
        peer_id=tl.PeerChannel(777),
        from_id=tl.PeerUser(7),
        date=T0,
        message="buy now",
        entities=[tl.MessageEntityBold(offset=0, length=3)],
        media=tl.MessageMediaPhoto(photo=photo),
    )


DELETE = tl.ChannelAdminLogEventActionDeleteMessage(message=deleted_photo_message())
TITLE = tl.ChannelAdminLogEventActionChangeTitle(prev_value="a", new_value="<b>Fun</b>")
JOIN = tl.ChannelAdminLogEventActionParticipantJoin()


class LogClient:
    """Answers the export's requests; `pages` are getAdminLog answers, newest first."""

    def __init__(self, pages):
        self.pages = list(pages)
        self.requests = []
        self.log_requests = []
        self.downloads = []

    async def __call__(self, request):
        self.requests.append(request)
        if isinstance(request, functions.users.GetUsersRequest):
            return [tl.User(id=1, is_self=True, first_name="Me")]
        if isinstance(request, functions.help.GetCountriesListRequest):
            raise errors.RPCError(request, "COUNTRIES_UNAVAILABLE", 400)
        if isinstance(request, functions.channels.GetChannelsRequest):
            return tl.messages.Chats(chats=[GROUP])
        if isinstance(request, functions.channels.GetAdminLogRequest):
            self.log_requests.append(request)
            events = self.pages.pop(0) if self.pages else []
            return tl.channels.AdminLogResults(events=events, chats=[GROUP], users=[SARA, BOB])
        if isinstance(request, functions.messages.GetCustomEmojiDocumentsRequest):
            return []
        raise AssertionError(f"unexpected request {type(request).__name__}")

    async def download_file(self, location, file, *, part_size_kb, file_size, dc_id):
        self.downloads.append(location.id)
        file.write(b"x" * (file_size or 1))


def run(client, tmp_path, fmt=Format.HtmlAndJson, **kwargs):
    settings = Settings(single_peer=INPUT, path=str(tmp_path), format=fmt)
    for name in ("single_peer_from", "single_peer_till"):
        if name in kwargs:
            setattr(settings, name, kwargs.pop(name))
    writer = export.writer_for(fmt)
    return asyncio.run(export.export_admin_log(client, settings, writer, **kwargs))


def test_a_deleted_message_is_a_sentence_then_its_own_bubble_with_its_photo(tmp_path):
    client = LogClient([[event(2, TITLE, 5), event(1, DELETE, 0)]])

    result = run(client, tmp_path)

    html = (tmp_path / "messages.html").read_text(encoding="utf-8")
    assert "Test Group - Recent actions" in html
    first = html.index("Sara deleted message:")
    assert first < html.index("<strong>buy</strong> now") < html.index("Sara changed group name")
    assert "&lt;b&gt;Fun&lt;/b&gt;" in html and "<b>Fun</b>" not in html
    photos = list((tmp_path / "photos").iterdir())
    assert len(photos) == 1 and client.downloads == [31]
    assert result.events == 2 and result.messages == 3 and result.files == 1

    data = json.loads((tmp_path / "result.json").read_text(encoding="utf-8"))
    assert data["name"] == "Test Group - Recent actions"
    line, bubble, title = data["messages"]
    assert line["type"] == "service" and line["information_text"] == "Sara deleted message:"
    assert line["actor"] == "Sara" and line["actor_id"] == "user42"
    assert bubble["type"] == "message" and bubble["from"] == "Bob"
    assert bubble["photo"].startswith("photos/") and bubble["text_entities"][0]["type"] == "bold"
    assert title["information_text"] == "Sara changed group name to \u00ab<b>Fun</b>\u00bb"
    assert [m["id"] for m in data["messages"]] == [1, 2, 3]


def test_a_photo_the_owner_did_not_choose_is_not_downloaded(tmp_path):
    settings = Settings(single_peer=INPUT, path=str(tmp_path), format=Format.Json)
    settings.media = MediaSettings(types=MediaSettings.Type(0), size_limit=8 << 20)
    client = LogClient([[event(1, DELETE)]])

    asyncio.run(export.export_admin_log(client, settings, export.writer_for(Format.Json)))

    assert client.downloads == [] and not (tmp_path / "photos").exists()


def test_a_new_chat_photo_is_a_line_with_its_photo(tmp_path):
    new_photo = deleted_photo_message().media.photo
    change = tl.ChannelAdminLogEventActionChangePhoto(tl.PhotoEmpty(0), new_photo)
    client = LogClient([[event(1, change)]])

    run(client, tmp_path)

    assert "Sara changed group photo" in (tmp_path / "messages.html").read_text(encoding="utf-8")
    (line,) = json.loads((tmp_path / "result.json").read_text(encoding="utf-8"))["messages"]
    assert line["action"] == "edit_group_photo" and line["photo"].startswith("photos/")


def test_the_whole_log_is_read_back_through_max_id_until_telegram_has_no_more(tmp_path):
    first = [event(1000 - i, JOIN, -i) for i in range(100)]
    second = [event(800, JOIN, -500)]
    client = LogClient([first, second, []])

    result = run(client, tmp_path, Format.Json)

    assert [r.max_id for r in client.log_requests] == [0, 901, 800]
    assert all(r.limit == 100 and r.min_id == 0 for r in client.log_requests)
    assert result.events == 101
    data = json.loads((tmp_path / "result.json").read_text(encoding="utf-8"))
    assert data["messages"][0]["date_unixtime"] == str(int(at(-500).timestamp()))


def test_the_period_stops_the_reading_and_trims_the_events(tmp_path):
    pages = [
        [event(3, JOIN, 60), event(2, JOIN, 30)],
        [event(1, JOIN, -60)],
        [event(0, JOIN, -90)],
    ]
    client = LogClient(pages)
    since, till = int(at(0).timestamp()), int(at(45).timestamp())

    result = run(client, tmp_path, Format.Json, single_peer_from=since, single_peer_till=till)

    assert len(client.log_requests) == 2  # the second page already reached before `since`
    assert result.events == 1


def test_the_filters_reach_the_request(tmp_path):
    client = LogClient([[]])
    flt = tl.ChannelAdminLogEventsFilter(delete=True)
    admins = [tl.InputUser(42, 4)]

    result = run(client, tmp_path, Format.Json, events_filter=flt, admins=admins, query="spam")

    (request,) = client.log_requests
    assert request.events_filter is flt and request.admins == admins and request.q == "spam"
    assert request.channel == tl.InputChannel(777, 5)
    assert result.events == 0


# --- the tool --------------------------------------------------------------------------

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


class ToolClient:
    def __init__(self):
        self.requests = []
        self.asked = []

    async def __call__(self, request):
        self.requests.append(request)
        return SimpleNamespace(me_url_prefix="https://t.me/")

    async def get_input_entity(self, entity):
        self.asked.append(entity)
        return INPUT


@pytest.fixture
def wired(wire_client, monkeypatch):
    client = ToolClient()
    names = {"@group": GROUP, "@news": NEWS, "@sara_admin": SARA, "@bob": BOB}

    async def _resolve(value, cl=None, account=None):
        return names[value]

    wire_client(tool, client, resolve=_resolve)
    runs = []

    async def _export(cl, settings, writer, environment=None, **filters):
        runs.append(SimpleNamespace(settings=settings, writer=writer, env=environment, **filters))
        return export.AdminLogResult(settings.path, 4, 6, 1)

    monkeypatch.setattr(tool.admin_log_export, "export_admin_log", _export)
    return SimpleNamespace(client=client, runs=runs)


@pytest.mark.asyncio
async def test_the_tool_starts_a_background_export_with_the_filters(wired):
    answer = await tool.export_recent_actions(
        "@group",
        event_types=["deleted_messages", "pinned_messages"],
        admins=["@sara_admin"],
        query="spam",
        **{**EVERY_OPTION, "format": "html_and_json"},
    )

    assert '"status": "running"' in answer and '"kind": "recent_actions"' in answer
    await export_jobs.settle()
    (job,) = wired.runs
    assert job.settings.single_peer == INPUT and job.settings.format is Format.HtmlAndJson
    assert job.events_filter.delete and job.events_filter.pinned and not job.events_filter.join
    assert job.admins == [tl.InputUser(42, 4)] and job.query == "spam"
    assert job.env.internal_links_domain == "https://t.me/"
    assert "/ChatExport_" in job.settings.path
    status = await chat_export.export_status()
    assert '"events": 4' in status and '"messages": 6' in status


@pytest.mark.asyncio
async def test_without_a_form_every_choice_must_come_from_the_owner(wired):
    said = await tool.export_recent_actions("@group", photos=True, format="html")

    assert "Ask the owner" in said and "size_limit_mb" in said
    assert wired.runs == [] and wired.client.requests == [] and wired.client.asked == []


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change, word",
    [
        ({"event_types": ["gossip"]}, "gossip"),
        ({"format": "pdf"}, "format"),
        ({"size_limit_mb": 0}, "size_limit_mb"),
        ({"date_to": "tomorrow"}, "date_to"),
    ],
)
async def test_a_bad_option_is_refused_before_any_request(wired, change, word):
    said = await tool.export_recent_actions("@group", **{**EVERY_OPTION, **change})

    assert word in said and wired.runs == [] and wired.client.requests == []
    assert wired.client.asked == []


@pytest.mark.asyncio
async def test_an_unknown_event_type_is_refused_before_the_form(wired):
    class Session:
        client_capabilities = SimpleNamespace(elicitation=object())

        async def elicit_form(self, *args, **kwargs):  # pragma: no cover - must not run
            raise AssertionError("the form was shown")

    said = await tool.export_recent_actions(
        "@group", event_types=["gossip"], ctx=SimpleNamespace(session=Session())
    )

    assert "gossip" in said and wired.runs == []


@pytest.mark.asyncio
async def test_a_group_only_checkbox_is_refused_for_a_channel(wired):
    said = await tool.export_recent_actions(
        "@news", event_types=["pinned_messages"], **EVERY_OPTION
    )

    assert "exist only in groups" in said and wired.runs == []


@pytest.mark.asyncio
async def test_a_user_has_no_recent_actions(wired):
    said = await tool.export_recent_actions("@bob", **EVERY_OPTION)

    assert "supergroups and channels" in said and wired.runs == []


@pytest.mark.asyncio
async def test_the_form_answer_decides(wired):
    class Session:
        client_capabilities = SimpleNamespace(elicitation=object())
        forms = []

        async def elicit_form(self, message, schema, related_request_id=None):
            self.forms.append(message)
            content = {**EVERY_OPTION, "format": "json", "videos": True}
            return SimpleNamespace(action="accept", content=content)

    await tool.export_recent_actions("@group", ctx=SimpleNamespace(session=Session()))

    await export_jobs.settle()
    (job,) = wired.runs
    assert "recent actions" in Session.forms[0].lower()
    assert job.settings.format is Format.Json
    assert job.settings.media.types == MediaSettings.Type.Photo | MediaSettings.Type.Video
