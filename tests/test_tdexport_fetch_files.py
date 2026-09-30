"""Media files and custom emoji of exported messages, against fake requests (no network).

Expectations follow Telegram Desktop v7.2.10's export_api_wrap.cpp: ComputeLocationKey,
LoadedFileCache, buildMessageFileWork, collectMessagesCustomEmoji, resolveCustomEmoji,
getCustomEmoji, processFileLoad (FilePolicy overload), writePreloadedFile, the loadFilePart
error handling and filePartRefreshReference with its filePartExtract*Reference handlers.
Synthetic data only; files land in tmp_path.
"""

import asyncio
import logging
from datetime import datetime, timezone

import pytest
from telethon import errors
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tdexport import fetch_files
from telegram_mcp.tdexport.fetch_files import (
    FileWork,
    LazyFile,
    LoadedFileCache,
    MessageFiles,
    compute_location_key,
    message_file_work,
    rpc_message,
    to_ulonglong,
)
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.model import (
    Document,
    File,
    FileLocation,
    Image,
    ParseMediaContext,
    Photo,
    TextPart,
    peer_from_user,
)
from telegram_mcp.tdexport.model_actions import ActionChatEditPhoto
from telegram_mcp.tdexport.model_media import Media, SharedContact
from telegram_mcp.tdexport.model_message import (
    FileOrigin,
    Message,
    MessagesSlice,
    Reaction,
    parse_messages_slice,
)
from telegram_mcp.tdexport.model_rich import RichBlock, RichMessage, RichText
from telegram_mcp.tdexport.settings import MediaSettings, Settings

WHEN = datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc)
Type = MediaSettings.Type
PEER = tl.InputPeerUser(user_id=100, access_hash=5)


def photo_location(photo_id=50, ref=b"old", thumb="y", dc_id=2):
    data = tl.InputPhotoFileLocation(
        id=photo_id, access_hash=1, file_reference=ref, thumb_size=thumb
    )
    return FileLocation(dc_id, data)


def document_location(document_id=3, ref=b"old", thumb=""):
    data = tl.InputDocumentFileLocation(
        id=document_id, access_hash=1, file_reference=ref, thumb_size=thumb
    )
    return FileLocation(2, data)


# Location keys, the loaded-file cache and the lazily created output file.


def test_location_keys():
    assert compute_location_key(document_location(7, thumb="m")) == (
        2 | (2 << 24) | (ord("m") << 16),
        7,
    )
    assert compute_location_key(photo_location(9, thumb="")) == (2 | (6 << 24), 9)
    assert compute_location_key(FileLocation(4, tl.InputTakeoutFileLocation())) == (
        4 | (5 << 24),
        0,
    )


def test_loaded_file_cache_skips_empty_locations_and_evicts_the_oldest():
    cache = LoadedFileCache(limit=1)
    cache.save(FileLocation(), "ignored")
    assert cache.find(FileLocation()) is None
    cache.save(photo_location(1), "photos/a.jpg")
    assert cache.find(photo_location(1, ref=b"other")) == "photos/a.jpg"
    cache.save(photo_location(2), "photos/b.jpg")
    assert cache.find(photo_location(1)) is None
    assert cache.find(photo_location(2)) == "photos/b.jpg"


def test_lazy_file_creates_its_folder_on_first_write_or_finish(tmp_path):
    empty = LazyFile(str(tmp_path / "a" / "empty.bin"))
    assert not (tmp_path / "a").exists()
    empty.finish()
    assert (tmp_path / "a" / "empty.bin").read_bytes() == b""
    target = LazyFile(str(tmp_path / "b" / "data.bin"))
    assert target.write(b"12") == 2 and target.write(b"3") == 1
    target.finish()
    target.close()
    assert (tmp_path / "b" / "data.bin").read_bytes() == b"123"


def test_to_ulonglong_and_rpc_message():
    assert (to_ulonglong("12"), to_ulonglong("x1"), to_ulonglong("")) == (12, 0, 0)
    assert rpc_message(errors.FileReferenceExpiredError(request=None)) == "FILE_REFERENCE_EXPIRED"
    assert rpc_message(errors.RPCError(None, "LOCATION_INVALID", 400)) == "LOCATION_INVALID"


