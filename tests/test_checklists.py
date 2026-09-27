"""Checklists (spec 015 US1), on a fake client.

A checklist is a message whose tasks carry ids the sender picks (positive, unique
within the list). What these pin: ids start at 1 and appended ones continue after
the highest, removing tasks resends the list WITHOUT renumbering (so the other
tasks keep their completions), Telegram's published limits are checked before
anything is sent, and reading one reports who completed what, and when.
"""

import json
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon.errors import RPCError
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tools import checklists as mod

CHAT = tl.Channel(
    id=777, title="Test", photo=tl.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=7
)
SARA = tl.User(id=42, first_name="Sara", access_hash=4)


def _text(value):
    return tl.TextWithEntities(text=value, entities=[])


def _todo(items, others_can_append=False, others_can_complete=True, completions=None):
    return tl.MessageMediaToDo(
        todo=tl.TodoList(
            title=_text("Launch"),
            list=[tl.TodoItem(id=i, title=_text(t)) for i, t in items],
            others_can_append=others_can_append,
            others_can_complete=others_can_complete,
        ),
        completions=completions,
    )


class _Client:
    def __init__(self, media=None, limits=None, refuse=None):
        self.sent = []
        self.media = media
        self.limits = limits or {}
        self.refuse = refuse

    async def __call__(self, request):
        self.sent.append(request)
        if isinstance(request, functions.help.GetAppConfigRequest):
            values = [
                tl.JsonObjectValue(key=k, value=tl.JsonNumber(value=float(v)))
                for k, v in self.limits.items()
            ]
            return tl.help.AppConfig(hash=1, config=tl.JsonObject(value=values))
        if self.refuse:
            raise RPCError(request, self.refuse, 400)
        return tl.Updates(
            updates=[tl.UpdateMessageID(id=31, random_id=1)], users=[], chats=[], date=None, seq=0
        )

    async def get_messages(self, entity, ids=None, limit=None):
        return SimpleNamespace(id=ids, media=self.media, sender_id=1)

    async def get_entity(self, peer):
        return SARA

    def of(self, kind):
        return [r for r in self.sent if isinstance(r, kind)]


@pytest.fixture
def client(monkeypatch):
    holder = {}

    def use(**kwargs):
        holder["c"] = _Client(**kwargs)
        return holder["c"]

    async def _resolve(value, cl=None, account=None):
        return CHAT

    async def _connected(cl=None):
        return None

    monkeypatch.setattr(mod, "get_client", lambda account=None: holder["c"])
    monkeypatch.setattr(mod, "resolve_entity", _resolve)
    monkeypatch.setattr(mod, "ensure_connected", _connected)
    mod._limits_cache.clear()
    return use


@pytest.mark.asyncio
async def test_sending_numbers_the_tasks_and_keeps_both_settings(client):
    c = client()
    text = await mod.send_checklist(
        "chat", "Launch", ["Write", "Test"], others_can_add=True, others_can_complete=False
    )
    (sent,) = c.of(functions.messages.SendMediaRequest)
    todo = sent.media.todo
    assert todo.title.text == "Launch"
    assert [(i.id, i.title.text) for i in todo.list] == [(1, "Write"), (2, "Test")]
    assert todo.others_can_append is True and not todo.others_can_complete
    assert "31" in text


@pytest.mark.asyncio
async def test_published_limits_are_checked_before_sending(client):
    c = client(limits={"todo_items_max": 2, "todo_item_length_max": 5, "todo_title_length_max": 4})
    for title, tasks in (("Ok", ["a", "b", "c"]), ("Ok", ["toolong"]), ("Toolong", ["a"])):
        text = await mod.send_checklist("chat", title, tasks)
        assert "limit" in text.lower() or "at most" in text.lower()
    assert c.of(functions.messages.SendMediaRequest) == []


@pytest.mark.asyncio
async def test_an_empty_checklist_is_refused(client):
    c = client()
    await mod.send_checklist("chat", "Launch", [])
    assert c.of(functions.messages.SendMediaRequest) == []


@pytest.mark.asyncio
async def test_appended_tasks_continue_after_the_highest_id(client):
    c = client(media=_todo([(1, "a"), (5, "b")]))
    await mod.add_checklist_tasks("chat", 30, ["c", "d"])
    (sent,) = c.of(functions.messages.AppendTodoListRequest)
    assert [i.id for i in sent.list] == [6, 7] and sent.msg_id == 30


@pytest.mark.asyncio
async def test_done_and_undone_go_in_one_request(client):
    c = client(media=_todo([(1, "a"), (2, "b")]))
    await mod.set_checklist_tasks_done("chat", 30, done=[1], undone=[2])
    (sent,) = c.of(functions.messages.ToggleTodoCompletedRequest)
    assert sent.completed == [1] and sent.incompleted == [2]


@pytest.mark.asyncio
async def test_an_unknown_task_id_changes_nothing(client):
    c = client(media=_todo([(1, "a")]))
    text = await mod.set_checklist_tasks_done("chat", 30, done=[9])
    assert c.of(functions.messages.ToggleTodoCompletedRequest) == [] and "9" in text


@pytest.mark.asyncio
async def test_removing_keeps_the_other_ids_and_settings(client):
    c = client(media=_todo([(1, "a"), (2, "b"), (3, "c")], others_can_append=True))
    await mod.remove_checklist_tasks("chat", 30, [2])
    (sent,) = c.of(functions.messages.EditMessageRequest)
    todo = sent.media.todo
    assert [i.id for i in todo.list] == [1, 3]
    assert todo.others_can_append is True and todo.others_can_complete is True


@pytest.mark.asyncio
async def test_reading_reports_who_completed_what_and_when(client):
    when = datetime(2026, 9, 27, 12, 0, tzinfo=timezone.utc)
    completion = tl.TodoCompletion(id=2, completed_by=tl.PeerUser(user_id=42), date=when)
    client(media=_todo([(1, "a"), (2, "b")], completions=[completion]))
    payload = json.loads(await mod.get_checklist("chat", 30))
    tasks = {t["id"]: t for t in payload["results"]}
    assert tasks[1]["done"] is False
    assert tasks[2]["done"] is True and tasks[2]["completed_by"]["name"] == "Sara"
    assert tasks[2]["completed_at"].startswith("2026-09-27")
    assert payload["title"] == "Launch"


@pytest.mark.asyncio
async def test_a_message_that_is_not_a_checklist_is_refused(client):
    c = client(media=None)
    text = await mod.add_checklist_tasks("chat", 30, ["x"])
    assert (
        "not a checklist" in text.lower() and c.of(functions.messages.AppendTodoListRequest) == []
    )


@pytest.mark.asyncio
async def test_a_telegram_refusal_is_reported_by_name(client):
    client(refuse="PREMIUM_ACCOUNT_REQUIRED")
    text = await mod.send_checklist("chat", "Launch", ["a"])
    assert "PREMIUM_ACCOUNT_REQUIRED" in text
