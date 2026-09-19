"""The four protocol actions, search, and copying in -- what each one really does.

Every tool here does something an ordinary chat also does, and every one of them
DIFFERS in a way a caller carrying over ordinary-chat habits gets wrong. The
tests are written against those differences rather than against the happy path,
because the happy path is the part that would survive any implementation:

- deletion always reaches both sides, so there is no choice to offer and the
  result must not imply one was made;
- clearing is irreversible and both-sided, so it takes the target twice;
- reading is addressed by DATE, so marking one message read marks everything
  before it, and the result says which moment rather than which message;
- searching reads this device's only copy, so an empty result is not evidence
  that nothing was ever sent;
- copying in is a copy, and whether it is even allowed is Telegram's verdict
  rather than ours.

No network, no TDLib: the client is a fake that records what would be sent.
"""

import json

import pytest

from telegram_mcp.tdlib import TDLibError
from telegram_mcp.tools import secret_actions as sa

_READY_CHAT = {"@type": "chat", "type": {"@type": "chatTypeSecret", "secret_chat_id": 7}}
_READY_SECRET = {"@type": "secretChat", "state": {"@type": "secretChatStateReady"}, "layer": 144}


class FakeTDLib:
    def __init__(self, answers=None):
        self.requests = []
        self.answers = {"getChat": _READY_CHAT, "getSecretChat": _READY_SECRET}
        self.answers.update(answers or {})

    async def request(self, obj, timeout=30.0):
        self.requests.append(obj)
        answer = self.answers.get(obj["@type"])
        if isinstance(answer, Exception):
            raise answer
        return answer if answer is not None else {"@type": "ok"}

    def types(self):
        return [r["@type"] for r in self.requests]

    def sent(self, type_name):
        found = [r for r in self.requests if r["@type"] == type_name]
        assert len(found) == 1, f"expected exactly one {type_name}, got {len(found)}"
        return found[0]


@pytest.fixture
def wire(monkeypatch):
    def _wire(answers=None):
        client = FakeTDLib(answers)
        monkeypatch.setattr(sa, "_account_label", lambda account=None: "acct")

        async def _client(label):
            return client

        monkeypatch.setattr(sa, "secret_client", _client)
        return client

    return _wire


def _payload(raw):
    return json.loads(raw)["results"]


# --------------------------------------------------------------------------
# Deleting
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_deleting_says_it_reached_both_sides_because_it_always_does(wire):
    """A caller used to ordinary chats will assume `revoke` was a choice someone
    made. In a secret chat the protocol has only one delete, so the result must
    state that rather than let the assumption stand."""
    client = wire()

    result = _payload(await sa.delete_secret_message(chat_id=1, message_id=5, account="acct"))

    sent = client.sent("deleteMessages")
    assert sent["message_ids"] == [5]
    assert sent["revoke"] is True
    assert "both" in json.dumps(result).lower()


@pytest.mark.asyncio
async def test_deleting_offers_no_bulk_form(wire):
    """There is deliberately no message_ids argument. An irreversible action
    reaching both devices takes one named target, the way terminate_authorization
    does."""
    import inspect

    parameters = inspect.signature(sa.delete_secret_message).parameters
    assert "message_ids" not in parameters
    assert "message_id" in parameters


# --------------------------------------------------------------------------
# Clearing
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_clearing_refuses_and_sends_nothing_when_the_confirmation_differs(wire):
    """The whole history, on someone else's device, unrecoverable. A mistyped id
    must cost nothing at all -- so the check happens before any request goes."""
    client = wire()

    answer = await sa.clear_secret_history(chat_id=1, confirm_chat_id=2, account="acct")

    assert "deleteChatHistory" not in client.types(), "cleared a chat on a mismatched id"
    assert "1" in answer and "2" in answer


@pytest.mark.asyncio
async def test_clearing_with_a_matching_confirmation_goes_through(wire):
    client = wire()

    result = _payload(await sa.clear_secret_history(chat_id=1, confirm_chat_id=1, account="acct"))

    sent = client.sent("deleteChatHistory")
    assert sent["chat_id"] == 1
    assert sent["revoke"] is True
    assert result["cleared"] is True


@pytest.mark.asyncio
async def test_clearing_does_not_remove_the_chat_from_the_list(wire):
    """Clearing the history and closing the chat are different acts, and only one
    of them was asked for."""
    client = wire()

    await sa.clear_secret_history(chat_id=1, confirm_chat_id=1, account="acct")

    assert client.sent("deleteChatHistory")["remove_from_chat_list"] is False


