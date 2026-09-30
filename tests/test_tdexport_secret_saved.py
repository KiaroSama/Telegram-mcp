"""Secret chats exported from Telethon-Secret-Chat's saved records (spec 031 FR-003).

Records follow autosave.py format 1 at package commit 98c366e. Synthetic data only.
"""

from datetime import timezone

import pytest
from telethon.tl import types as tl

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.model import Document, Photo, TextPart
from telegram_mcp.tdexport.model_actions import (
    ActionHistoryClear,
    ActionScreenshotTaken,
    ActionSetMessagesTTL,
)
from telegram_mcp.tdexport.model_dialogs import DialogInfo
from telegram_mcp.tdexport.model_media import GeoPoint, SharedContact
from telegram_mcp.tdexport.secret_saved import export_saved_chat, parse_saved_messages
from telegram_mcp.tdexport.settings import MediaSettings, Settings

BASE = 1_700_000_000  # 14-11-2023 22:13:20 UTC
SELF = tl.User(id=1, is_self=True, first_name="Me")
PEER = tl.User(id=100, first_name="Peer", access_hash=7)


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def message(random_id, date, out=False, text="", **extra):
    record = {
        "v": 1,
        "type": "message",
        "id": random_id,
        "date": date,
        "out": out,
        "text": text,
        "entities": [],
        "ttl": 0,
        "reply_to": None,
        "media": None,
    }
    record.update(extra)
    return record


def service(date, action, out=False):
    return {
        "v": 1,
        "type": "service",
        "id": f"service-{date}",
        "date": date,
        "out": out,
        "action": action,
    }


def saved_file(tmp_path, name, size):
    path = tmp_path / "saved" / name
    path.parent.mkdir(exist_ok=True)
    path.write_bytes(b"s" * size)
    return str(path)


class RecordingWriter:
    def __init__(self):
        self.calls, self.slices = [], []

    def start(self, settings, environment):
        self.calls.append("start")
        self.settings = settings

    def write_dialogs_start(self, data):
        self.calls.append("dialogs_start")

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


def test_records_become_messages_with_ids_replies_and_actions():
    records = [
        message(
            -55,
            BASE,
            out=True,
            text="hi bold",
            entities=[
                {"_": "messageEntityCustomEmoji", "offset": 0, "length": 2, "document_id": 9},
                {"_": "messageEntityBold", "offset": 3, "length": 4},
                {"_": "somethingElse", "offset": 0, "length": 1},
            ],
        ),
        service(BASE + 1, {"_": "decryptedMessageActionSetMessageTTL", "ttl_seconds": 30}),
        service(BASE + 2, {"_": "decryptedMessageActionDeleteMessages", "random_ids": [-55]}),
        message(77, BASE + 3, text="re", reply_to=-55),
        message(78, BASE + 4, reply_to=12345),
        service(BASE + 5, {"_": "decryptedMessageActionScreenshotMessages", "random_ids": []}),
        service(BASE + 6, {"_": "decryptedMessageActionFlushHistory"}, out=True),
    ]
    messages = parse_saved_messages(records, SELF, PEER)
    # DeleteMessages has no Desktop service message: it is skipped, ids stay dense.
    assert [m.id for m in messages] == [1, 2, 3, 4, 5, 6]
    first = messages[0]
    assert (first.from_id, first.peer_id, first.self_id, first.out) == (1, 100, 1, True)
    assert [(p.type, p.text) for p in first.text] == [
        (TextPart.Type.CustomEmoji, "hi"),
        (TextPart.Type.Text, " "),
        (TextPart.Type.Bold, "bold"),
    ]
    assert first.text[0].additional == TextPart.unavailable_emoji()
    assert messages[1].action.content == ActionSetMessagesTTL(period=30)
    assert messages[2].from_id == 100 and messages[2].reply_to_msg_id == 1
    assert messages[3].reply_to_msg_id == 0
    assert isinstance(messages[4].action.content, ActionScreenshotTaken)
    assert isinstance(messages[5].action.content, ActionHistoryClear)
    assert messages[5].from_id == 1


