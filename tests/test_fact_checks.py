"""Fact-checks on channel posts: read, set, delete (plan 016)."""

import pytest
import telethon.errors.rpcerrorlist as rpc
from telethon.tl import functions, types

from telegram_mcp.tools import fact_checks as mod

CHANNEL = types.Channel(id=555, title="News", photo=None, date=None, broadcast=True)


class Recorder:
    def __init__(self, answer=True):
        self.sent = []
        self.answer = answer

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(self.answer, Exception):
            raise self.answer
        return self.answer


@pytest.mark.asyncio
async def test_read_maps_each_message(wire_client):
    client = Recorder(
        [
            types.FactCheck(
                hash=1,
                need_check=True,
                country="US",
                text=types.TextWithEntities(text="Mis​leading", entities=[]),
            ),
            types.FactCheck(hash=2),
        ]
    )
    wire_client(mod, client, entity=CHANNEL)

    result = await mod.get_fact_check("@news", [7, 8])

    request = client.sent[0]
    assert isinstance(request, functions.messages.GetFactCheckRequest)
    assert request.msg_id == [7, 8]
    assert '"message_id": 7, "need_check": true, "country": "US", "text": "Misleading"' in result
    assert '"message_id": 8, "need_check": false, "country": null, "text": null' in result


@pytest.mark.parametrize(
    "answer", [[], rpc.MsgIdInvalidError(request=None)], ids=["empty", "msg_id_invalid"]
)
@pytest.mark.asyncio
async def test_no_fact_check_is_an_empty_list(wire_client, answer):
    wire_client(mod, Recorder(answer), entity=CHANNEL)

    result = await mod.get_fact_check("@news", [7])

    assert '"results": []' in result


@pytest.mark.asyncio
async def test_set_sends_the_text_with_no_entities(wire_client):
    client = Recorder()
    wire_client(mod, client, entity=CHANNEL)

    await mod.set_fact_check("@news", 7, "Context: the photo is from 2019.")

    request = client.sent[0]
    assert isinstance(request, functions.messages.EditFactCheckRequest)
    assert request.msg_id == 7
    assert request.text == types.TextWithEntities(
        text="Context: the photo is from 2019.", entities=[]
    )


@pytest.mark.asyncio
async def test_empty_text_is_refused_before_any_request(wire_client):
    client = Recorder()
    wire_client(mod, client, entity=CHANNEL)

    result = await mod.set_fact_check("@news", 7, "   ")

    assert client.sent == []
    assert "delete_fact_check" in result


@pytest.mark.asyncio
async def test_delete_sends_its_request(wire_client):
    client = Recorder()
    wire_client(mod, client, entity=CHANNEL)

    await mod.delete_fact_check("@news", 7)

    assert isinstance(client.sent[0], functions.messages.DeleteFactCheckRequest)
    assert client.sent[0].msg_id == 7


def test_hints():
    from telegram_mcp.runtime import mcp

    get = mcp._tool_manager.get_tool

    assert get("get_fact_check").annotations.read_only_hint is True
    assert get("set_fact_check").annotations.destructive_hint is False
    assert get("delete_fact_check").annotations.destructive_hint is True
    assert "always asks" in get("delete_fact_check").description