# --------------------------------------------------------------------------
# Marking read
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_marking_read_reports_the_moment_not_the_message(wire):
    """TDLib sends a DATE to the other side, so everything at or before that
    timestamp becomes read. Reporting 'message 5 is read' would be false."""
    wire({"getMessage": {"@type": "message", "id": 5, "date": 1700000000}})

    result = _payload(await sa.mark_secret_read(chat_id=1, message_id=5, account="acct"))

    assert result["read_up_to_date"] == 1700000000
    assert "before" in json.dumps(result).lower()


@pytest.mark.asyncio
async def test_marking_read_sends_nothing_when_no_date_can_be_found(wire):
    """TDLib's own code logs an error and sends nothing when it has no date.
    Reporting success for that would be a read receipt the other side never got."""
    client = wire({"getMessage": {"@type": "message", "id": 5, "date": 0}})

    result = _payload(await sa.mark_secret_read(chat_id=1, message_id=5, account="acct"))

    assert result["marked"] is False
    assert "viewMessages" not in client.types()


@pytest.mark.asyncio
async def test_marking_read_without_a_message_uses_the_newest_one(wire):
    client = wire(
        {
            "getChatHistory": {
                "@type": "messages",
                "messages": [{"@type": "message", "id": 9, "date": 1700000009}],
            }
        }
    )

    result = _payload(await sa.mark_secret_read(chat_id=1, account="acct"))

    assert result["read_up_to_date"] == 1700000009
    assert client.sent("viewMessages")["message_ids"] == [9]


@pytest.mark.asyncio
async def test_marking_read_on_an_empty_chat_says_so(wire):
    client = wire({"getChatHistory": {"@type": "messages", "messages": []}})

    result = _payload(await sa.mark_secret_read(chat_id=1, account="acct"))

    assert result["marked"] is False
    assert "viewMessages" not in client.types()


# --------------------------------------------------------------------------
# Typing
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_typing_sends_the_action_telegram_names(wire):
    client = wire()

    await sa.send_secret_typing(chat_id=1, action="recording_voice", account="acct")

    assert client.sent("sendChatAction")["action"]["@type"] == "chatActionRecordingVoiceNote"


@pytest.mark.asyncio
async def test_an_action_outside_the_seven_is_refused_by_name(wire):
    client = wire()

    answer = await sa.send_secret_typing(chat_id=1, action="juggling", account="acct")

    assert "juggling" in answer
    assert "typing" in answer, "the refusal did not list what does work"
    assert "sendChatAction" not in client.types()


@pytest.mark.asyncio
async def test_typing_can_be_cancelled(wire):
    client = wire()

    await sa.send_secret_typing(chat_id=1, action="cancel", account="acct")

    assert client.sent("sendChatAction")["action"]["@type"] == "chatActionCancel"


# --------------------------------------------------------------------------
# Searching
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_search_uses_the_dedicated_secret_call(wire):
    """`searchChatMessages` answers a secret chat with an error - the ordinary
    search path is not merely unhelpful here, it fails."""
    client = wire(
        {
            "searchSecretMessages": {
                "@type": "foundMessages",
                "messages": [{"@type": "message", "id": 3, "date": 1, "content": {}}],
                "total_count": 1,
            }
        }
    )

    await sa.search_secret_messages(chat_id=1, query="hello", account="acct")

    assert "searchChatMessages" not in client.types()
    assert client.sent("searchSecretMessages")["query"] == "hello"


@pytest.mark.asyncio
async def test_no_matches_is_distinguished_from_no_history(wire):
    """An empty answer has two completely different meanings and only one of them
    means 'that word was never sent'."""
    wire({"searchSecretMessages": {"@type": "foundMessages", "messages": [], "total_count": 0}})

    answer = await sa.search_secret_messages(chat_id=1, query="hello", account="acct")

    lowered = answer.lower()
    assert "no message" in lowered or "no match" in lowered
    assert "device" in lowered, "did not warn that the local copy is the only copy"


@pytest.mark.asyncio
async def test_search_results_go_through_the_shared_record_builder(wire):
    """Search returns message text straight off Telegram. Building its own record
    shape would route untrusted content around the sanitiser every other
    message-bearing tool here uses."""
    wire(
        {
            "searchSecretMessages": {
                "@type": "foundMessages",
                "messages": [
                    {
                        "@type": "message",
                        "id": 3,
                        "date": 1,
                        "content": {
                            "@type": "messageText",
                            "text": {"text": "hel​lothere"},
                        },
                    }
                ],
                "total_count": 1,
            }
        }
    )

    result = _payload(await sa.search_secret_messages(chat_id=1, query="hel", account="acct"))

    text = result["messages"][0]["text"]
    assert "​" not in text, "a zero-width character survived into the result"
    assert "" not in text, "a control character survived into the result"


