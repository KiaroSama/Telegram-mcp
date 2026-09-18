"""A deletion reports what is KNOWN, and separates it from what is not.

Deleting is irreversible, so the report is the only thing a caller has to decide
what to do next - and the obvious next move, running it again, is destructive.
Two sentences were claiming knowledge nobody had:

* **A first call that timed out still said "Messages WERE deleted."** Zero
  passes completed. Nothing had been acknowledged. The caller was told to
  re-read the chat for messages that may never have gone.
* **A timeout mid-batch was reported as "failed."** A lost response says
  nothing about whether the server acted; calling it a failure invites a retry
  that deletes a second hundred messages the caller thought were still there.

The fix in both places is the same distinction: requested, acknowledged,
rejected, UNCONFIRMED, unattempted - and a timeout lands in the fourth, never
the second or third.
"""

import asyncio

import pytest

from telegram_mcp.tools import messages_delete


class _Entity:
    def __init__(self, id=777):
        self.id = id


@pytest.fixture
def wired(monkeypatch):
    """A client whose delete behaviour each test sets, with a private chat."""

    class _Client:
        def __init__(self):
            self.calls = []
            self.behaviour = None

        async def __call__(self, request):
            self.calls.append(request)
            if self.behaviour is not None:
                return await self.behaviour(request, len(self.calls))
            return type("R", (), {"offset": 0, "pts_count": 1})()

    client = _Client()

    async def _resolve(chat_id, cl=None):
        return _Entity()

    monkeypatch.setattr(messages_delete, "get_client", lambda account=None: client)
    monkeypatch.setattr(messages_delete, "resolve_entity", _resolve)

    async def _connected(cl):
        return None

    monkeypatch.setattr(messages_delete, "ensure_connected", _connected, raising=False)
    return client


# --- the history clear --------------------------------------------------------


@pytest.mark.asyncio
async def test_a_first_call_that_times_out_does_not_claim_anything_was_deleted(wired, monkeypatch):
    """The counterexample. No pass completed, so nothing is known to have gone -
    and the old sentence sent the caller to re-read a chat for a deletion that
    may never have happened."""

    async def _never_answers(request, call_number):
        await asyncio.get_running_loop().create_future()

    monkeypatch.setattr(messages_delete, "_DELETE_HISTORY_DEADLINE_SECONDS", 0.2)
    wired.behaviour = _never_answers

    said = await asyncio.wait_for(
        messages_delete.delete_chat_history(chat_id=1, account="default"),
        timeout=15,
    )

    assert "WERE deleted" not in said, "a timed-out first call still claimed a deletion"
    assert "not known" in said.lower() or "unconfirmed" in said.lower()


@pytest.mark.asyncio
async def test_a_later_pass_timing_out_still_reports_the_completed_ones(wired, monkeypatch):
    """The other direction: passes DID complete, so those deletions are real and
    saying otherwise would be just as wrong."""
    calls = {"n": 0}

    async def _one_then_hang(request, call_number):
        calls["n"] += 1
        if calls["n"] == 1:
            return type("R", (), {"offset": 5, "pts_count": 1})()
        await asyncio.get_running_loop().create_future()

    monkeypatch.setattr(messages_delete, "_DELETE_HISTORY_DEADLINE_SECONDS", 0.2)
    wired.behaviour = _one_then_hang

    said = await asyncio.wait_for(
        messages_delete.delete_chat_history(chat_id=1, account="default"),
        timeout=15,
    )

    assert "1 pass" in said
    assert "DID delete" in said, "completed passes were not reported as real"
    assert "NOT KNOWN" not in said, "completed passes were reported as uncertain"


# --- the bulk delete ----------------------------------------------------------


@pytest.mark.asyncio
async def test_a_timeout_mid_batch_is_unconfirmed_not_failed(wired, monkeypatch):
    """A lost response is not proof the server did nothing. Calling it "failed"
    invites a retry that deletes a second batch the caller believed was still
    there."""
    monkeypatch.setattr(messages_delete, "_DELETE_CHUNK", 2)

    async def _second_hangs(request, call_number):
        if call_number >= 2:
            raise asyncio.TimeoutError()
        return type("R", (), {"pts_count": 1})()

    wired.behaviour = _second_hangs
    monkeypatch.setattr(
        messages_delete,
        "_ids_that_live_here",
        _lives_here([1, 2, 3, 4, 5, 6]),
    )

    said = await asyncio.wait_for(
        messages_delete.delete_messages_bulk(chat_id=1, message_ids=[1, 2, 3, 4, 5, 6]),
        timeout=15,
    )

    assert "failed" not in said.lower(), "an unanswered batch was reported as a failure"
    assert "unconfirmed" in said.lower()
    # And the caller is told WHICH ids are in which state, not just how many.
    assert "1, 2" in said and "3, 4" in said


@pytest.mark.asyncio
async def test_a_rejected_batch_is_still_reported_as_rejected(wired, monkeypatch):
    """A deterministic refusal from Telegram is different from a lost response
    and must keep saying so."""
    monkeypatch.setattr(messages_delete, "_DELETE_CHUNK", 2)

    async def _second_refuses(request, call_number):
        if call_number >= 2:
            raise ValueError("MESSAGE_ID_INVALID")
        return type("R", (), {"pts_count": 1})()

    wired.behaviour = _second_refuses
    monkeypatch.setattr(messages_delete, "_ids_that_live_here", _lives_here([1, 2, 3, 4, 5, 6]))

    said = await asyncio.wait_for(
        messages_delete.delete_messages_bulk(chat_id=1, message_ids=[1, 2, 3, 4, 5, 6]),
        timeout=15,
    )

    assert "rejected" in said.lower()
    assert "unconfirmed" not in said.lower()


def _lives_here(ids):
    async def _resolve(cl, entity, wanted):
        return list(ids), [], []

    return _resolve
