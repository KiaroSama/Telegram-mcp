"""What kiaro-telethon-secret-chat 0.2.0 added, as this server exposes it (spec 024).

* a received file stays saveable and forwardable after a restart, because its
  MediaReference is kept on disk - and dropped with the chat;
* `forward_secret_media` re-sends a received file without an upload;
* `send_secret_media` passes the media metadata the peer's client shows before download;
* `secret_chat_status(chat_id=...)` draws the key picture;
* `forget_secret_chat` drops a closed chat;
* a send Telegram has not confirmed yet is reported as "do not send it again".
"""

import json
from types import SimpleNamespace

import pytest
from telethon.utils import encode_waveform

from telegram_mcp import secret_backend, secret_history, secret_media_refs
from telegram_mcp.tools import secret_chats as sc
from telegram_mcp.tools import secret_messaging as sm

from secret_fakes import CHAT_ID, SECRET_ID, FakeChat
from test_secret_messaging import _writable

OTHER_SECRET = 627857999
OTHER_CHAT = OTHER_SECRET - 2_000_000_000_000


def _results(raw):
    return json.loads(raw)["results"]


class FakeReference:
    """Stands in for the package's MediaReference: a dict round trip, no key."""

    def __init__(self, data):
        self.data = data

    @classmethod
    def from_message(cls, message):
        if getattr(message, "media", None) is None:
            return None
        return cls({"version": 1, "file_id": message.random_id})

    @classmethod
    def from_dict(cls, data):
        return cls(dict(data))

    def to_dict(self):
        return dict(self.data)


@pytest.fixture
def refs(backend, monkeypatch):
    monkeypatch.setattr(secret_backend, "MediaReference", FakeReference)
    return backend


def _arrival(random_id=77, media=True, ttl=0):
    return SimpleNamespace(
        chat_id=SECRET_ID, random_id=random_id, media=object() if media else None, ttl=ttl, text=""
    )


# --- media references ------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_received_file_is_saveable_after_a_restart(refs, tmp_path, monkeypatch):
    await secret_backend._remember("acct")(_arrival(77, ttl=30))
    refs.history.clear()  # the package's memory is gone, as after a restart
    _writable(monkeypatch, tmp_path)

    answer = _results(await sm.save_secret_media(CHAT_ID, 77, account="acct"))

    assert answer["saved"] is True
    source, _path = refs.saved[-1]
    assert isinstance(source, FakeReference) and source.data["file_id"] == 77
    assert answer["sender_restriction_overridden"] is True, "the stored timer was lost"


@pytest.mark.asyncio
async def test_a_text_message_stores_no_reference(refs):
    await secret_backend._remember("acct")(_arrival(78, media=False))

    assert secret_media_refs.load("acct", SECRET_ID, 78) is None


@pytest.mark.asyncio
async def test_references_are_dropped_when_the_chat_closes(refs):
    await secret_backend._remember("acct")(_arrival(79))

    await secret_backend._closed("acct")(SimpleNamespace(chat_id=SECRET_ID, reason="peer"))

    assert secret_media_refs.load("acct", SECRET_ID, 79) is None


@pytest.mark.asyncio
async def test_a_file_never_received_here_is_still_explained(refs):
    answer = await sm.save_secret_media(CHAT_ID, 12345, account="acct")

    assert "no stored reference" in answer


# --- forwarding --------------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_received_file_is_forwarded_without_an_upload(refs):
    refs._chats[OTHER_SECRET] = FakeChat(OTHER_SECRET)
    refs.history[SECRET_ID] = [SimpleNamespace(random_id=80, media=object(), ttl=0, text="")]

    answer = _results(
        await sm.forward_secret_media(CHAT_ID, 80, OTHER_CHAT, caption="look", account="acct")
    )

    call = refs.forwarded[-1]
    assert call.chat_id == OTHER_SECRET and call.caption == "look"
    assert call.source.random_id == 80
    assert answer["forwarded"] is True and "original key" in answer["note"]
    kept = secret_history.read("acct", OTHER_SECRET, 5)
    assert kept[-1]["is_outgoing"] is True and kept[-1]["message_id"] == answer["message_id"]


@pytest.mark.asyncio
async def test_forwarding_a_message_without_a_file_is_refused(refs):
    refs.history[SECRET_ID] = [SimpleNamespace(random_id=81, media=None, ttl=0, text="hi")]

    answer = await sm.forward_secret_media(CHAT_ID, 81, CHAT_ID, account="acct")

    assert "carries no file" in answer and refs.forwarded == []


# --- media metadata ------------------------------------------------------------------


