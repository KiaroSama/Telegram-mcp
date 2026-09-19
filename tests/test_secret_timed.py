"""The timed send: arm the chat's timer, send one message, put the timer back.

This tool exists against a recommendation. A secret chat has no per-message
self-destruct -- Telegram answers one with "Messages can self-destruct only in
private chats" -- so the only mechanism is the timer belonging to the CHAT, and
using it for a single message means changing shared state and changing it back.
The owner asked for it anyway, which is their call about their own account, and
the three guarantees below are what makes it safe enough to ship. All three are
pinned here because all three are invisible when they break.

1. The restore runs even when the send fails. Otherwise a network error between
   arming and sending leaves the conversation armed forever.
2. A FAILED restore is reported loudly, as `unconfirmed`, naming the timer the
   chat is stuck on. This is the real harm: the message went, the chat is not
   how the caller left it, and nobody knows.
3. The previous timer is restored, not zero. A chat that already had a timer
   must not come back disarmed.

Constitution Principle III is what shapes case 2: the send succeeded and the
state is wrong, so the outcome is neither success nor failure -- it is unknown,
and it says so.
"""

import json

import pytest

from telegram_mcp.tdlib import TDLibError
from telegram_mcp.tools import secret_timed as st

_READY_SECRET = {"@type": "secretChat", "state": {"@type": "secretChatStateReady"}, "layer": 144}


def _chat(timer=0):
    return {
        "@type": "chat",
        "type": {"@type": "chatTypeSecret", "secret_chat_id": 7},
        "message_auto_delete_time": timer,
    }


class FakeTDLib:
    """Records requests; can be told to fail the Nth timer write."""

    def __init__(self, answers=None, fail_timer_on=None):
        self.requests = []
        self.answers = {"getChat": _chat(0), "getSecretChat": _READY_SECRET}
        self.answers.update(answers or {})
        self.fail_timer_on = fail_timer_on
        self.timer_writes = 0

    async def request(self, obj, timeout=30.0):
        self.requests.append(obj)
        if obj["@type"] == "setChatMessageAutoDeleteTime":
            self.timer_writes += 1
            if self.timer_writes == self.fail_timer_on:
                raise TDLibError(500, "TIMER_WRITE_FAILED")
            return {"@type": "ok"}
        answer = self.answers.get(obj["@type"])
        if isinstance(answer, Exception):
            raise answer
        return answer if answer is not None else {"@type": "ok"}

    def timers(self):
        return [
            r["message_auto_delete_time"]
            for r in self.requests
            if r["@type"] == "setChatMessageAutoDeleteTime"
        ]

    def types(self):
        return [r["@type"] for r in self.requests]


@pytest.fixture
def wire(monkeypatch):
    def _wire(answers=None, fail_timer_on=None):
        client = FakeTDLib(answers, fail_timer_on)
        monkeypatch.setattr(st, "_account_label", lambda account=None: "acct")

        async def _client(label):
            return client

        monkeypatch.setattr(st, "secret_client", _client)
        return client

    return _wire


def _payload(raw):
    return json.loads(raw)["results"]


@pytest.mark.asyncio
async def test_the_timer_is_armed_then_put_back(wire):
    client = wire()

    result = _payload(
        await st.send_timed_secret_message(chat_id=1, message="hi", seconds=30, account="acct")
    )

    assert client.timers() == [30, 0], "did not arm then restore, in that order"
    assert result["sent"] is True
    assert result["timer_restored"] is True


@pytest.mark.asyncio
async def test_an_existing_timer_comes_back_rather_than_zero(wire):
    """Restoring to zero would silently disarm a chat the owner had deliberately
    armed - a privacy setting quietly turned off by a convenience."""
    client = wire({"getChat": _chat(604800)})

    await st.send_timed_secret_message(chat_id=1, message="hi", seconds=30, account="acct")

    assert client.timers() == [30, 604800]


@pytest.mark.asyncio
async def test_a_send_that_fails_still_disarms(wire):
    """The `finally`. Without it a refused send leaves the conversation armed,
    and the caller has no idea because their message never arrived."""
    client = wire({"sendMessage": TDLibError(400, "CHAT_SEND_FAILED")})

    answer = await st.send_timed_secret_message(
        chat_id=1, message="hi", seconds=30, account="acct"
    )

    assert client.timers() == [30, 0], "left the chat armed after a failed send"
    assert "CHAT_SEND_FAILED" in answer


