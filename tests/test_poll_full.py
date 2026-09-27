"""Polls, every capability (spec 015 US3), on fake clients.

What these pin: each setting of Telegram's poll dialog lands on the right wire
field (and the negated ones - "allow revoting" is `revoting_disabled` - land
inverted); impossible combinations are refused before anything is sent; the new
management calls (add/delete an option, unread votes, poll statistics) build the
right request; reading a poll reports every setting; closing a poll no longer
drops the settings the old close did not carry; and a reply or a link can point
at one option or one checklist task.
"""

import json
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from telethon.tl import functions
from telethon.tl import types as tl

from telegram_mcp.tools import poll_build, poll_creation, poll_manage, polls

CHAT = tl.Channel(
    id=777, title="Test", photo=tl.ChatPhotoEmpty(), date=None, megagroup=True, access_hash=7
)
PHOTO = tl.InputMediaPhoto(id=tl.InputPhoto(id=1, access_hash=2, file_reference=b""))


def _text(value):
    return tl.TextWithEntities(text=value, entities=[])


def _poll_message(answers=("Yes", "No"), open_answers=False, quiz=False, correct=None, **flags):
    poll = tl.Poll(
        id=9,
        question=_text("Ship?"),
        answers=[tl.PollAnswer(text=_text(a), option=bytes([i])) for i, a in enumerate(answers)],
        hash=5,
        open_answers=open_answers,
        quiz=quiz,
        **flags,
    )
    voters = [
        tl.PollAnswerVoters(option=bytes([i]), voters=v, correct=(i in (correct or [])))
        for i, v in enumerate((3, 1)[: len(answers)])
    ]
    results = tl.PollResults(results=voters, total_voters=4)
    return SimpleNamespace(
        id=40, media=tl.MessageMediaPoll(poll=poll, results=results), sender_id=1
    )


class _Client:
    def __init__(self, message=None, answer=None):
        self.sent = []
        self.message = message
        self.answer = answer

    async def __call__(self, request):
        self.sent.append(request)
        if self.answer is not None and not isinstance(self.answer, Exception):
            return self.answer
        if isinstance(self.answer, Exception):
            raise self.answer
        return tl.Updates(
            updates=[tl.UpdateMessageID(id=41, random_id=1)], users=[], chats=[], date=None, seq=0
        )

    async def get_messages(self, entity, ids=None, limit=None):
        return self.message

    def of(self, kind):
        return [r for r in self.sent if isinstance(r, kind)]


@pytest.fixture
def wire(monkeypatch):
    holder = {}

    def use(**kwargs):
        holder["c"] = _Client(**kwargs)
        return holder["c"]

    async def _resolve(value, cl=None, account=None):
        return CHAT

    async def _connected(cl=None):
        return None

    async def _max(cl, account):
        return 12

    async def _upload(cl, entity, ctx, path, tool_name):
        return PHOTO, None

    for module in (poll_creation, poll_manage, polls):
        monkeypatch.setattr(module, "get_client", lambda account=None: holder["c"])
        monkeypatch.setattr(module, "resolve_entity", _resolve)
        monkeypatch.setattr(module, "ensure_connected", _connected)
    monkeypatch.setattr(poll_creation, "_poll_answers_max", _max)
    monkeypatch.setattr(poll_build, "upload_attachment", _upload)
    return use


def _sent_poll(client):
    (request,) = client.of(functions.messages.SendMediaRequest)
    return request, request.media, request.media.poll


# --- creating ---------------------------------------------------------------------


