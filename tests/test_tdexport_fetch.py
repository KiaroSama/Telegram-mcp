"""The single-chat export flow against a fake Telethon client (spec 030, stream A).

No network: the fake answers the TL requests tdesktop's ApiWrap sends and records them, so the
tests pin the takeout flags, the takeout/fallback choice, history order and slicing, the date
range, media selection and the tdesktop file naming rules. Synthetic data only.
"""

import asyncio
from datetime import date as Date
from datetime import timezone
from types import SimpleNamespace

import pytest
from telethon import errors
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tdexport import files as tdfiles
from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.fetch import export_single_chat
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.model import TextPart
from telegram_mcp.tdexport.settings import MediaSettings, Settings

SELF_ID = 1
PEER_ID = 100
DAY = 86_400
BASE = 1_700_000_000
PEER = tl.InputPeerUser(user_id=PEER_ID, access_hash=5)


@pytest.fixture(autouse=True)
def pinned(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)
    monkeypatch.setattr(model_format, "_COUNTRIES", [])

    class _Today(Date):
        @classmethod
        def today(cls):
            return Date(2026, 9, 30)

    monkeypatch.setattr(tdfiles, "Date", _Today)


def text_message(msg_id, when, **kwargs):
    return tl.Message(
        id=msg_id, peer_id=tl.PeerUser(user_id=PEER_ID), date=when, message=f"m{msg_id}", **kwargs
    )


def photo_message(msg_id, when, photo_id=None, size=100):
    photo = tl.Photo(
        id=photo_id or msg_id * 10,
        access_hash=1,
        file_reference=b"old",
        date=when,
        sizes=[tl.PhotoSize(type="y", w=10, h=10, size=size)],
        dc_id=2,
    )
    return text_message(msg_id, when, media=tl.MessageMediaPhoto(photo=photo))


def document_message(msg_id, when, doc_id, name=None, size=100, attributes=()):
    attrs = list(attributes) + ([tl.DocumentAttributeFilename(file_name=name)] if name else [])
    document = tl.Document(
        id=doc_id,
        access_hash=1,
        file_reference=b"old",
        date=when,
        mime_type="application/pdf",
        size=size,
        dc_id=2,
        attributes=attrs,
    )
    return text_message(msg_id, when, media=tl.MessageMediaDocument(document=document))


class FakeTakeout:
    def __init__(self, client, kwargs):
        self.client = client
        self.kwargs = kwargs

    async def __aenter__(self):
        self.client.takeout_kwargs = self.kwargs
        if self.client.takeout_error is not None:
            raise self.client.takeout_error
        return self

    async def __aexit__(self, exc_type, exc, tb):
        self.client.takeout_finished = exc_type is None
        return False

    async def __call__(self, request):
        self.client.wrapped.append(request)
        return await self.client.handle(request)


class FakeClient:
    """Answers the requests of ApiWrap's single-peer path from an in-memory history."""

    def __init__(self, history, takeout_error=None, splits=None):
        self.history = sorted(history, key=lambda m: -m.id)
        self.takeout_error = takeout_error
        self.splits = splits or [tl.MessageRange(min_id=1, max_id=2**31 - 1)]
        self.takeout_kwargs = None
        self.takeout_finished = None
        self.wrapped = []
        self.requests = []
        self.downloads = []
        self.download_errors = {}
        self.emoji_documents = []
        self.refetched = {}

    def takeout(self, finalize=True, **kwargs):
        assert finalize is True
        return FakeTakeout(self, kwargs)

    async def __call__(self, request):
        return await self.handle(request)

    async def handle(self, request):
        self.requests.append(request)
        if isinstance(request, functions.InvokeWithMessagesRangeRequest):
            return await self.handle(request.query)
        if isinstance(request, functions.users.GetUsersRequest):
            if isinstance(request.id[0], tl.InputUserSelf):
                return [tl.User(id=SELF_ID, is_self=True, first_name="Me")]
            return [tl.User(id=PEER_ID, first_name="Peer", access_hash=5)]
        if isinstance(request, functions.help.GetCountriesListRequest):
            raise errors.RPCError(request, "COUNTRIES_UNAVAILABLE", 400)
        if isinstance(request, functions.messages.GetSplitRangesRequest):
            return self.splits
        if isinstance(request, functions.messages.GetHistoryRequest):
            return self.history_slice(request.offset_id, request.add_offset, request.limit)
        if isinstance(request, functions.messages.GetCustomEmojiDocumentsRequest):
            return [d for d in self.emoji_documents if d.id in request.document_id]
        if isinstance(request, functions.messages.GetMessagesRequest):
            message = self.refetched[request.id[0].id]
            return tl.messages.Messages(messages=[message], topics=[], chats=[], users=[])
        raise AssertionError(f"unexpected request {type(request).__name__}")

    def history_slice(self, offset_id, add_offset, limit):
        ordered = self.history
        if offset_id:
            index = next((i for i, m in enumerate(ordered) if m.id < offset_id), len(ordered))
        else:
            index = 0
        start = index + add_offset
        end = start + limit
        chunk = ordered[max(start, 0) : max(end, 0)]
        users = [tl.User(id=PEER_ID, first_name="Peer")]
        return tl.messages.MessagesSlice(
            count=len(ordered), messages=chunk, topics=[], chats=[], users=users
        )

    async def download_file(self, location, file, *, part_size_kb, file_size, dc_id):
        assert part_size_kb == 128
        self.downloads.append((location.id, location.thumb_size, location.file_reference))
        error = self.download_errors.pop(location.id, None)
        if error is not None:
            raise error
        file.write(b"x" * (file_size or 1))


