"""The six things you do TO a secret chat, on the replacement backend.

Two of the six had no direct equivalent in the package and are composed here, so
they are where a silent behaviour change would hide:

* **search** runs locally, because the encrypted protocol has no search call and
  never did - the previous backend's dedicated one was that client searching its
  own database;
* **copy in** goes down and up again, because the file's key is made per chat:
  the original's bytes sit under a key this chat cannot use, so a copy is a
  genuine re-send rather than a pointer.

And one behaviour is easy to get wrong in both: a delete that left the text in
this server's own record would be a delete in name only.
"""

import json
from types import SimpleNamespace

import pytest

from telegram_mcp import secret_history
from telegram_mcp.tools import secret_actions as sa

from secret_fakes import CHAT_ID, SECRET_ID


def _results(raw):
    return json.loads(raw)["results"]


def _seed(*entries):
    for item in entries:
        secret_history.record("acct", SECRET_ID, item)


def _incoming(message_id, text="", date=None):
    item = secret_history.entry(message_id=message_id, is_outgoing=False, text=text)
    if date is not None:
        item["date"] = date
    return item


# --- deleting ------------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_delete_reaches_both_sides_and_says_so(backend):
    _seed(_incoming(11, "gone"))

    answer = _results(await sa.delete_secret_message(CHAT_ID, 11, account="acct"))

    assert backend.deleted == [(SECRET_ID, [11])]
    assert "no delete-for-me-only" in answer["reached"]


@pytest.mark.asyncio
async def test_a_deleted_message_leaves_this_server_s_record_too(backend):
    _seed(_incoming(11, "gone"), _incoming(12, "stays"))

    await sa.delete_secret_message(CHAT_ID, 11, account="acct")

    left = [m["message_id"] for m in secret_history.read("acct", SECRET_ID, 10)]
    assert left == [12], "the text survived a delete that reached both devices"


@pytest.mark.asyncio
async def test_deleting_in_a_chat_that_is_not_ready_is_refused(backend):
    backend.status(SECRET_ID).state.value = "pending"

    assert "still pending" in await sa.delete_secret_message(CHAT_ID, 11, account="acct")
    assert backend.deleted == []


# --- clearing ------------------------------------------------------------------


@pytest.mark.asyncio
async def test_clearing_needs_the_chat_named_twice(backend):
    answer = await sa.clear_secret_history(CHAT_ID, CHAT_ID + 1, account="acct")

    assert "Refusing to clear anything" in answer
    assert backend.flushed == [], "a mistyped id emptied a conversation"


@pytest.mark.asyncio
async def test_clearing_empties_both_sides_and_the_local_record(backend):
    _seed(_incoming(1), _incoming(2), _incoming(3))

    answer = _results(await sa.clear_secret_history(CHAT_ID, CHAT_ID, account="acct"))

    assert backend.flushed == [SECRET_ID]
    assert answer["messages_removed_here"] == 3
    assert secret_history.read("acct", SECRET_ID, 10) == []


# --- marking read --------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_receipt_covers_everything_at_or_before_that_moment(backend):
    """The published field is `read_up_to_date` rather than one id, because saying
    "message 3 is read" would be false about the two before it."""
    _seed(_incoming(1, date=100), _incoming(2, date=200), _incoming(3, date=300))

    answer = _results(await sa.mark_secret_read(CHAT_ID, 2, account="acct"))

    assert backend.read == [(SECRET_ID, [1, 2])]
    assert answer["read_up_to_message_id"] == 2
    assert answer["read_up_to_date"] == 200
    assert answer["messages_acknowledged"] == 2


@pytest.mark.asyncio
async def test_no_message_named_means_the_newest_one(backend):
    _seed(_incoming(1, date=100), _incoming(2, date=200))

    answer = _results(await sa.mark_secret_read(CHAT_ID, account="acct"))

    assert answer["read_up_to_message_id"] == 2


@pytest.mark.asyncio
async def test_your_own_messages_are_not_acknowledged(backend):
    """A receipt for something you sent starts a countdown on your own copy and
    tells the peer nothing."""
    secret_history.record(
        "acct", SECRET_ID, secret_history.entry(message_id=9, is_outgoing=True, text="mine")
    )

    answer = _results(await sa.mark_secret_read(CHAT_ID, account="acct"))

    assert answer["marked"] is False
    assert "no incoming messages" in answer["reason"]


@pytest.mark.asyncio
async def test_a_message_this_device_never_received_is_named(backend):
    _seed(_incoming(1, date=100))

    answer = _results(await sa.mark_secret_read(CHAT_ID, 404, account="acct"))

    assert answer["marked"] is False and "not among the messages" in answer["reason"]