@pytest.fixture
def voice(tmp_path, monkeypatch):
    target = tmp_path / "note.ogg"
    target.write_bytes(b"OggS")

    async def _resolve(raw_path, ctx, tool_name):
        return target, None

    monkeypatch.setattr(sm, "_resolve_readable_file_path", _resolve)
    return target


@pytest.mark.asyncio
async def test_the_metadata_reaches_the_package(backend, voice):
    samples = [0, 31, 16, 8] * 25

    await sm.send_secret_media(
        CHAT_ID, str(voice), kind="voice_note", duration=3, waveform=samples, account="acct"
    )

    sent = backend.files[-1].metadata
    assert sent["duration"] == 3
    assert sent["waveform"] == encode_waveform(bytes(samples))


@pytest.mark.asyncio
async def test_a_waveform_sample_out_of_range_is_refused_before_upload(backend, voice):
    answer = await sm.send_secret_media(
        CHAT_ID, str(voice), kind="voice_note", waveform=[40], account="acct"
    )

    assert "0-31" in answer and backend.files == []


@pytest.mark.asyncio
async def test_unset_metadata_is_not_sent(backend, voice):
    await sm.send_secret_media(CHAT_ID, str(voice), kind="voice_note", account="acct")

    assert backend.files[-1].metadata == {}


# --- key picture ---------------------------------------------------------------------


@pytest.mark.asyncio
async def test_the_key_picture_is_drawn_with_its_hex(backend, tmp_path, monkeypatch):
    backend.status(SECRET_ID).key_hash = bytes(range(36))
    monkeypatch.setattr(sc, "key_picture_dir", lambda: tmp_path)

    answer = _results(await sc.secret_chat_status(chat_id=CHAT_ID, account="acct"))

    picture = answer["key_picture"]
    assert picture["hex"] == bytes(range(32)).hex()
    from PIL import Image

    with Image.open(picture["png"]) as image:
        assert image.size[0] == image.size[1] and image.size[0] % 12 == 0


@pytest.mark.asyncio
async def test_a_chat_without_a_key_hash_says_so(backend):
    answer = _results(await sc.secret_chat_status(chat_id=CHAT_ID, account="acct"))

    assert "no key picture" in answer["key_picture"]


# --- forget, pending sends -------------------------------------------------------------


@pytest.mark.asyncio
async def test_forgetting_a_closed_chat_drops_it_and_its_references(refs):
    await secret_backend._remember("acct")(_arrival(82))
    refs.status(SECRET_ID).state.value = "closed"

    answer = _results(await sc.forget_secret_chat(CHAT_ID, account="acct"))

    assert answer["forgotten"] is True and refs.forgotten == [SECRET_ID]
    assert secret_media_refs.load("acct", SECRET_ID, 82) is None


@pytest.mark.asyncio
async def test_an_open_chat_cannot_be_forgotten(backend):
    answer = await sc.forget_secret_chat(CHAT_ID, account="acct")

    assert "close_secret_chat" in answer and backend.forgotten == []


@pytest.mark.asyncio
async def test_an_unconfirmed_send_says_not_to_send_again(backend, monkeypatch):
    async def pending(*args, **kwargs):
        raise secret_backend.SendPending(chat_id=SECRET_ID, random_id=555, cause="FloodWait")

    monkeypatch.setattr(backend, "send_message", pending)

    answer = await sm.send_secret_message(CHAT_ID, "hi", account="acct")

    assert "do not send it again" in answer and "555" in answer


@pytest.mark.asyncio
async def test_a_closed_chat_says_its_key_was_discarded(backend):
    backend.status(SECRET_ID).state.value = "closed"

    answer = _results(await sc.secret_chat_status(chat_id=CHAT_ID, account="acct"))

    assert "key was discarded" in answer["key_picture"]


def test_key_pictures_go_where_every_save_goes():
    from telegram_mcp.safeguard import folders

    assert sc.key_picture_dir() == folders.default_download_dir()


def test_a_document_with_an_image_size_is_a_photo():
    """The package sends a photo as a document carrying DocumentAttributeImageSize,
    and Telegram's clients show it as a photo. Measured live 2026-09-30: the
    receiving side listed the package's own photo as a document."""
    size = type("DocumentAttributeImageSize", (), {})()
    name = type("DocumentAttributeFilename", (), {})()
    media = type("DecryptedMessageMediaDocument", (), {"attributes": [name, size]})()
    sticker = type("DocumentAttributeSticker", (), {})()
    as_sticker = type("DecryptedMessageMediaDocument", (), {"attributes": [size, sticker]})()

    assert secret_history.media_kind(media) == "photo"
    assert secret_history.media_kind(as_sticker) == "sticker"