@pytest.mark.asyncio
async def test_every_setting_of_the_dialog_reaches_the_wire(wire):
    c = wire()
    await poll_creation.create_poll(
        "chat",
        "Ship?",
        ["Yes", "No", "Later"],
        multiple_choice=True,
        public_votes=True,
        allow_adding_options=True,
        allow_revoting=False,
        shuffle_options=True,
        hide_results_until_close=True,
        duration_seconds=3600,
        countries=["ir", "DE"],
        members_only=True,
    )
    _, _, poll = _sent_poll(c)
    assert poll.multiple_choice and poll.public_voters and poll.open_answers
    assert poll.revoting_disabled is True, "allow_revoting=False must set revoting_disabled"
    assert poll.shuffle_answers and poll.hide_results_until_close and poll.subscribers_only
    assert poll.close_period == 3600 and poll.close_date is None
    assert poll.countries_iso2 == ["IR", "DE"]


@pytest.mark.asyncio
async def test_defaults_leave_every_new_flag_off(wire):
    c = wire()
    await poll_creation.create_poll("chat", "Ship?", ["Yes", "No"])
    _, media, poll = _sent_poll(c)
    assert not poll.open_answers and not poll.revoting_disabled and not poll.shuffle_answers
    assert not poll.hide_results_until_close and not poll.subscribers_only
    assert poll.countries_iso2 is None and media.attached_media is None


@pytest.mark.asyncio
async def test_a_quiz_can_have_several_correct_answers_and_an_explanation(wire):
    c = wire()
    await poll_creation.create_poll(
        "chat",
        "Pick primes",
        ["2", "4", "5"],
        quiz_mode=True,
        correct_option_indexes=[0, 2],
        explanation="2 and 5 are prime",
        explanation_file="why.png",
    )
    _, media, poll = _sent_poll(c)
    assert poll.quiz and media.correct_answers == [0, 2]
    assert media.solution == "2 and 5 are prime" and media.solution_media is PHOTO


@pytest.mark.asyncio
async def test_description_and_attachments_ride_along(wire):
    c = wire()
    await poll_creation.create_poll(
        "chat",
        "Which logo?",
        ["A", "B"],
        description="Vote by Friday",
        description_file="brief.pdf",
        option_files=["a.png", None],
    )
    request, media, poll = _sent_poll(c)
    assert request.message == "Vote by Friday" and media.attached_media is PHOTO
    assert isinstance(poll.answers[0], tl.InputPollAnswer) and poll.answers[0].media is PHOTO
    assert poll.answers[1].media is None


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "kwargs, why",
    [
        ({"correct_option_indexes": [0]}, "quiz_mode"),
        ({"quiz_mode": True, "correct_option_indexes": [0, 5]}, "correct_option_indexes"),
        ({"quiz_mode": True, "correct_option_index": 0, "correct_option_indexes": [1]}, "both"),
        ({"explanation": "why"}, "quiz"),
        ({"quiz_mode": True, "correct_option_index": 0, "explanation": "x" * 201}, "200"),
        ({"duration_seconds": 60, "close_date": "2099-01-01 00:00:00"}, "duration_seconds"),
        ({"duration_seconds": 2}, "duration_seconds"),
        ({"countries": ["IRN"]}, "country"),
        ({"option_files": ["a", "b", "c"]}, "option_files"),
    ],
)
async def test_impossible_combinations_are_refused_before_sending(wire, kwargs, why):
    c = wire()
    text = await poll_creation.create_poll("chat", "Q?", ["A", "B"], **kwargs)
    assert c.of(functions.messages.SendMediaRequest) == [] and why in text


# --- managing ---------------------------------------------------------------------


@pytest.mark.asyncio
async def test_an_option_is_added_to_an_open_poll(wire):
    c = wire(message=_poll_message(open_answers=True))
    await poll_manage.add_poll_option("chat", 40, "Maybe", file_path="m.png")
    (request,) = c.of(functions.messages.AddPollAnswerRequest)
    assert request.msg_id == 40 and request.answer.text.text == "Maybe"
    assert request.answer.media is PHOTO


@pytest.mark.asyncio
async def test_adding_to_a_closed_list_poll_is_refused(wire):
    c = wire(message=_poll_message(open_answers=False))
    text = await poll_manage.add_poll_option("chat", 40, "Maybe")
    assert c.of(functions.messages.AddPollAnswerRequest) == [] and "open" in text.lower()