@pytest.mark.asyncio
async def test_a_failed_restore_is_reported_as_unconfirmed_and_names_the_timer(wire):
    """The one case where the message succeeded and the call is still not a
    success: the chat is armed, the caller did not ask for that, and only this
    reply can tell them."""
    client = wire(fail_timer_on=2)

    result = _payload(
        await st.send_timed_secret_message(chat_id=1, message="hi", seconds=30, account="acct")
    )

    assert result["sent"] is True, "the message did go; saying otherwise would be false"
    assert result["outcome"] == "unconfirmed"
    assert result["timer_left_on"] == 30
    assert "timer_restored" not in result
    body = json.dumps(result).lower()
    assert "set_secret_chat_timer" in body, "did not say how to fix it by hand"
    # The restore was attempted, not skipped: the chat is armed because the write
    # failed, which is a different fault from never having tried.
    assert client.timer_writes == 2


@pytest.mark.asyncio
async def test_a_failed_restore_after_a_failed_send_still_reports_the_armed_chat(wire):
    """Both went wrong. Reporting only the send error would hide the armed chat,
    which is the more dangerous of the two."""
    client = wire({"sendMessage": TDLibError(400, "CHAT_SEND_FAILED")}, fail_timer_on=2)

    answer = await st.send_timed_secret_message(
        chat_id=1, message="hi", seconds=30, account="acct"
    )

    assert "30" in answer, "did not name the timer the chat is left on"
    lowered = answer.lower()
    assert "timer" in lowered
    assert client.timers() == [30, 0], "did not attempt the restore"


@pytest.mark.asyncio
async def test_a_chat_that_is_not_ready_is_refused_before_the_timer_moves(wire):
    """Arming a chat that cannot be sent to would change its settings for a
    message that was never going to leave."""
    client = wire({"getSecretChat": {"state": {"@type": "secretChatStatePending"}}})

    answer = await st.send_timed_secret_message(
        chat_id=1, message="hi", seconds=30, account="acct"
    )

    assert "pending" in answer.lower()
    assert client.timers() == [], "armed a chat it then refused to send to"


@pytest.mark.asyncio
async def test_the_seconds_are_validated_before_anything_is_touched(wire):
    client = wire()

    answer = await st.send_timed_secret_message(chat_id=1, message="hi", seconds=0, account="acct")

    assert client.timers() == []
    assert "set_secret_chat_timer" in answer, "did not point at the tool that turns it off"


@pytest.mark.asyncio
async def test_the_media_variant_arms_and_restores_the_same_way(wire, monkeypatch, tmp_path):
    sample = tmp_path / "x.jpg"
    sample.write_bytes(b"jpeg")

    async def _path(raw_path, ctx, tool_name):
        return sample, None

    monkeypatch.setattr(st, "_resolve_readable_file_path", _path)
    client = wire({"getChat": _chat(60)})

    result = _payload(
        await st.send_timed_secret_media(
            chat_id=1, file_path=str(sample), seconds=10, account="acct"
        )
    )

    assert client.timers() == [10, 60]
    assert result["kind"] == "photo"
    assert result["timer_restored"] is True


@pytest.mark.asyncio
async def test_the_media_variant_also_disarms_after_a_failed_send(wire, monkeypatch, tmp_path):
    sample = tmp_path / "x.jpg"
    sample.write_bytes(b"jpeg")

    async def _path(raw_path, ctx, tool_name):
        return sample, None

    monkeypatch.setattr(st, "_resolve_readable_file_path", _path)
    client = wire({"sendMessage": TDLibError(400, "UPLOAD_FAILED")})

    await st.send_timed_secret_media(chat_id=1, file_path=str(sample), seconds=10, account="acct")

    assert client.timers() == [10, 0]


@pytest.mark.asyncio
async def test_the_docstrings_warn_that_the_window_catches_the_other_person(wire):
    """The timer belongs to the chat, so anything they send inside the window is
    caught too. No amount of care here prevents it, which is exactly why it has
    to be said rather than engineered around."""
    for tool in (st.send_timed_secret_message, st.send_timed_secret_media):
        doc = (tool.__doc__ or "").lower()
        assert "other" in doc and "window" in doc, f"{tool.__name__} does not warn about it"
