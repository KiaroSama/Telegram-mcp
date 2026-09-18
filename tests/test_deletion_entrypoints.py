"""Every deletion entrypoint applies the same rules, and reports what happened.

The safeguards existed. They were each on ONE of the two paths, so which
protection a caller got depended on whether they deleted one message or a list:

* the peer preflight - the check that an id belongs to THIS chat, which matters
  because outside a channel `messages.deleteMessages` carries no peer at all -
  ran for bulk and not for single, so the one-message call was the dangerous one;
* the private-only refusal - `revoke=False` in a channel, where no per-account
  deletion exists - ran for single and not for bulk, so the same request was
  refused alone and performed globally in a list.

Two more about what a caller is told afterwards, which for an irreversible
operation is not a nicety:

* history reported `pts_count` as a count of deleted messages. It counts update
  events; a synthetic 777 counter became "777 messages deleted";
* a bulk failure after the first hundred lost that accepted prefix in a generic
  error, leaving the caller unable to tell "nothing happened" from "a hundred
  are gone" - and the obvious next move, running it again, is destructive in the
  first case.

Every test drives the PUBLIC tool and asserts on the request that went out. No
real Telegram traffic: the client is a double that records what it was asked.
"""

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import messages_delete as delete_mod

RAW_CHAT_ID = "@somechat"
CHANNEL = types.Channel(
    id=4242,
    title="A channel",
    photo=None,
    date=None,
    creator=True,
    left=False,
    broadcast=True,
    verified=False,
    megagroup=False,
    restricted=False,
    signatures=False,
    min=False,
    scam=False,
    has_link=False,
    has_geo=False,
    slowmode_enabled=False,
    access_hash=7,
    username=None,
    restriction_reason=[],
    admin_rights=None,
    banned_rights=None,
    default_banned_rights=None,
    participants_count=None,
)
MEGAGROUP = types.Channel(
    id=555,
    title="A supergroup",
    photo=None,
    date=None,
    creator=True,
    left=False,
    broadcast=False,
    verified=False,
    megagroup=True,
    restricted=False,
    signatures=False,
    min=False,
    scam=False,
    has_link=False,
    has_geo=False,
    slowmode_enabled=False,
    access_hash=9,
    username=None,
    restriction_reason=[],
    admin_rights=None,
    banned_rights=None,
    default_banned_rights=None,
    participants_count=None,
)
PRIVATE = types.User(id=99, is_self=False, access_hash=3, first_name="A person")


class _Client:
    """Records the requests, and answers `get_messages` like Telegram does."""

    def __init__(self, peer_of=None, missing=(), fail_after=None, history_offsets=None):
        self.sent = []
        self.deleted = []
        self.peer_of = peer_of or {}
        self.missing = set(missing)
        self.fail_after = fail_after
        # Python looks dunder methods up on the TYPE, so a per-test `__call__`
        # assigned to the instance is silently ignored - the answer is scripted
        # here instead.
        self.history_offsets = list(history_offsets or [])

    async def get_messages(self, entity, ids=None):
        wanted = ids if isinstance(ids, list) else [ids]
        answer = []
        for one in wanted:
            if one in self.missing:
                answer.append(None)
                continue
            peer = self.peer_of.get(one, types.PeerUser(user_id=99))
            answer.append(types.Message(id=one, peer_id=peer, message=""))
        return answer if isinstance(ids, list) else answer[0]

    async def delete_messages(self, entity, message_id, revoke=False):
        self.deleted.append((entity, message_id, revoke))

    async def __call__(self, request):
        self.sent.append(request)
        if self.fail_after is not None and len(self.sent) > self.fail_after:
            raise RuntimeError("Telegram refused this batch")
        if self.history_offsets:
            index = min(len(self.sent) - 1, len(self.history_offsets) - 1)
            return types.messages.AffectedHistory(
                pts=1, pts_count=777, offset=self.history_offsets[index]
            )
        return types.messages.AffectedMessages(pts=1, pts_count=777)


@pytest.fixture
def wire(monkeypatch):
    def _wire(client, entity):
        async def _resolve(chat_id, cl=None):
            return entity

        async def _connected(cl=None):
            return None

        monkeypatch.setattr(delete_mod, "get_client", lambda account=None: client)
        # `with_account` refreshes before routing now, so a test that does not
        # pin the registry reads the machine's real `.env` and the tool refuses
        # for being multi-account. The wired client IS the registry here.
        from telegram_mcp import connection as conn

        monkeypatch.setattr(conn, "refresh_accounts", lambda: [])
        monkeypatch.setattr(conn, "clients", {"default": client})
        monkeypatch.setattr(delete_mod, "resolve_entity", _resolve)
        monkeypatch.setattr(delete_mod, "ensure_connected", _connected)
        return client

    return _wire


# --- the peer preflight, on both paths -----------------------------------------


@pytest.mark.asyncio
async def test_single_deletion_refuses_an_id_from_another_chat(wire):
    """The defect. Outside a channel the request carries no peer, so this id
    names a message in a DIFFERENT conversation - and deleting it is final."""
    client = wire(_Client(peer_of={7: types.PeerUser(user_id=12345)}), PRIVATE)

    answer = await delete_mod.delete_message(RAW_CHAT_ID, 7)

    assert "Refusing to delete" in answer
    assert client.deleted == [], "the wrong chat's message was deleted anyway"


@pytest.mark.asyncio
async def test_single_deletion_proceeds_for_an_id_in_this_chat(wire):
    client = wire(_Client(peer_of={7: types.PeerUser(user_id=99)}), PRIVATE)

    answer = await delete_mod.delete_message(RAW_CHAT_ID, 7)

    assert client.deleted == [(PRIVATE, 7, False)]
    assert "deleted for you only" in answer


