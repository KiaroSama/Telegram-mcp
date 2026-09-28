"""`get_message_views`: read a post's view counter without adding to it (plan 016).

`messages.getMessagesViews` both READS and INCREMENTS the counter; which one is
the `increment` flag. Counting a view tells the channel someone looked, which is
a seen signal under ghost mode that no approval covers - so the tool always
sends `increment=False` and offers no way to change that.
"""

import inspect

import pytest
from telethon.tl import functions, types

from telegram_mcp.tools import message_views as mod

CHANNEL = types.Channel(id=555, title="News", photo=None, date=None, broadcast=True)


class Recorder:
    def __init__(self, answer):
        self.sent = []
        self.answer = answer

    async def __call__(self, request):
        self.sent.append(request)
        return self.answer


def _views(*items):
    return types.messages.MessageViews(views=list(items), chats=[], users=[])


@pytest.mark.asyncio
async def test_reads_without_counting_and_maps_each_id(wire_client):
    client = Recorder(
        _views(
            types.MessageViews(
                views=120, forwards=4, replies=types.MessageReplies(replies=7, replies_pts=1)
            ),
            types.MessageViews(),
        )
    )
    wire_client(mod, client, entity=CHANNEL)

    result = await mod.get_message_views("@news", [10, 11])

    request = client.sent[0]
    assert isinstance(request, functions.messages.GetMessagesViewsRequest)
    assert request.increment is False, "a read must never count a view"
    assert request.id == [10, 11]
    assert '"message_id": 10, "views": 120, "forwards": 4, "replies": 7' in result
    assert '"message_id": 11, "views": null, "forwards": null, "replies": null' in result


@pytest.mark.asyncio
async def test_no_ids_is_refused_before_any_request(wire_client):
    client = Recorder(_views())
    wire_client(mod, client, entity=CHANNEL)

    result = await mod.get_message_views("@news", [])

    assert client.sent == []
    assert "at least one" in result


def test_there_is_no_way_to_count_a_view():
    """The ghost-mode guard: no argument, and no call site, can pass True."""
    assert "increment" not in inspect.signature(mod.get_message_views).parameters
    source = inspect.getsource(mod)
    assert "increment=True" not in source
    assert "increment=False" in source


def test_it_is_a_read():
    from telegram_mcp.runtime import mcp

    hints = mcp._tool_manager.get_tool("get_message_views").annotations
    assert hints.read_only_hint is True and hints.destructive_hint is False
