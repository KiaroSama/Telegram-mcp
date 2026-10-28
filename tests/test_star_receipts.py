"""Original recipient receipt lookup through one existing account, never a purchase."""

import asyncio
import importlib
import json
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon.tl import functions, types

STAMP = datetime(2026, 10, 1, tzinfo=timezone.utc)
REFERENCE = "synthetic-receiver-reference"


def entry(**changes):
    fields = dict(
        id=REFERENCE,
        amount=types.StarsAmount(amount=50, nanos=0),
        date=STAMP,
        peer=types.StarsTransactionPeerFragment(),
        gift=True,
        transaction_date=STAMP,
        transaction_url="https://tonviewer.com/" + "a" * 64,
    )
    fields.update(changes)
    return types.StarsTransaction(**fields)


class Client:
    def __init__(self, history=None, action=None, fail=False):
        self.history = [entry()] if history is None else history
        self.action = action or types.MessageActionGiftStars(
            currency="USD", amount=75, stars=50, transaction_id=REFERENCE
        )
        self.messages = []
        self.requests = []
        self.fail = fail

    async def get_messages(self, peer, *, ids):
        self.messages.append((peer, ids))
        return SimpleNamespace(id=ids, action=self.action)

    async def __call__(self, request):
        self.requests.append(request)
        if self.fail:
            raise RuntimeError("private upstream financial response")
        return SimpleNamespace(history=self.history)


@pytest.mark.asyncio
async def test_original_receipt_uses_one_self_lookup_without_payment_or_extra_client(wire_client):
    mod = importlib.import_module("telegram_mcp.tools.star_receipts")
    client = wire_client(mod, Client())
    answer = json.loads(await asyncio.wait_for(mod.get_stars_receipt(176), timeout=2))
    assert client.messages == [(777000, 176)]
    (request,) = client.requests
    assert isinstance(request, functions.payments.GetStarsTransactionsByIDRequest)
    assert isinstance(request.peer, types.InputPeerSelf)
    assert request.ton is None
    assert len(request.id) == 1 and request.id[0].id == REFERENCE
    assert request.id[0].refund is None
    assert answer["status"] == "observed"
    assert answer["receipt"]["stars"] == 50
    assert answer["transaction"] == {
        "id": REFERENCE,
        "amount": 50,
        "nanos": 0,
        "date": STAMP.isoformat(),
        "peer_type": "StarsTransactionPeerFragment",
        "gift": True,
        "pending": False,
        "failed": False,
        "refund": False,
        "transaction_date": STAMP.isoformat(),
        "transaction_url": "https://tonviewer.com/" + "a" * 64,
    }


@pytest.mark.parametrize(
    "history",
    [
        [],
        [entry(id="different")],
        [entry(), entry()],
        [object()],
        [entry(amount=types.StarsAmount(amount=True, nanos=0))],
        [entry(amount=types.StarsAmount(amount=50, nanos=1000000000))],
        [entry(date=STAMP.replace(tzinfo=None))],
        [entry(pending="false")],
    ],
)
@pytest.mark.asyncio
async def test_missing_conflicting_or_malformed_ledger_is_not_observed(wire_client, history):
    mod = importlib.import_module("telegram_mcp.tools.star_receipts")
    client = wire_client(mod, Client(history=history))
    answer = json.loads(await asyncio.wait_for(mod.get_stars_receipt(176), timeout=2))
    assert answer == {"status": "unavailable", "code": "stars_transaction_unavailable"}
    assert len(client.requests) == 1


@pytest.mark.parametrize("message_id", [0, -1, 2**31, True])
@pytest.mark.asyncio
async def test_invalid_message_id_performs_no_read(wire_client, message_id):
    mod = importlib.import_module("telegram_mcp.tools.star_receipts")
    client = wire_client(mod, Client())
    answer = json.loads(await mod.get_stars_receipt(message_id))
    assert answer["status"] == "unavailable"
    assert client.messages == [] and client.requests == []


@pytest.mark.parametrize(
    "action",
    [
        types.MessageActionEmpty(),
        types.MessageActionGiftStars(currency="USD", amount=75, stars=50),
        types.MessageActionGiftStars(
            currency="USD", amount=75, stars=50, transaction_id="bad\nref"
        ),
    ],
)
@pytest.mark.asyncio
async def test_non_receipt_or_missing_reference_never_queries_ledger(wire_client, action):
    mod = importlib.import_module("telegram_mcp.tools.star_receipts")
    client = wire_client(mod, Client(action=action))
    answer = json.loads(await mod.get_stars_receipt(176))
    assert answer["code"] == "stars_receipt_unavailable" and client.requests == []


@pytest.mark.parametrize(
    "url",
    [
        None,
        "http://example.test/tx",
        "https://user:password@example.test/tx",
        "javascript:alert(1)",
        "https://example.test/\nprivate",
        "https://[invalid",
    ],
)
@pytest.mark.asyncio
async def test_unsafe_or_absent_links_are_not_rewritten_or_followed(wire_client, url):
    mod = importlib.import_module("telegram_mcp.tools.star_receipts")
    client = wire_client(mod, Client(history=[entry(transaction_url=url, pending=True)]))
    answer = json.loads(await mod.get_stars_receipt(176))
    assert answer["status"] == "observed" and answer["transaction"]["pending"] is True
    assert "transaction_url" not in answer["transaction"]
    assert len(client.requests) == 1


@pytest.mark.asyncio
async def test_lookup_error_never_exposes_upstream_financial_text(wire_client):
    mod = importlib.import_module("telegram_mcp.tools.star_receipts")
    client = wire_client(mod, Client(fail=True))
    answer = await mod.get_stars_receipt(176)
    assert json.loads(answer) == {"status": "unavailable", "code": "stars_lookup_unavailable"}
    assert "private" not in answer and len(client.requests) == 1