@pytest.mark.asyncio
async def test_single_deletion_says_so_when_the_message_is_already_gone(wire):
    client = wire(_Client(missing=[7]), PRIVATE)

    answer = await delete_mod.delete_message(RAW_CHAT_ID, 7)

    assert "already deleted, or never existed" in answer
    assert client.deleted == []


@pytest.mark.asyncio
async def test_a_channel_still_needs_no_preflight(wire):
    """A channel request carries the peer, so an id from elsewhere is simply not
    found there. Making it pay for a lookup it does not need would be waste."""
    client = wire(_Client(), CHANNEL)

    await delete_mod.delete_message(RAW_CHAT_ID, 7, revoke=True)

    assert client.deleted == [(CHANNEL, 7, True)]


@pytest.mark.asyncio
async def test_bulk_still_refuses_ids_from_another_chat(wire):
    client = wire(_Client(peer_of={2: types.PeerUser(user_id=12345)}), PRIVATE)

    answer = await delete_mod.delete_messages_bulk(RAW_CHAT_ID, [1, 2, 3])

    assert "Refusing to delete" in answer
    assert client.sent == []


# --- the private-only refusal, on both paths -----------------------------------


@pytest.mark.asyncio
async def test_bulk_refuses_private_only_deletion_in_a_channel(wire):
    """The mirror defect: single deletion refused this and bulk performed it, so
    the same request was safe one at a time and global in a list."""
    client = wire(_Client(), CHANNEL)

    answer = await delete_mod.delete_messages_bulk(RAW_CHAT_ID, [1, 2], revoke=False)

    assert "no per-account copy" in answer
    assert client.sent == [], "a global deletion went out for a private-only request"


@pytest.mark.asyncio
async def test_bulk_refuses_private_only_deletion_in_a_supergroup(wire):
    client = wire(_Client(), MEGAGROUP)

    answer = await delete_mod.delete_messages_bulk(RAW_CHAT_ID, [1], revoke=False)

    assert "no per-account copy" in answer
    assert client.sent == []


@pytest.mark.asyncio
async def test_single_refuses_private_only_deletion_in_a_supergroup(wire):
    client = wire(_Client(), MEGAGROUP)

    answer = await delete_mod.delete_message(RAW_CHAT_ID, 1, revoke=False)

    assert "no per-account copy" in answer
    assert client.deleted == []


@pytest.mark.asyncio
async def test_an_explicit_revoke_in_a_channel_is_performed(wire):
    client = wire(_Client(), CHANNEL)

    await delete_mod.delete_messages_bulk(RAW_CHAT_ID, [1, 2], revoke=True)

    assert len(client.sent) == 1
    assert isinstance(client.sent[0], functions.channels.DeleteMessagesRequest)


@pytest.mark.asyncio
async def test_a_private_chat_honours_revoke_false(wire):
    """Outside a channel a per-account deletion genuinely exists, so the refusal
    must not spread to where the operation is real."""
    client = wire(_Client(), PRIVATE)

    await delete_mod.delete_messages_bulk(RAW_CHAT_ID, [1], revoke=False)

    assert len(client.sent) == 1
    assert client.sent[0].revoke is False


# --- what the caller is told ---------------------------------------------------


@pytest.mark.asyncio
async def test_a_failure_after_the_first_batch_names_what_is_already_gone(wire):
    """101 ids is two batches. Losing the first in a generic error left the
    caller unable to tell "nothing happened" from "a hundred are gone"."""
    client = wire(_Client(fail_after=1), CHANNEL)

    answer = await delete_mod.delete_messages_bulk(RAW_CHAT_ID, list(range(1, 102)), revoke=True)

    assert len(client.sent) == 2, "the second batch was never attempted"
    # The report now names the ids per state rather than only counting them,
    # because after an irreversible deletion WHICH ones is the question.
    assert "accepted and GONE (100)" in answer
    assert "stopped part-way" in answer
    assert "re-read" in answer.lower()


@pytest.mark.asyncio
async def test_the_hundred_boundary_is_one_batch(wire):
    client = wire(_Client(), CHANNEL)

    await delete_mod.delete_messages_bulk(RAW_CHAT_ID, list(range(1, 101)), revoke=True)

    assert len(client.sent) == 1


@pytest.mark.asyncio
async def test_a_hundred_and_one_is_two(wire):
    client = wire(_Client(), CHANNEL)

    await delete_mod.delete_messages_bulk(RAW_CHAT_ID, list(range(1, 102)), revoke=True)

    assert len(client.sent) == 2


@pytest.mark.asyncio
async def test_duplicates_are_one_deletion(wire):
    client = wire(_Client(), CHANNEL)

    answer = await delete_mod.delete_messages_bulk(RAW_CHAT_ID, [5, 5, 5], revoke=True)

    assert client.sent[0].id == [5]
    assert "1 message(s)" in answer


@pytest.mark.asyncio
async def test_history_never_reports_an_update_counter_as_a_message_count(wire):
    """`pts_count` counts the update events a deletion produced. Reported as
    messages, a synthetic 777 counter became "777 messages deleted"."""
    client = wire(_Client(history_offsets=[0]), PRIVATE)

    answer = await delete_mod.delete_chat_history(RAW_CHAT_ID)

    assert client.sent, "the history request never went out"
    assert "777" not in answer
    assert "cleared" in answer


@pytest.mark.asyncio
async def test_an_incomplete_history_deletion_does_not_claim_a_count(wire):
    # An offset that never shrinks: the server keeps saying there is more.
    client = wire(_Client(history_offsets=[50]), PRIVATE)

    answer = await delete_mod.delete_chat_history(RAW_CHAT_ID)

    assert client.sent, "the history request never went out"
    assert "INCOMPLETE" in answer
    assert "777" not in answer
    assert "does not say how many" in answer
