"""A forward reports the ids of the copies it made (upstream chigwell #230, adapted).

Without them an agent could forward a message and then not pin, edit or reply to the
copy. Telethon's helper answers with Message objects; the raw ForwardMessagesRequest
(topic / send_as routing) answers with Updates whose UpdateMessageID pairs each new id
with the random_id it was sent under.
"""

from datetime import datetime, timezone
from types import SimpleNamespace

from telethon.tl import types as tl

from telegram_mcp.tools import messages_relay as mod

WHEN = datetime(2026, 9, 28, tzinfo=timezone.utc)


def test_the_helper_answer_gives_the_new_ids():
    assert mod._new_ids(SimpleNamespace(id=501)) == [501]
    assert mod._new_ids([SimpleNamespace(id=7), None, SimpleNamespace(id=8)]) == [7, 8]
    assert mod._new_ids(None) == []


def test_the_raw_answer_is_matched_by_random_id_in_send_order():
    updates = tl.Updates(
        updates=[
            tl.UpdateMessageID(id=902, random_id=22),
            tl.UpdateMessageID(id=901, random_id=11),
            tl.UpdateMessageID(id=999, random_id=77),  # not ours
        ],
        users=[],
        chats=[],
        date=WHEN,
        seq=0,
    )
    assert mod._new_ids(updates, random_ids=[11, 22]) == [901, 902]


def test_the_reply_names_the_new_ids():
    assert mod._with_ids("Message 5 forwarded from a to b.", [901]) == (
        "Message 5 forwarded from a to b. New message id: 901."
    )
    assert mod._with_ids("2 messages forwarded.", [7, 8]).endswith("New message ids: 7, 8.")
    assert mod._with_ids("Message 5 forwarded.", []) == "Message 5 forwarded."