# --- typing --------------------------------------------------------------------


@pytest.mark.asyncio
async def test_each_indicator_maps_to_its_own_protocol_action(backend):
    for name, expected in (
        ("typing", "SendMessageTypingAction"),
        ("recording_voice", "SendMessageRecordAudioAction"),
        ("recording_video", "SendMessageRecordRoundAction"),
        ("uploading_photo", "SendMessageUploadPhotoAction"),
        ("cancel", "SendMessageCancelAction"),
    ):
        await sa.send_secret_typing(CHAT_ID, name, account="acct")
        assert backend.typing[-1] == (SECRET_ID, expected), name


@pytest.mark.asyncio
async def test_an_indicator_telegram_does_not_draw_is_refused(backend):
    answer = await sa.send_secret_typing(CHAT_ID, "juggling", account="acct")

    assert "not an indicator Telegram draws" in answer
    assert backend.typing == []


# --- searching -----------------------------------------------------------------


@pytest.mark.asyncio
async def test_search_matches_text_and_captions_case_insensitively(backend):
    _seed(_incoming(1, "The Invoice is attached"), _incoming(2, "unrelated"))
    secret_history.record(
        "acct",
        SECRET_ID,
        secret_history.entry(message_id=3, is_outgoing=True, text="invoice copy", kind="photo"),
    )

    answer = _results(await sa.search_secret_messages(CHAT_ID, "invoice", account="acct"))

    assert [m["message_id"] for m in answer["messages"]] == [1, 3]
    assert answer["total_count"] == 2


@pytest.mark.asyncio
async def test_an_empty_search_says_a_gap_is_permanent(backend):
    """ "No results" must not read as "never sent": a secret chat has no
    server-side history, so what this login missed is simply not here."""
    _seed(_incoming(1, "nothing relevant"))

    answer = await sa.search_secret_messages(CHAT_ID, "missing", account="acct")

    assert "no server-side history" in answer


@pytest.mark.asyncio
async def test_a_closed_chat_is_still_searchable(backend):
    """Its local copy is the only one that exists, so refusing to read it to
    satisfy a rule about sending would be the wrong trade."""
    backend.status(SECRET_ID).state.value = "closed"
    _seed(_incoming(1, "still here"))

    answer = _results(await sa.search_secret_messages(CHAT_ID, "still", account="acct"))

    assert answer["total_count"] == 1


# --- copying in ----------------------------------------------------------------


@pytest.mark.asyncio
async def test_a_text_message_crosses_and_arrives_unattributed(backend, monkeypatch):
    _wire_source(monkeypatch, SimpleNamespace(message="quoted", media=None, entities=None))

    answer = _results(await sa.copy_into_secret_chat(-100, 5, CHAT_ID, account="acct"))

    assert backend.sent[-1].text == "quoted"
    assert "arrives as though you wrote it" in answer["attribution"]
    assert secret_history.read("acct", SECRET_ID, 10)[-1]["is_outgoing"] is True


@pytest.mark.asyncio
async def test_media_is_fetched_and_re_sent_under_this_chat_s_own_key(backend, monkeypatch):
    _wire_source(
        monkeypatch,
        SimpleNamespace(
            message="caption",
            media=object(),
            entities=None,
            file=SimpleNamespace(name="p.jpg", ext=".jpg"),
        ),
        downloaded=b"\xff\xd8\xff",
    )

    answer = _results(await sa.copy_into_secret_chat(-100, 5, CHAT_ID, account="acct"))

    assert backend.files[-1].kind == "photo" and backend.files[-1].caption == "caption"
    assert answer["kind"] == "photo"


@pytest.mark.asyncio
async def test_a_message_with_neither_text_nor_media_names_what_cannot_cross(backend, monkeypatch):
    _wire_source(monkeypatch, SimpleNamespace(message="", media=None, entities=None))

    answer = await sa.copy_into_secret_chat(-100, 5, CHAT_ID, account="acct")

    assert "no encrypted form" in answer
    assert backend.sent == []


@pytest.mark.asyncio
async def test_a_message_this_account_cannot_see_is_named(backend, monkeypatch):
    _wire_source(monkeypatch, None)

    assert "cannot see it" in await sa.copy_into_secret_chat(-100, 5, CHAT_ID, account="acct")


def _wire_source(monkeypatch, message, downloaded=b""):
    class _Client:
        async def get_messages(self, chat, ids=None):
            return message

        async def download_media(self, source, file=None):
            return downloaded

    monkeypatch.setattr(sa, "get_client", lambda account=None: _Client())

    async def _connected(client):
        return None

    monkeypatch.setattr(sa, "ensure_connected", _connected)