# buildMessageFileWork.


def test_action_photo_comes_first_and_keeps_the_photo_type():
    edit = Photo(image=Image(file=File(size=11)))
    document = Document(is_video_file=True, file=File(size=99), thumb=Image(width=90))
    message = Message(media=Media(document))
    message.action.content = ActionChatEditPhoto(edit)
    assert message_file_work(message) == [
        FileWork(edit.image.file, Type.Photo, 11),
        FileWork(document.thumb.file, Type.Video, 11),
    ]


def test_contact_vcard_and_document_without_thumbnail():
    contact = SharedContact(vcard=File(size=3))
    assert message_file_work(Message(media=Media(contact))) == [
        FileWork(contact.vcard, Type.Photo, 3)
    ]
    voice = Document(is_voice_message=True, file=File(size=4))
    assert message_file_work(Message(media=Media(voice))) == [
        FileWork(voice.file, Type.VoiceMessage, 4)
    ]
    assert message_file_work(Message()) == []


def test_rich_files_are_appended_once_each():
    photo = Photo(id=1, image=Image(file=File(size=5)))
    sticker = Document(id=2, is_sticker=True, file=File(size=6), thumb=Image(width=1))
    blocks = [
        RichBlock(kind=RichBlock.Kind.Photo, photo_id=1),
        RichBlock(kind=RichBlock.Kind.Photo, photo_id=1),
        RichBlock(kind=RichBlock.Kind.File, document_id=2),
    ]
    rich = RichMessage(blocks=blocks, photos={1: photo}, documents={2: sticker})
    main = Photo(image=Image(file=File(size=1)))
    work = message_file_work(Message(media=Media(main), rich_message=rich))
    assert work == [
        FileWork(main.image.file, Type.Photo, 1),
        FileWork(photo.image.file, Type.Photo, 5, rich=True),
        FileWork(sticker.file, Type.Sticker, 6, rich=True),
        FileWork(sticker.thumb.file, Type.Sticker, 6, rich=True),
    ]


# The request side: a MessageFiles whose main/split requests answer from a script.


class Downloader:
    def __init__(self, outcomes):
        self.outcomes = list(outcomes)
        self.calls = []

    async def download_file(self, data, target, part_size_kb, file_size, dc_id):
        self.calls.append((data.file_reference, file_size, dc_id, part_size_kb))
        outcome = self.outcomes.pop(0)
        if isinstance(outcome, BaseException):
            raise outcome
        if outcome:
            target.write(outcome)


class Files(MessageFiles):
    def __init__(self, tmp_path, answers=(), downloads=()):
        self.client = Downloader(downloads)
        media = MediaSettings(types=Type.AllMask)
        self.settings = Settings(path=str(tmp_path).replace("\\", "/") + "/", media=media)
        self.self_id = 1
        self.file_cache = LoadedFileCache()
        self.resolved_emoji = {}
        self.unresolved_emoji = set()
        self.files = 0
        self.answers = list(answers)
        self.requests = []

    def _answer(self, request):
        self.requests.append(request)
        answer = self.answers.pop(0)
        if isinstance(answer, BaseException):
            raise answer
        return answer

    async def main(self, request):
        return self._answer(request)

    async def split(self, index, request):
        return self._answer((index, request))


def test_base_requests_are_abstract():
    base = MessageFiles()
    with pytest.raises(NotImplementedError):
        asyncio.run(base.main(None))
    with pytest.raises(NotImplementedError):
        asyncio.run(base.split(0, None))


def _emoji_text(emoji_id):
    return RichText(type=RichText.Type.CustomEmoji, id=emoji_id, custom_emoji_data=str(emoji_id))