class RecordingWriter:
    def __init__(self):
        self.calls = []
        self.slices = []
        self.dialogs = None

    def start(self, settings, environment):
        self.calls.append("start")
        self.settings = settings

    def write_dialogs_start(self, data):
        self.calls.append("dialogs_start")
        self.dialogs = data

    def write_dialog_start(self, data):
        self.calls.append("dialog_start")
        self.dialog = data

    def write_dialog_slice(self, data):
        self.calls.append("slice")
        self.slices.append(data)

    def write_dialog_end(self):
        self.calls.append("dialog_end")

    def write_dialogs_end(self):
        self.calls.append("dialogs_end")

    def finish(self):
        self.calls.append("finish")

    def messages(self):
        return [m for s in self.slices for m in s.list]


def run(client, tmp_path, **settings_kwargs):
    settings = Settings(single_peer=PEER, path=str(tmp_path), **settings_kwargs)
    writer = RecordingWriter()
    result = asyncio.run(export_single_chat(client, settings, writer))
    return result, writer


def test_takeout_path_flags_order_and_writer_sequence(tmp_path):
    history = [text_message(i, BASE + i) for i in range(1, 251)]
    client = FakeClient(history, splits=[tl.MessageRange(min_id=1, max_id=2**31 - 1)])
    result, writer = run(client, tmp_path)
    assert result.takeout and result.takeout_error == ""
    assert client.takeout_kwargs == {
        "contacts": None,
        "users": True,
        "chats": True,
        "megagroups": True,
        "channels": True,
        "files": True,
        "max_file_size": 8 * 1024 * 1024,
    }
    assert client.takeout_finished is True
    assert writer.calls[:3] == ["start", "dialogs_start", "dialog_start"]
    assert writer.calls[-3:] == ["dialog_end", "dialogs_end", "finish"]
    assert [m.id for m in writer.messages()] == list(range(1, 251))
    assert [len(s.list) for s in writer.slices] == [100, 100, 50]
    history_requests = [
        r for r in client.wrapped if isinstance(r, functions.InvokeWithMessagesRangeRequest)
    ]
    assert history_requests and all(
        isinstance(r.query, functions.messages.GetHistoryRequest) for r in history_requests
    )
    # A single-peer export writes into the destination itself when it is empty.
    assert result.path == str(tmp_path).replace("\\", "/") + "/"
    assert writer.dialog.relative_path == "" and writer.dialog.name == "Peer"
    assert result.messages == 250


def test_takeout_refusal_falls_back_to_ordinary_requests(tmp_path):
    delay = errors.TakeoutInitDelayError(request=None, capture=3600)
    client = FakeClient([text_message(1, BASE)], takeout_error=delay)
    result, writer = run(client, tmp_path)
    assert not result.takeout and result.takeout_error == "TakeoutInitDelayError"
    assert client.takeout_finished is None and client.wrapped == []
    assert not any(
        isinstance(r, functions.InvokeWithMessagesRangeRequest) for r in client.requests
    )
    assert [m.id for m in writer.messages()] == [1]


