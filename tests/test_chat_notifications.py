"""A chat's notifications (spec 008, US3), on a fake client.

What these pin: every change reads the chat's current settings and writes them back
with ONE field changed, so turning the sound off never unmutes a chat and choosing a
tone never resets a mute. Telegram does not promise to keep a field a partial update
leaves out, so nothing is left out.
"""

from datetime import datetime, timedelta, timezone

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import chat_notifications as mod
from telegram_mcp.tools import chat_state as state_mod

FRIEND = types.User(id=42, first_name="Sara", access_hash=4)
FAR_FUTURE = 2**31 - 1


def _doc(doc_id, name, seconds=2):
    return types.Document(
        id=doc_id,
        access_hash=doc_id + 1,
        file_reference=b"r",
        date=None,
        mime_type="audio/mpeg",
        size=100,
        dc_id=1,
        attributes=[
            types.DocumentAttributeFilename(file_name=name),
            types.DocumentAttributeAudio(duration=seconds),
        ],
    )


SAVED = [_doc(11, "chime.mp3"), _doc(22, "bell.ogg", 3)]


class FakeClient:
    def __init__(self):
        self.sent = []
        self.settings = types.PeerNotifySettings(
            show_previews=True,
            silent=False,
            mute_until=datetime(2030, 1, 1, tzinfo=timezone.utc),
            other_sound=types.NotificationSoundDefault(),
        )
        self.answers = {}

    async def upload_file(self, handle):
        return types.InputFile(id=1, parts=1, name="t.mp3", md5_checksum="")

    async def __call__(self, request):
        if isinstance(request, functions.account.GetNotifySettingsRequest):
            return self.settings
        if isinstance(request, functions.account.GetSavedRingtonesRequest):
            return types.account.SavedRingtones(hash=0, ringtones=list(SAVED))
        self.sent.append(request)
        return self.answers.get(type(request), True)


@pytest.fixture
def client(monkeypatch):
    fake = FakeClient()

    async def _resolve(value, cl=None, account=None):
        return FRIEND

    for module in (mod, state_mod):
        monkeypatch.setattr(module, "get_client", lambda account=None: fake)
        monkeypatch.setattr(module, "resolve_entity", _resolve)
    return fake


def _written(client):
    updates = [
        r for r in client.sent if isinstance(r, functions.account.UpdateNotifySettingsRequest)
    ]
    assert len(updates) == 1
    assert updates[0].peer.peer.user_id == 42
    return updates[0].settings


# --- one field at a time ---------------------------------------------------------------


@pytest.mark.asyncio
async def test_turning_the_sound_off_keeps_the_mute_and_the_tone(client):
    await mod.set_chat_sound_disabled(chat="@sara_k", disabled=True)
    s = _written(client)
    assert s.silent is True
    assert s.mute_until == client.settings.mute_until, "the mute was reset"
    assert isinstance(s.sound, types.NotificationSoundDefault)
    assert s.show_previews is True


@pytest.mark.asyncio
async def test_a_timed_mute_ends_after_the_given_duration(client):
    before = datetime.now(timezone.utc)
    await state_mod.mute_chat(chat_id="@sara_k", days=1, hours=8, minutes=5)
    s = _written(client)
    expected = before + timedelta(days=1, hours=8, minutes=5)
    assert abs((s.mute_until - expected).total_seconds()) < 5
    assert s.silent is False, "muting changed the sound setting"


@pytest.mark.asyncio
async def test_no_duration_mutes_forever(client):
    await state_mod.mute_chat(chat_id="@sara_k")
    assert _written(client).mute_until == FAR_FUTURE


@pytest.mark.parametrize(
    "kwargs", [{"hours": -1}, {"minutes": 1.5}, {"days": True}, {"days": "2"}]
)
@pytest.mark.asyncio
async def test_a_bad_duration_is_refused_before_any_request(client, kwargs):
    text = await state_mod.mute_chat(chat_id="@sara_k", **kwargs)
    assert "whole" in text and client.sent == []