def test_collect_custom_emoji_skips_known_zero_and_out_of_range_messages(tmp_path):
    files = Files(tmp_path)
    files.settings.single_peer_from = 10
    files.resolved_emoji[8] = Document()
    paragraph = RichBlock(kind=RichBlock.Kind.Paragraph, text=_emoji_text(7))
    inside = Message(
        date=20,
        text=[
            TextPart(type=TextPart.Type.CustomEmoji, additional="5"),
            TextPart(type=TextPart.Type.CustomEmoji, additional="8"),
            TextPart(type=TextPart.Type.CustomEmoji, additional="0"),
        ],
        reactions=[
            Reaction(type=Reaction.Type.CustomEmoji, document_id="6"),
            Reaction(type=Reaction.Type.Emoji, document_id="66"),
        ],
        rich_message=RichMessage(blocks=[paragraph]),
    )
    early = Message(date=5, text=[TextPart(type=TextPart.Type.CustomEmoji, additional="9")])
    files.collect_custom_emoji(MessagesSlice(list=[inside, early]))
    assert files.unresolved_emoji == {5, 6, 7}


def _emoji_document(document_id, ref=b"old"):
    return tl.Document(
        id=document_id,
        access_hash=1,
        file_reference=ref,
        date=WHEN,
        mime_type="image/webp",
        size=10,
        dc_id=2,
        attributes=[
            tl.DocumentAttributeSticker(alt="*", stickerset=tl.InputStickerSetEmpty()),
            tl.DocumentAttributeFilename(file_name="e.webp"),
        ],
    )


def test_resolve_custom_emoji_largest_first_with_failed_chunks(tmp_path, monkeypatch, caplog):
    monkeypatch.setattr(fetch_files, "_MAX_EMOJI_PER_REQUEST", 1)
    files = Files(tmp_path, answers=[[_emoji_document(7)], errors.RPCError(None, "X", 400)])
    files.unresolved_emoji = {5, 7}
    with caplog.at_level(logging.WARNING, logger=fetch_files.__name__):
        asyncio.run(files.resolve_custom_emoji(ParseMediaContext(), ""))
    assert [request.document_id for request in files.requests] == [[7], [5]]
    assert files.resolved_emoji[7].id == 7 and files.resolved_emoji[7].is_sticker
    assert files.resolved_emoji[5].file.skip_reason == SkipReason.Unavailable
    assert "Export Error: Failed to get documents for emoji." in caplog.text
    assert not files.unresolved_emoji


def test_custom_emoji_values_are_paths_or_placeholders(tmp_path):
    files = Files(tmp_path)
    ready = Document(
        id=3,
        is_sticker=True,
        file=File(
            content=b"webp",
            size=4,
            suggested_path="stickers/e.webp",
            location=document_location(3),
        ),
    )
    files.resolved_emoji = {
        3: ready,
        4: Document(file=File(skip_reason=SkipReason.FileType)),
        5: Document(file=File(skip_reason=SkipReason.Unavailable)),
    }

    def value(data):
        return asyncio.run(files.custom_emoji_value("", Message(), data))

    assert value("abc") == "abc" and value("0") == "0"
    assert value("9") == "(unavailable)"
    assert value("4") == "" and value("5") == "(unavailable)"
    assert value("3") == "stickers/e.webp"
    assert (tmp_path / "stickers" / "e.webp").read_bytes() == b"webp"
    # A second document with the same location reuses the written file.
    twin = Document(file=File(content=b"x", location=document_location(3)))
    files.resolved_emoji[6] = twin
    assert value("6") == "stickers/e.webp"


def test_message_custom_emoji_are_replaced_everywhere(tmp_path):
    files = Files(tmp_path)
    files.resolved_emoji = {
        3: Document(file=File(relative_path="stickers/e.webp")),
    }
    rich_emoji = _emoji_text(3)
    message = Message(
        text=[TextPart(type=TextPart.Type.CustomEmoji, additional="3"), TextPart(text="t")],
        reactions=[Reaction(type=Reaction.Type.CustomEmoji, document_id="9")],
        rich_message=RichMessage(
            blocks=[RichBlock(kind=RichBlock.Kind.Paragraph, text=rich_emoji)]
        ),
    )
    asyncio.run(files.resolve_message_custom_emoji("", message))
    assert message.text[0].additional == "stickers/e.webp"
    assert message.text[1].additional == ""
    assert message.reactions[0].document_id == "(unavailable)"
    assert rich_emoji.custom_emoji_data == "stickers/e.webp"
    plain = Message(text=[TextPart(type=TextPart.Type.CustomEmoji, additional="3")])
    asyncio.run(files.resolve_message_custom_emoji("", plain))
    assert plain.text[0].additional == "stickers/e.webp"