def test_split_ranges_are_walked_oldest_first(tmp_path):
    history = [text_message(i, BASE + i) for i in (1, 2, 3)]
    splits = [tl.MessageRange(min_id=1, max_id=2), tl.MessageRange(min_id=2, max_id=2**31 - 1)]
    client = FakeClient(history, splits=splits)
    _, writer = run(client, tmp_path)
    assert writer.dialog.splits == [0, 1]
    ranges = [
        r.range for r in client.wrapped if isinstance(r, functions.InvokeWithMessagesRangeRequest)
    ]
    assert splits[0] in ranges and splits[1] in ranges


def test_date_range_leaves_outside_files_alone_but_counts_them(tmp_path):
    history = [
        photo_message(1, BASE),
        photo_message(2, BASE + DAY),
        photo_message(3, BASE + 3 * DAY),
    ]
    client = FakeClient(history)
    _, writer = run(
        client, tmp_path, single_peer_from=BASE + DAY // 2, single_peer_till=BASE + 2 * DAY
    )
    messages = {m.id: m for m in writer.messages()}
    # Out-of-range messages still reach the writer (it skips them) and still count photos, but
    # loadNextMessageFile never looks at their files.
    for skipped in (messages[1], messages[3]):
        assert skipped.file().skip_reason == SkipReason.None_
        assert skipped.file().relative_path == ""
    assert messages[2].file().relative_path == "photos/photo_2@15-11-2023_22-13-20.jpg"
    assert [d[0] for d in client.downloads] == [20]


def test_date_range_before_all_messages_exports_nothing(tmp_path):
    client = FakeClient([text_message(1, BASE)])
    _, writer = run(client, tmp_path, single_peer_from=BASE + DAY)
    assert writer.dialog.messages_count_per_split == [0]
    assert writer.slices == []


def test_media_types_size_limit_names_and_dedupe(tmp_path):
    video = [tl.DocumentAttributeVideo(duration=1, w=1, h=1)]
    history = [
        document_message(1, BASE, doc_id=500, name="report.pdf"),
        document_message(2, BASE, doc_id=501, name="report.pdf"),
        document_message(3, BASE, doc_id=500, name="report.pdf"),
        document_message(4, BASE, doc_id=502, size=10 * 1024 * 1024, attributes=video),
        document_message(5, BASE, doc_id=503, attributes=video),
    ]
    client = FakeClient(history)
    media = MediaSettings(types=MediaSettings.Type.File | MediaSettings.Type.Video)
    _, writer = run(client, tmp_path, media=media)
    files = {m.id: m.file() for m in writer.messages()}
    assert files[1].relative_path == "files/report.pdf"
    assert files[2].relative_path == "files/report (1).pdf"
    # The same document again comes from the loaded-file cache, not a second download.
    assert files[3].relative_path == "files/report.pdf"
    assert files[4].skip_reason == SkipReason.FileSize and files[4].relative_path == ""
    # The oversized video without a name took video_1 while parsing.
    assert files[5].relative_path == "video_files/video_2@14-11-2023_22-13-20.pdf"
    assert sorted(d[0] for d in client.downloads) == [500, 501, 503]
    assert (tmp_path / "files" / "report (1).pdf").read_bytes() == b"x" * 100


def test_file_type_not_selected_is_skipped(tmp_path):
    client = FakeClient([document_message(1, BASE, doc_id=9, name="a.pdf")])
    _, writer = run(client, tmp_path)
    assert writer.messages()[0].file().skip_reason == SkipReason.FileType
    assert client.downloads == []


def test_file_reference_refresh_and_unavailable_file(tmp_path):
    history = [photo_message(1, BASE), photo_message(2, BASE + 1)]
    client = FakeClient(history)
    client.download_errors[10] = errors.FileReferenceExpiredError(request=None)
    fresh = photo_message(1, BASE)
    fresh.media.photo.file_reference = b"new"
    client.refetched[1] = fresh
    client.download_errors[20] = errors.LocationInvalidError(request=None)
    _, writer = run(client, tmp_path)
    first, second = writer.messages()
    assert first.file().relative_path == "photos/photo_1@14-11-2023_22-13-20.jpg"
    assert [d for d in client.downloads if d[0] == 10] == [(10, "y", b"old"), (10, "y", b"new")]
    assert second.file().skip_reason == SkipReason.Unavailable
    assert not (tmp_path / "photos" / "photo_2@14-11-2023_22-13-21.jpg").exists()