@pytest.mark.asyncio
async def test_a_limit_above_the_ceiling_is_clamped_and_the_clamp_is_declared(wire):
    """The house convention is to clamp rather than refuse, but never silently:
    a caller who asked for 5000 and got 100 must be able to tell that from a chat
    that only held 100, or they will read a trimmed answer as a complete one."""
    client = wire(
        {
            "searchSecretMessages": {
                "@type": "foundMessages",
                "messages": [{"@type": "message", "id": 3, "date": 1, "content": {}}],
                "total_count": 1,
            }
        }
    )

    answer = await sa.search_secret_messages(chat_id=1, query="x", limit=5000, account="acct")

    assert client.sent("searchSecretMessages")["limit"] == 100, "asked Telegram for 5000"
    # Inside `results`, matching read_secret_messages: this family returns the
    # payload as an object rather than a list.
    body = _payload(answer)
    assert body["requested_limit"] == 5000
    assert body["effective_limit"] == 100
    assert "limit_note" in body


@pytest.mark.asyncio
async def test_a_limit_below_one_is_refused_rather_than_clamped(wire):
    """Telegram reads a non-positive limit as 'no limit', which is the opposite
    of what it looks like, so this one really is a refusal."""
    client = wire()

    answer = await sa.search_secret_messages(chat_id=1, query="x", limit=0, account="acct")

    assert "searchSecretMessages" not in client.types()
    assert "at least 1" in answer


# --------------------------------------------------------------------------
# Copying in
# --------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_copying_in_asks_telegram_before_it_sends(wire):
    """TDLib computes `can_be_copied_to_secret_chat` per message, from the content
    type. Deciding that here would be this server guessing at a rule it does not
    own."""
    client = wire(
        {
            "getMessageProperties": {
                "@type": "messageProperties",
                "can_be_copied_to_secret_chat": False,
            }
        }
    )

    answer = await sa.copy_into_secret_chat(
        from_chat_id=9, message_id=5, to_chat_id=1, account="acct"
    )

    assert "forwardMessages" not in client.types(), "sent a message Telegram had refused"
    assert "copied" in answer.lower() or "cannot" in answer.lower()


@pytest.mark.asyncio
async def test_copying_in_sends_a_copy_and_says_attribution_is_lost(wire):
    """The encrypted message carries no forwarding information at all, so this
    arrives looking like an original. A caller who thinks otherwise will quote
    someone without meaning to."""
    client = wire(
        {
            "getMessageProperties": {
                "@type": "messageProperties",
                "can_be_copied_to_secret_chat": True,
            },
            "forwardMessages": {"@type": "messages", "messages": [{"id": 77}]},
        }
    )

    result = _payload(
        await sa.copy_into_secret_chat(from_chat_id=9, message_id=5, to_chat_id=1, account="acct")
    )

    sent = client.sent("forwardMessages")
    assert sent["send_copy"] is True, "forwarded with attribution the protocol cannot carry"
    assert sent["chat_id"] == 1 and sent["from_chat_id"] == 9
    assert "attribution" in json.dumps(result).lower()


# --------------------------------------------------------------------------
# The readiness guard, applied where it belongs
# --------------------------------------------------------------------------


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "call",
    [
        lambda: sa.delete_secret_message(chat_id=1, message_id=5, account="acct"),
        lambda: sa.clear_secret_history(chat_id=1, confirm_chat_id=1, account="acct"),
        lambda: sa.mark_secret_read(chat_id=1, account="acct"),
        lambda: sa.send_secret_typing(chat_id=1, account="acct"),
    ],
)
async def test_a_mutating_tool_refuses_a_chat_that_is_not_ready(wire, call):
    client = wire({"getSecretChat": {"state": {"@type": "secretChatStatePending"}}})

    answer = await call()

    assert "pending" in answer.lower()
    for mutation in ("deleteMessages", "deleteChatHistory", "viewMessages", "sendChatAction"):
        assert mutation not in client.types()


@pytest.mark.asyncio
async def test_reading_a_closed_chat_is_still_allowed(wire):
    """The history is local and it is the only copy. Refusing to search it because
    the chat is closed would destroy the last thing that still works."""
    wire(
        {
            "getSecretChat": {"state": {"@type": "secretChatStateClosed"}},
            "searchSecretMessages": {
                "@type": "foundMessages",
                "messages": [{"@type": "message", "id": 3, "date": 1, "content": {}}],
                "total_count": 1,
            },
        }
    )

    answer = await sa.search_secret_messages(chat_id=1, query="x", account="acct")

    assert "closed" not in answer.lower(), "refused to read a closed chat's local history"


@pytest.mark.asyncio
async def test_telegrams_own_refusal_is_shown_rather_than_filed_under_a_code(wire):
    wire({"deleteMessages": TDLibError(400, "MESSAGE_DELETE_FORBIDDEN")})

    answer = await sa.delete_secret_message(chat_id=1, message_id=5, account="acct")

    assert "MESSAGE_DELETE_FORBIDDEN" in answer
    assert "error occurred" not in answer.lower()