@pytest.mark.asyncio
async def test_an_option_is_deleted_by_its_index(wire):
    c = wire(message=_poll_message(answers=("Yes", "No", "Added")))
    await poll_manage.delete_poll_option("chat", 40, 2)
    (request,) = c.of(functions.messages.DeletePollAnswerRequest)
    assert request.option == bytes([2])


@pytest.mark.asyncio
async def test_unread_votes_are_listed_and_marked_read(wire):
    answer = tl.messages.Messages(messages=[], topics=[], chats=[], users=[])
    c = wire(answer=answer)
    await poll_manage.list_unread_poll_votes("chat")
    assert c.of(functions.messages.GetUnreadPollVotesRequest)
    c.answer = tl.messages.AffectedHistory(pts=1, pts_count=1, offset=0)
    await poll_manage.mark_poll_votes_read("chat")
    assert c.of(functions.messages.ReadPollVotesRequest)


@pytest.mark.asyncio
async def test_poll_statistics_are_requested_for_that_message(wire):
    graph = tl.StatsGraphError(error="not enough data")
    message = _poll_message()
    c = wire(message=message, answer=tl.stats.PollStats(votes_graph=graph))
    assert "can_view_stats" in await poll_manage.get_poll_statistics("chat", 40)
    assert c.of(functions.stats.GetPollStatsRequest) == []
    message.media.results.can_view_stats = True
    text = await poll_manage.get_poll_statistics("chat", 40)
    (request,) = c.of(functions.stats.GetPollStatsRequest)
    assert request.msg_id == 40 and "not enough data" in text


# --- reading and closing ------------------------------------------------------------


def test_reading_reports_every_setting():
    msg = _poll_message(
        open_answers=True,
        revoting_disabled=True,
        shuffle_answers=True,
        hide_results_until_close=True,
        subscribers_only=True,
        countries_iso2=["IR"],
        close_period=600,
    )
    described = polls._describe(msg.media.poll, msg.media.results)
    assert described["others_can_add_options"] is True
    assert described["revoting_allowed"] is False
    assert described["shuffled"] and described["results_hidden_until_close"]
    assert described["members_only"] and described["countries"] == ["IR"]
    assert described["duration_seconds"] == 600


def test_a_quiz_reports_every_correct_option():
    msg = _poll_message(answers=("2", "5"), quiz=True, correct=[0, 1])
    described = polls._describe(msg.media.poll, msg.media.results)
    assert described["correct_option_indexes"] == [0, 1]


def test_an_added_option_reports_who_added_it_and_when():
    msg = _poll_message()
    msg.media.poll.answers[1].added_by = tl.PeerUser(user_id=42)
    msg.media.poll.answers[1].date = datetime(2026, 9, 27, tzinfo=timezone.utc)
    described = polls._describe(msg.media.poll, msg.media.results)
    assert described["options"][1]["added_by_id"] == 42


@pytest.mark.asyncio
async def test_closing_keeps_the_settings_it_used_to_drop(wire):
    c = wire(
        message=_poll_message(open_answers=True, subscribers_only=True, countries_iso2=["IR"])
    )
    await polls.close_poll("chat", 40)
    (request,) = c.of(functions.messages.EditMessageRequest)
    poll = request.media.poll
    assert poll.closed and poll.open_answers and poll.subscribers_only
    assert poll.countries_iso2 == ["IR"]


# --- pointing at one option or one task -----------------------------------------------


@pytest.mark.asyncio
async def test_a_reply_can_point_at_one_option(wire):
    c = wire(message=_poll_message())
    await poll_manage.reply_to_part("chat", 40, "I pick this", option_index=1)
    (request,) = c.of(functions.messages.SendMessageRequest)
    assert request.reply_to.reply_to_msg_id == 40 and request.reply_to.poll_option == bytes([1])