def test_custom_emoji_are_resolved_to_files_or_placeholders(tmp_path):
    entities = [
        tl.MessageEntityCustomEmoji(offset=0, length=1, document_id=77),
        tl.MessageEntityCustomEmoji(offset=2, length=1, document_id=78),
    ]
    message = tl.Message(
        id=1, peer_id=tl.PeerUser(user_id=PEER_ID), date=BASE, message="a b", entities=entities
    )
    client = FakeClient([message])
    sticker = tl.DocumentAttributeCustomEmoji(alt="a", stickerset=tl.InputStickerSetEmpty())
    client.emoji_documents = [
        tl.Document(
            id=77,
            access_hash=1,
            file_reference=b"",
            date=BASE,
            mime_type="image/webp",
            size=10,
            dc_id=2,
            attributes=[sticker],
        )
    ]
    media = MediaSettings(types=MediaSettings.Type.Photo | MediaSettings.Type.Sticker)
    _, writer = run(client, tmp_path, media=media)
    parts = [p for p in writer.messages()[0].text if p.type == TextPart.Type.CustomEmoji]
    assert [p.additional for p in parts] == ["stickers/file_1.webp", "(unavailable)"]

    client = FakeClient([message])
    client.emoji_documents = []
    _, writer = run(client, tmp_path / "second")
    assert [p.additional for p in writer.messages()[0].text if p.additional] == [
        "(unavailable)",
        "(unavailable)",
    ]


def test_existing_destination_gets_a_chat_export_folder(tmp_path):
    (tmp_path / "keep.txt").write_text("x", encoding="utf-8")
    (tmp_path / "ChatExport_2026-09-30").mkdir()
    result, writer = run(FakeClient([photo_message(1, BASE)]), tmp_path)
    assert result.path.endswith("/ChatExport_2026-09-30 (1)/")
    assert writer.settings.path == result.path
    assert (tmp_path / "ChatExport_2026-09-30 (1)" / "photos").is_dir()


def test_supergroup_exports_migrated_chat_first(tmp_path):
    channel = tl.Channel(id=300, title="SG", photo=None, date=None, megagroup=True, access_hash=9)
    old_chat = tl.Chat(
        id=200,
        title="Old",
        photo=None,
        participants_count=1,
        date=None,
        version=1,
        migrated_to=tl.InputChannel(channel_id=300, access_hash=9),
    )
    old = [
        tl.Message(id=i, peer_id=tl.PeerChat(chat_id=200), date=BASE + i, message="o")
        for i in (1, 2)
    ]
    new = [
        tl.Message(id=i, peer_id=tl.PeerChannel(channel_id=300), date=BASE + 10 + i, message="n")
        for i in (1, 2)
    ]

    class GroupClient(FakeClient):
        async def handle(self, request):
            if isinstance(request, functions.channels.GetChannelsRequest):
                self.requests.append(request)
                return tl.messages.Chats(chats=[channel])
            if isinstance(request, functions.channels.GetFullChannelRequest):
                self.requests.append(request)
                full = SimpleNamespace(migrated_from_chat_id=200)
                return tl.messages.ChatFull(full_chat=full, chats=[old_chat], users=[])
            if isinstance(request, functions.messages.GetHistoryRequest):
                self.history = sorted(
                    old if isinstance(request.peer, tl.InputPeerChat) else new, key=lambda m: -m.id
                )
            return await super().handle(request)

    client = GroupClient([], takeout_error=ValueError("takeout already open"))
    settings = Settings(
        single_peer=tl.InputPeerChannel(channel_id=300, access_hash=9), path=str(tmp_path)
    )
    writer = RecordingWriter()
    result = asyncio.run(export_single_chat(client, settings, writer))
    assert result.takeout_error == "ValueError"
    assert writer.dialog.splits == [-1, 0]
    assert writer.dialog.migrated_from_input == tl.InputPeerChat(chat_id=200)
    shift = -1_000_000_000
    assert [m.id for m in writer.messages()] == [1 + shift, 2 + shift, 1, 2]