@pytest.mark.asyncio
async def test_unmute_keeps_the_sound_setting(client):
    client.settings.silent = True
    await state_mod.unmute_chat(chat_id="@sara_k")
    s = _written(client)
    assert s.mute_until == 0 and s.silent is True


# --- tones -----------------------------------------------------------------------------


@pytest.mark.parametrize(
    "tone,expected",
    [("default", types.NotificationSoundDefault), ("none", types.NotificationSoundNone)],
)
@pytest.mark.asyncio
async def test_tone_default_and_none(client, tone, expected):
    await mod.set_chat_tone(chat="@sara_k", tone=tone)
    s = _written(client)
    assert isinstance(s.sound, expected)
    assert s.mute_until == client.settings.mute_until


@pytest.mark.parametrize("tone", ["22", "BELL.ogg", "bell"])
@pytest.mark.asyncio
async def test_a_saved_sound_is_found_by_id_or_title(client, tone):
    await mod.set_chat_tone(chat="@sara_k", tone=tone)
    assert _written(client).sound == types.NotificationSoundRingtone(id=22)


@pytest.mark.asyncio
async def test_an_unknown_saved_sound_lists_the_real_ones(client):
    text = await mod.set_chat_tone(chat="@sara_k", tone="trumpet")
    assert "chime.mp3" in text and "bell.ogg" in text and client.sent == []


@pytest.mark.parametrize("kwargs", [{}, {"tone": "none", "file_path": "files/t.mp3"}])
@pytest.mark.asyncio
async def test_exactly_one_of_tone_and_file(client, kwargs):
    text = await mod.set_chat_tone(chat="@sara_k", **kwargs)
    assert "exactly one" in text and client.sent == []


@pytest.mark.asyncio
async def test_a_new_file_is_uploaded_saved_and_used(client, monkeypatch):
    uploaded = _doc(33, "t.mp3")
    client.answers[functions.account.UploadRingtoneRequest] = uploaded
    client.answers[functions.account.SaveRingtoneRequest] = types.account.SavedRingtoneConverted(
        document=_doc(44, "t.mp3")
    )

    class _Source:
        path = "G:/x/files/t.mp3"
        handle = None

    class _Opened:
        async def __aenter__(self):
            return _Source(), None

        async def __aexit__(self, *exc):
            return False

    monkeypatch.setattr(mod, "_open_verified_source", lambda **kw: _Opened())
    await mod.set_chat_tone(chat="@sara_k", file_path="G:/x/files/t.mp3")
    upload, save, update = client.sent
    assert upload.file_name == "t.mp3" and upload.mime_type == "audio/mpeg"
    assert save.id.id == 33 and save.unsave is False
    assert update.settings.sound == types.NotificationSoundRingtone(id=44), "the converted id"


@pytest.mark.asyncio
async def test_a_file_the_folder_rule_refuses_sends_nothing(client, monkeypatch):
    class _Refused:
        async def __aenter__(self):
            return None, "refused by the folder rule"

        async def __aexit__(self, *exc):
            return False

    monkeypatch.setattr(mod, "_open_verified_source", lambda **kw: _Refused())
    text = await mod.set_chat_tone(chat="@sara_k", file_path="C:/t.mp3")
    assert text == "refused by the folder rule" and client.sent == []


@pytest.mark.asyncio
async def test_an_unsupported_format_is_refused_before_upload(client):
    text = await mod.set_chat_tone(chat="@sara_k", file_path="files/t.wav")
    assert "MP3" in text and client.sent == []


# --- saved sounds ----------------------------------------------------------------------


@pytest.mark.asyncio
async def test_saved_sounds_are_listed_with_id_title_and_length(client):
    text = await mod.list_saved_sounds()
    assert "11" in text and "chime.mp3" in text and "3 s" in text


@pytest.mark.asyncio
async def test_removing_a_saved_sound_unsaves_it(client):
    await mod.remove_saved_sound(sound="chime")
    request = client.sent[0]
    assert isinstance(request, functions.account.SaveRingtoneRequest)
    assert request.id.id == 11 and request.unsave is True