@pytest.mark.asyncio
async def test_a_reply_can_point_at_one_task(wire):
    todo = tl.MessageMediaToDo(
        todo=tl.TodoList(title=_text("List"), list=[tl.TodoItem(id=3, title=_text("Ship"))])
    )
    c = wire(message=SimpleNamespace(id=40, media=todo))
    assert "does not exist" in await poll_manage.reply_to_part("chat", 40, "x", task_id=9)
    await poll_manage.reply_to_part("chat", 40, "Done", task_id=3)
    (request,) = c.of(functions.messages.SendMessageRequest)
    assert request.reply_to.todo_item_id == 3


@pytest.mark.asyncio
async def test_a_link_can_point_at_one_option(wire):
    c = wire(
        message=_poll_message(), answer=tl.ExportedMessageLink(link="https://t.me/x/40", html="")
    )
    text = await poll_manage.get_part_link("chat", 40, option_index=1)
    assert "https://t.me/x/40?option=AQ" in text
    assert c.of(functions.channels.ExportMessageLinkRequest)


@pytest.mark.asyncio
async def test_a_part_needs_exactly_one_of_option_or_task(wire):
    c = wire(message=_poll_message())
    text = await poll_manage.reply_to_part("chat", 40, "x", option_index=0, task_id=1)
    assert c.sent == [] and "one of" in text.lower()


def test_poll_text_is_parsed_with_the_servers_parser():
    parsed = poll_build.parse("**bold**", "md")
    assert parsed.text == "bold" and parsed.entities
    assert poll_build.parse("plain", None).entities == []


def test_the_description_and_explanation_limits_come_from_the_docs():
    assert json.dumps(poll_build.EXPLANATION_MAX) == "200"


@pytest.mark.asyncio
async def test_a_telegram_refusal_names_its_error_code(wire):
    from telethon.errors import RPCError

    wire(answer=RPCError(None, "POLL_ANSWERS_INVALID", 400))
    text = await poll_creation.create_poll("chat", "Q?", ["A", "B"])
    assert "POLL_ANSWERS_INVALID" in text and "Nothing was sent" in text


@pytest.mark.asyncio
async def test_several_correct_answers_make_the_quiz_multiple_choice(wire):
    c = wire()
    await poll_creation.create_poll(
        "chat", "Primes?", ["2", "4", "5"], quiz_mode=True, correct_option_indexes=[0, 2]
    )
    _, _, poll = _sent_poll(c)
    assert poll.multiple_choice and poll.revoting_disabled is True


@pytest.mark.asyncio
async def test_every_option_goes_out_as_an_input_answer(wire):
    # A quiz with self-chosen option bytes was refused live (BAD_REQUEST).
    c = wire()
    await poll_creation.create_poll(
        "chat", "Q?", ["A", "B"], quiz_mode=True, correct_option_index=1
    )
    _, _, poll = _sent_poll(c)
    assert all(isinstance(a, tl.InputPollAnswer) for a in poll.answers)


@pytest.mark.asyncio
async def test_closing_a_quiz_sends_index_answers_and_a_whole_explanation(wire):
    # Live 2026-09-27: closing a quiz with an explanation failed inside the encoder
    # (solution without solution_entities), and the answers went out as bytes.
    message = _poll_message(answers=("2", "4", "5"), quiz=True, correct=[0])
    message.media.results.results.append(
        tl.PollAnswerVoters(option=bytes([2]), voters=0, correct=True)
    )
    message.media.results.solution = "2 and 5 are prime"
    message.media.results.solution_entities = []
    c = wire(message=message)
    await polls.close_poll("chat", 40)
    (request,) = c.of(functions.messages.EditMessageRequest)
    assert request.media.correct_answers == [0, 2]
    bytes(request)  # serialises: the encoder accepts the explanation as sent