# loadFile and the FILE_REFERENCE refresh.


def _tl_photo(ref):
    return tl.Photo(
        id=50,
        access_hash=1,
        file_reference=ref,
        date=WHEN,
        sizes=[tl.PhotoSize(type="y", w=10, h=10, size=5)],
        dc_id=2,
    )


def _tl_message(ref, message_id=7, media=None):
    return tl.Message(
        id=message_id,
        peer_id=tl.PeerUser(user_id=100),
        date=WHEN,
        message="",
        media=media or tl.MessageMediaPhoto(photo=_tl_photo(ref)),
    )


def _messages(*messages):
    return tl.messages.Messages(messages=list(messages), topics=[], chats=[], users=[])


def _parsed(tl_message):
    context = ParseMediaContext(self_peer_id=peer_from_user(1))
    return parse_messages_slice(context, [tl_message], [], [], "").list[0]


def _load(files, message, origin=None):
    file = message.file()
    origin = origin or FileOrigin(split=0, peer=PEER, message_id=7)
    asyncio.run(files.process_file_load("", file, origin, message, Type.Photo, file.size))
    return file


def test_download_success_and_empty_download(tmp_path):
    files = Files(tmp_path, downloads=[b"jpeg", b""])
    message = _parsed(_tl_message(b"old"))
    file = _load(files, message)
    assert (tmp_path / file.relative_path).read_bytes() == b"jpeg"
    assert files.files == 1 and files.client.calls == [(b"old", 5, 2, 128)]
    assert files.file_cache.find(file.location) == file.relative_path
    other = _parsed(_tl_message(b"old", media=tl.MessageMediaPhoto(photo=_tl_photo(b"old"))))
    other.file().location = photo_location(51)
    empty = _load(files, other)
    assert empty.relative_path != file.relative_path
    assert (tmp_path / empty.relative_path).read_bytes() == b""


def test_unavailable_location_marks_the_file(tmp_path, caplog):
    files = Files(tmp_path, downloads=[errors.RPCError(None, "LOCATION_INVALID", 400)])
    with caplog.at_level(logging.WARNING, logger=fetch_files.__name__):
        file = _load(files, _parsed(_tl_message(b"old")))
    assert file.relative_path == "" and file.skip_reason == SkipReason.Unavailable
    assert "Export Error: File unavailable." in caplog.text and files.files == 0


def test_other_errors_propagate(tmp_path):
    files = Files(tmp_path, downloads=[errors.RPCError(None, "FLOOD_WAIT_X", 420)])
    with pytest.raises(errors.RPCError):
        _load(files, _parsed(_tl_message(b"old")))


def test_file_reference_is_refreshed_through_the_split_request(tmp_path):
    expired = errors.FileReferenceExpiredError(request=None)
    files = Files(tmp_path, answers=[_messages(_tl_message(b"new"))], downloads=[expired, b"ok"])
    file = _load(files, _parsed(_tl_message(b"old")))
    index, request = files.requests[0]
    assert index == 0 and isinstance(request, functions.messages.GetMessagesRequest)
    assert request.id[0].id == 7
    assert [call[0] for call in files.client.calls] == [b"old", b"new"]
    assert (tmp_path / file.relative_path).read_bytes() == b"ok"


def test_refresh_gives_up_when_the_message_is_gone_or_keeps_expiring(tmp_path):
    expired = errors.FileReferenceExpiredError(request=None)
    files = Files(tmp_path, answers=[_messages(_tl_message(b"new", 8))], downloads=[expired])
    assert _load(files, _parsed(_tl_message(b"old"))).skip_reason == SkipReason.Unavailable
    answers = [_messages(_tl_message(b"new"))] * 3
    files = Files(tmp_path, answers=answers, downloads=[expired] * 4)
    assert _load(files, _parsed(_tl_message(b"old"))).skip_reason == SkipReason.Unavailable
    assert len(files.client.calls) == 4 and len(files.requests) == 3