def test_media_records_map_to_desktop_documents():
    video_attrs = [
        {"_": "documentAttributeVideo", "round_message": True, "duration": 3, "w": 240, "h": 240}
    ]
    records = [
        message(
            1,
            BASE,
            media={
                "_": "decryptedMessageMediaPhoto",
                "w": 800,
                "h": 600,
                "size": 50,
                "thumb": "",
                "caption": "cap",
            },
        ),
        message(
            2,
            BASE,
            media={
                "_": "decryptedMessageMediaDocument",
                "mime_type": "video/mp4",
                "size": 10,
                "attributes": video_attrs,
            },
        ),
        message(
            3,
            BASE,
            media={
                "_": "decryptedMessageMediaDocument",
                "mime_type": "audio/ogg",
                "size": 10,
                "attributes": [{"_": "documentAttributeAudio", "voice": True, "duration": 2}],
            },
        ),
        message(
            4,
            BASE,
            media={
                "_": "decryptedMessageMediaDocument",
                "mime_type": "image/webp",
                "size": 10,
                "attributes": [{"_": "documentAttributeSticker", "alt": "x"}],
            },
        ),
        message(
            5,
            BASE,
            media={
                "_": "decryptedMessageMediaDocument",
                "mime_type": "video/mp4",
                "size": 10,
                "attributes": [
                    {"_": "documentAttributeAnimated"},
                    {"_": "documentAttributeFilename", "file_name": "a:b.mp4"},
                ],
            },
        ),
        message(
            6,
            BASE,
            media={"_": "decryptedMessageMediaVideo", "duration": 4, "w": 1, "h": 1, "size": 10},
        ),
        message(7, BASE, media={"_": "decryptedMessageMediaGeoPoint", "lat": 1.5, "long": 2.5}),
        message(
            8,
            BASE,
            media={
                "_": "decryptedMessageMediaContact",
                "phone_number": "1",
                "first_name": "A",
                "last_name": "",
                "user_id": 5,
            },
        ),
        message(9, BASE, media={"_": "decryptedMessageMediaWebPage", "url": "https://t.me"}),
    ]
    messages = parse_saved_messages(records, SELF, PEER)
    photo = messages[0].media.content
    assert isinstance(photo, Photo) and (photo.image.width, photo.image.height) == (800, 600)
    assert photo.image.file.suggested_path == "photos/photo_1@14-11-2023_22-13-20.jpg"
    assert messages[0].text == [TextPart(text="cap")]
    paths = [m.media.content.file.suggested_path for m in messages[1:6]]
    assert paths == [
        "round_video_messages/file_1@14-11-2023_22-13-20.mp4",
        "voice_messages/audio_1@14-11-2023_22-13-20.ogg",
        "stickers/file_2@14-11-2023_22-13-20.webp",
        "animations/a_b.mp4",
        "video_files/video_1@14-11-2023_22-13-20.mp4",
    ]
    assert isinstance(messages[1].media.content, Document)
    assert messages[1].media.content.is_video_message and messages[2].media.content.duration == 2
    assert isinstance(messages[6].media.content, GeoPoint) and messages[6].media.content.valid
    assert isinstance(messages[7].media.content, SharedContact)
    assert messages[8].media.content is None


def test_export_copies_saved_files_and_applies_media_settings(tmp_path):
    photo = {"_": "decryptedMessageMediaPhoto", "w": 1, "h": 1, "size": 5}
    pdf = {
        "_": "decryptedMessageMediaDocument",
        "mime_type": "application/pdf",
        "size": 5,
        "attributes": [{"_": "documentAttributeFilename", "file_name": "r.pdf"}],
    }
    big = dict(photo, size=9 * 1024 * 1024)
    records = [
        message(1, BASE, media=photo, file=saved_file(tmp_path, "1-a.jpg", 5)),
        message(2, BASE + 1, media=photo),  # never downloaded
        message(3, BASE + 2, media=pdf, file=saved_file(tmp_path, "3-r.pdf", 5)),
        message(4, BASE + 3, media=pdf, file=saved_file(tmp_path, "4-r.pdf", 5)),
        message(5, BASE + 4, media=big, file=saved_file(tmp_path, "5-b.jpg", 5)),
    ]
    out = tmp_path / "out"
    writer = RecordingWriter()
    media = MediaSettings(types=MediaSettings.Type.Photo | MediaSettings.Type.File)
    result = export_saved_chat(records, Settings(path=str(out), media=media), writer, SELF, PEER)

    assert writer.calls == [
        "start",
        "dialogs_start",
        "dialog_start",
        "slice",
        "dialog_end",
        "dialogs_end",
        "finish",
    ]
    assert result.path == str(out).replace("\\", "/") + "/" and not result.takeout
    assert writer.settings.single_peer == tl.InputPeerUser(user_id=100, access_hash=7)
    assert writer.dialog.type == DialogInfo.Type.Personal and writer.dialog.relative_path == ""
    files = [m.file() for m in writer.slices[0].list]
    assert files[0].relative_path == "photos/photo_1@14-11-2023_22-13-20.jpg"
    assert files[1].skip_reason == SkipReason.Unavailable
    assert files[2].relative_path == "files/r.pdf"
    assert files[3].relative_path == "files/r (1).pdf"
    assert files[4].skip_reason == SkipReason.FileSize
    assert (out / "files" / "r (1).pdf").read_bytes() == b"s" * 5
    assert (result.messages, result.files) == (5, 3)
    assert set(writer.slices[0].peers) == {1, 100}

    writer = RecordingWriter()
    no_files = Settings(
        path=str(tmp_path / "o3"), media=MediaSettings(types=MediaSettings.Type.File)
    )
    export_saved_chat(records[:1], no_files, writer, SELF, PEER)
    assert writer.slices[0].list[0].file().skip_reason == SkipReason.FileType


def test_date_range_is_handed_to_the_writer(tmp_path):
    photo = {"_": "decryptedMessageMediaPhoto", "w": 1, "h": 1, "size": 5}
    records = [
        message(i, BASE + i * 100, media=photo, file=saved_file(tmp_path, f"{i}.jpg", 5))
        for i in (1, 2, 3)
    ]
    writer = RecordingWriter()
    settings = Settings(
        path=str(tmp_path / "out"), single_peer_from=BASE + 150, single_peer_till=BASE + 250
    )
    export_saved_chat(records, settings, writer, SELF, PEER)
    files = [m.file() for m in writer.slices[0].list]
    # Every message reaches the writer (it filters by date); only in-range files are copied.
    assert [f.relative_path for f in files] == ["", "photos/photo_2@14-11-2023_22-16-40.jpg", ""]
    assert files[0].skip_reason == SkipReason.None_

    writer = RecordingWriter()
    after_all = Settings(path=str(tmp_path / "late"), single_peer_from=BASE + 1000)
    result = export_saved_chat(records, after_all, writer, SELF, PEER)
    assert writer.slices == [] and writer.dialog.messages_count_per_split == [0]
    assert result.messages == 0