def _refresh(files, origin, location):
    return asyncio.run(files.refresh_reference("", origin, location))


def test_refresh_through_a_channel_request(tmp_path):
    for peer, channel in (
        (tl.InputPeerChannel(channel_id=9, access_hash=4), tl.InputChannel(9, 4)),
        (
            tl.InputPeerChannelFromMessage(peer=PEER, msg_id=3, channel_id=9),
            tl.InputChannelFromMessage(PEER, 3, 9),
        ),
    ):
        files = Files(tmp_path, answers=[_messages(_tl_message(b"new"))])
        origin = FileOrigin(peer=peer, message_id=7)
        refreshed = _refresh(files, origin, photo_location())
        assert refreshed.data.file_reference == b"new"
        request = files.requests[0]
        assert isinstance(request, functions.channels.GetMessagesRequest)
        assert request.channel == channel


def test_refresh_of_a_document_thumbnail(tmp_path):
    def document_message(ref):
        document = tl.Document(
            id=3,
            access_hash=1,
            file_reference=ref,
            date=WHEN,
            mime_type="application/pdf",
            size=100,
            dc_id=2,
            attributes=[],
            thumbs=[tl.PhotoSize(type="m", w=90, h=90, size=10)],
        )
        return _tl_message(ref, media=tl.MessageMediaDocument(document=document))

    stale = _parsed(document_message(b"old")).thumb().file.location
    assert stale.data.thumb_size == "m"
    files = Files(tmp_path, answers=[_messages(document_message(b"new"))])
    refreshed = _refresh(files, FileOrigin(peer=PEER, message_id=7), stale)
    assert refreshed.data.file_reference == b"new" and refreshed.data.thumb_size == "m"


def test_refresh_failures(tmp_path):
    files = Files(tmp_path, answers=[errors.RPCError(None, "CHANNEL_PRIVATE", 400)])
    assert _refresh(files, FileOrigin(peer=PEER, message_id=7), photo_location()) is None
    with pytest.raises(RuntimeError, match="FILE_REFERENCE error for non-message file."):
        _refresh(Files(tmp_path), FileOrigin(peer=PEER), photo_location())
    # A rich origin without a message is simply unavailable (tdesktop checks it first).
    assert _refresh(
        Files(tmp_path), FileOrigin(peer=PEER, rich_message=True), photo_location()
    ) is (None)


def test_refresh_from_the_full_rich_message(tmp_path):
    rich = tl.RichMessage(blocks=[], photos=[_tl_photo(b"new")], documents=[])
    full = tl.Message(id=7, peer_id=tl.PeerUser(user_id=100), rich_message=rich)
    files = Files(tmp_path, answers=[_messages(full)])
    origin = FileOrigin(peer=PEER, message_id=7, rich_message=True)
    refreshed = _refresh(files, origin, photo_location())
    assert refreshed.data.file_reference == b"new"
    request = files.requests[0]
    assert isinstance(request, functions.messages.GetRichMessageRequest)
    assert (request.peer, request.id) == (PEER, 7)
    files = Files(tmp_path, answers=[_messages()])
    assert _refresh(files, origin, photo_location()) is None


def test_refresh_of_a_custom_emoji(tmp_path):
    stale = document_location(3)
    files = Files(tmp_path, answers=[[_emoji_document(3, b"new")]])
    refreshed = _refresh(files, FileOrigin(custom_emoji_id=3), stale)
    assert refreshed.data.file_reference == b"new"
    assert files.requests[0].document_id == [3]
    twice = Files(tmp_path, answers=[[_emoji_document(3), _emoji_document(3)]])
    assert _refresh(twice, FileOrigin(custom_emoji_id=3), stale) is None
    other = Files(tmp_path, answers=[[_emoji_document(4)]])
    assert _refresh(other, FileOrigin(custom_emoji_id=3), stale) is None
    thumb = Files(tmp_path, answers=[[_emoji_document(3, b"new")]])
    assert _refresh(thumb, FileOrigin(custom_emoji_id=3), document_location(3, thumb="m")) is None
