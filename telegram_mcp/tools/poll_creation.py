"""Creating a poll or a quiz, with the limits Telegram enforces checked first.

Split out of ``messages_state`` when it passed the 800-line ceiling; the tools that
read, vote in and close a poll are in ``polls``.
"""

from telegram_mcp.runtime import *
from telegram_mcp.message_view import display_text
from telegram_mcp.effect_catalog import account_key
from telethon.errors import RPCError

from telegram_mcp.tools import poll_build

# Telegram's own limits for a poll. Checked here so an over-long question comes
# back as an argument error rather than an RPC refusal after the round trip.
_POLL_QUESTION_LIMIT = 255


_POLL_OPTION_LIMIT = 100


# `poll.close_date` is a unix timestamp, and Telegram takes it only inside this
# window -- 5 seconds to about 30 days. Only "is it in the future" was checked
# here, so a 100-day deadline made the whole round trip to be refused on the
# wire. https://core.telegram.org/constructor/poll
_POLL_CLOSE_MIN_SECONDS = 5


_POLL_CLOSE_MAX_SECONDS = 2_628_000


# Telegram measures that 5-second floor against ITS clock at the moment the
# request lands, and everything between the check and the landing costs time:
# resolving the chat is a round trip of its own, then the Poll is built, the
# InputMediaPoll serialised, and the whole thing put on the wire. A deadline that
# was legal when parsed could therefore be under the floor on arrival, and the
# poll came back refused AFTER the send. This is the slack that keeps a deadline
# accepted here acceptable there; it is deliberately small, because it is only
# covering serialisation and one hop, not user latency.
_POLL_CLOSE_SEND_MARGIN_SECONDS = 2


# The earliest close_date this server will send, floor plus slack. Public so a
# test can pin the boundary to the rule rather than to a copied number.
EARLIEST_POLL_CLOSE_SECONDS = _POLL_CLOSE_MIN_SECONDS + _POLL_CLOSE_SEND_MARGIN_SECONDS


def _close_date_problem(close_date_obj) -> Optional[str]:
    """Why this deadline cannot be sent right now, or ``None``.

    Called twice on purpose: once from the arguments alone, so an impossible date
    costs nothing, and once immediately before the request is built, because by
    then the clock has moved and the first answer may no longer be true.
    """
    seconds = (close_date_obj - datetime.now(close_date_obj.tzinfo)).total_seconds()
    if seconds <= 0:
        return (
            "Error: close_date is in the past; a poll cannot close before it opens. "
            "Pick a later close_date. Nothing was sent."
        )
    if seconds < EARLIEST_POLL_CLOSE_SECONDS:
        return (
            f"Error: close_date is {seconds:.0f} seconds away. Telegram requires at least "
            f"{_POLL_CLOSE_MIN_SECONDS} seconds measured when the request reaches it, and "
            f"sending this one takes time it no longer has, so this server needs "
            f"{EARLIEST_POLL_CLOSE_SECONDS} seconds. Pick a later close_date. Nothing was sent."
        )
    if seconds > _POLL_CLOSE_MAX_SECONDS:
        return (
            f"Error: close_date is {seconds:.0f} seconds away, and Telegram accepts "
            f"{_POLL_CLOSE_MIN_SECONDS} to {_POLL_CLOSE_MAX_SECONDS} seconds "
            "(5 seconds to about 30 days). Nothing was sent."
        )
    return None


# How many answers a poll may carry. Telegram publishes this in the client
# config as `poll_answers_max`, and a client is expected to read it there rather
# than assume: the number written into this file was 10 and the real one has
# been 12 for some time, so every 11- and 12-option poll was refused locally and
# blamed on the caller. The constant below is only what a client that cannot
# reach the config must fall back to, and it is the current documented value.
# https://core.telegram.org/api/config#poll-answers-max
_POLL_ANSWERS_MAX_FALLBACK = 12


_POLL_ANSWERS_MIN = 2


_APP_CONFIG_TIMEOUT_SECONDS = 10.0


# One lookup per account label per process. The value changes on Telegram's
# schedule, not within a call, and an unbounded config request per poll is a
# round trip bought for nothing.
_poll_answers_max_cache: dict = {}


async def _poll_answers_max(cl, account) -> int:
    """Telegram's current ceiling on poll options, asked once per account.

    A config that cannot be read is not a reason to refuse the poll, so any
    failure -- including the timeout that bounds the request -- falls back to the
    documented current value rather than to no limit or to a hang.
    """
    label = account_key(account)
    cached = _poll_answers_max_cache.get(label)
    if cached is not None:
        return cached

    value = _POLL_ANSWERS_MAX_FALLBACK
    try:
        # create_poll otherwise reaches the wire for the first time inside
        # resolve_entity, which connects on the way. Asking for the config before
        # that would fail on a cold client and quietly fall back every time.
        await ensure_connected(cl)
        config = await asyncio.wait_for(
            cl(functions.help.GetAppConfigRequest(hash=0)),
            timeout=_APP_CONFIG_TIMEOUT_SECONDS,
        )
        for entry in getattr(getattr(config, "config", None), "value", None) or []:
            if getattr(entry, "key", None) != "poll_answers_max":
                continue
            # help.appConfig carries a JsonObject, so the number arrives as a
            # JsonNumber whose `value` is a float.
            published = int(getattr(getattr(entry, "value", None), "value", 0) or 0)
            if published >= _POLL_ANSWERS_MIN:
                value = published
            break
    except Exception as error:
        log_event(
            logging.DEBUG,
            "poll_answers_max lookup failed; using the fallback",
            error=error,
            fallback=value,
        )

    _poll_answers_max_cache[label] = value
    return value


def _settings_problem(
    options,
    quiz_mode,
    multiple_choice,
    correct_option_index,
    correct_option_indexes,
    explanation,
    explanation_file,
    duration_seconds,
    close_date,
    countries,
    option_files,
    allow_revoting=None,
    allow_adding_options=False,
) -> Optional[str]:
    """Why these settings cannot make one poll, or ``None``. Decided from the arguments alone."""
    if correct_option_index is not None and correct_option_indexes is not None:
        return "Error: pass correct_option_index or correct_option_indexes, not both."
    name = (
        "correct_option_indexes" if correct_option_indexes is not None else "correct_option_index"
    )
    correct = (
        [correct_option_index]
        if correct_option_index is not None
        else list(correct_option_indexes or [])
    )

    if quiz_mode:
        # Measured 2026-09-27: Telegram refuses (BAD_REQUEST) a quiz whose answer can be
        # changed, and a quiz takes no added options.
        if allow_revoting:
            return "Error: a quiz never lets a voter change the answer; drop allow_revoting."
        if allow_adding_options:
            return "Error: a quiz takes no added options; drop allow_adding_options."
        if not correct:
            return (
                f"Error: quiz_mode needs {name}. Without it Telegram has "
                "no correct answer to grade against and marks every voter wrong."
            )
        for index in correct:
            if not isinstance(index, int) or isinstance(index, bool):
                return f"Error: {name} must be integers."
            if not 0 <= index < len(options):
                return (
                    f"Error: {name} {index} is not one of the "
                    f"options. Valid indexes are 0-{len(options) - 1}."
                )
    elif correct:
        return f"Error: {name} only applies to a quiz. Pass quiz_mode=True."

    if (explanation or explanation_file) and not quiz_mode:
        return (
            "Error: an explanation is shown after a quiz is answered, so it needs quiz_mode=True."
        )
    if explanation and len(explanation) > poll_build.EXPLANATION_MAX:
        return f"Error: the explanation is limited to {poll_build.EXPLANATION_MAX} characters."

    if duration_seconds is not None:
        if close_date:
            return "Error: pass duration_seconds or close_date, not both; Telegram takes one."
        if not _POLL_CLOSE_MIN_SECONDS <= int(duration_seconds) <= _POLL_CLOSE_MAX_SECONDS:
            return (
                f"Error: duration_seconds must be {_POLL_CLOSE_MIN_SECONDS} to "
                f"{_POLL_CLOSE_MAX_SECONDS} (about 30 days). Nothing was sent."
            )
    if countries:
        problem = poll_build.countries_problem(countries)
        if problem:
            return problem
    if option_files is not None and len(option_files) != len(options):
        return (
            f"Error: option_files needs one entry per option ({len(options)}), "
            "with null for an option without a file."
        )
    return None


@mcp.tool(
    annotations=ToolAnnotations(
        title="Create Poll",
        openWorldHint=True,
        destructiveHint=True,
        readOnlyHint=False,
        idempotentHint=False,
    )
)
@with_account(readonly=False)
@validate_id("chat_id")
async def create_poll(
    chat_id: Union[int, str],
    question: str,
    options: list,
    multiple_choice: bool = False,
    quiz_mode: bool = False,
    public_votes: bool = True,
    close_date: str = None,
    correct_option_index: Optional[int] = None,
    correct_option_indexes: Optional[list] = None,
    explanation: Optional[str] = None,
    explanation_file: Optional[str] = None,
    description: Optional[str] = None,
    description_file: Optional[str] = None,
    option_files: Optional[list] = None,
    allow_adding_options: bool = False,
    allow_revoting: Optional[bool] = None,
    shuffle_options: bool = False,
    hide_results_until_close: bool = False,
    duration_seconds: Optional[int] = None,
    countries: Optional[list] = None,
    members_only: bool = False,
    parse_mode: Optional[str] = None,
    account: str = None,
    ctx: Optional[Context] = None,
) -> str:
    """
    Create a poll or a quiz, with every setting of Telegram's poll dialog.

    Args:
        chat_id: The ID of the chat to send the poll to
        question: The poll question (1-255 characters)
        options: List of answer options, 1-100 characters each. At least 2, and at
            most as many as Telegram's published `poll_answers_max` allows (12 at
            the time of writing); an over-long list is refused before sending.
        multiple_choice: Whether users can select multiple answers
        quiz_mode: Whether this is a quiz. A quiz is graded, so it REQUIRES
            correct_option_index or correct_option_indexes; its answer can never be
            changed and it takes no added options.
        public_votes: Whether votes are public ("show who voted")
        close_date: Optional close date in ISO format (YYYY-MM-DD HH:MM:SS). It
            must fall in Telegram's window — at least 5 seconds and at most
            2,628,000 seconds (about 30 days) — measured on Telegram's clock when
            the request arrives, not on this one when you call. It is therefore
            checked twice, and a deadline still in the future but too close to
            survive the send is refused here rather than by Telegram afterwards.
        correct_option_index: Zero-based index of the one correct answer of a quiz.
        correct_option_indexes: Several correct answers of a quiz, by index. Pass
            this or correct_option_index, not both.
        explanation: Quiz only: text shown after answering (0-200 characters).
        explanation_file: Quiz only: a local photo or file shown with the explanation.
        description: Text sent with the poll, above it.
        description_file: A local photo or file attached to the poll.
        option_files: One local path (or null) per option, attached to that option.
        allow_adding_options: Voters may add their own options.
        allow_revoting: Voters may change their vote (default: yes for a poll; a quiz
            never allows it).
        shuffle_options: Each voter sees the options in a random order.
        hide_results_until_close: Nobody sees the results until the poll closes.
        duration_seconds: Close the poll this many seconds after it is posted
            (5 to about 30 days). Use this or close_date, not both.
        countries: Only voters whose phone number is from these countries may vote
            (two-letter codes, e.g. ["IR", "DE"]).
        members_only: Only members who joined at least 24 hours before the poll
            was posted may vote.
        parse_mode: 'md' or 'html' for the question, options, description and
            explanation; unset sends them as written.
    """
    try:
        cl = get_client(account)

        # Everything below is settled before a chat is resolved or a poll is
        # sent: a quiz Telegram cannot grade must not reach the chat and then
        # need deleting. The one thing not decided from the arguments alone is
        # the option ceiling, which is read from Telegram's published config
        # rather than guessed at.
        if not str(question).strip():
            return "Error: The poll question cannot be empty."
        if len(question) > _POLL_QUESTION_LIMIT:
            return f"Error: The poll question is limited to {_POLL_QUESTION_LIMIT} characters."
        if len(options) < _POLL_ANSWERS_MIN:
            return f"Error: Poll must have at least {_POLL_ANSWERS_MIN} options."
        answers_max = await _poll_answers_max(cl, account)
        if len(options) > answers_max:
            return f"Error: Poll can have at most {answers_max} options."
        for index, option in enumerate(options):
            if not str(option).strip():
                return f"Error: Poll option {index} is empty."
            if len(option) > _POLL_OPTION_LIMIT:
                return (
                    f"Error: Poll option {index} exceeds the "
                    f"{_POLL_OPTION_LIMIT}-character limit."
                )

        problem = _settings_problem(
            options,
            quiz_mode,
            multiple_choice,
            correct_option_index,
            correct_option_indexes,
            explanation,
            explanation_file,
            duration_seconds,
            close_date,
            countries,
            option_files,
            allow_revoting,
            allow_adding_options,
        )
        if problem:
            return problem
        correct = (
            [correct_option_index]
            if correct_option_index is not None
            else list(correct_option_indexes or [])
        )

        # Parse close date if provided
        close_date_obj = None
        if close_date:
            try:
                close_date_obj = datetime.fromisoformat(close_date.replace("Z", "+00:00"))
            except ValueError:
                return "Invalid close_date format. Use YYYY-MM-DD HH:MM:SS format."
            problem = _close_date_problem(close_date_obj)
            if problem:
                return problem

        entity = await resolve_entity(chat_id, cl)

        async def attach(path):
            if not path:
                return None, None
            return await poll_build.upload_attachment(cl, entity, ctx, path, "create_poll")

        attached, error = await attach(description_file)
        if error:
            return error
        solution_media, error = await attach(explanation_file)
        if error:
            return error
        option_media = []
        for path in option_files or []:
            media, error = await attach(path)
            if error:
                return error
            option_media.append(media)

        # Again, now that resolving the chat and uploading have been paid for. The
        # first check answered a question about a clock that has since moved; this
        # one answers it about the request that is actually about to go out.
        if close_date_obj is not None:
            problem = _close_date_problem(close_date_obj)
            if problem:
                return problem

        import random

        # Every option goes out as InputPollAnswer and Telegram picks the option bytes.
        # Measured 2026-09-27: a quiz sent with self-chosen bytes (PollAnswer) is refused
        # with BAD_REQUEST even when every other field is right.
        media_of = option_media or [None] * len(options)
        answers = [
            types.InputPollAnswer(text=poll_build.parse(option, parse_mode), media=media)
            for option, media in zip(options, media_of)
        ]
        poll = types.Poll(
            id=random.randint(0, 2**63 - 1),
            question=poll_build.parse(question, parse_mode),
            answers=answers,
            # Telethon 1.44 made `hash` a required argument on Poll. It caches
            # server-side results, so a poll being created sends 0.
            hash=0,
            # Several correct answers make a quiz multiple choice, as Telegram's own apps do.
            multiple_choice=multiple_choice or len(correct) > 1,
            quiz=quiz_mode,
            public_voters=public_votes,
            close_date=close_date_obj,
            close_period=int(duration_seconds) if duration_seconds is not None else None,
            open_answers=allow_adding_options or None,
            revoting_disabled=True if quiz_mode else (allow_revoting is False) or None,
            shuffle_answers=shuffle_options or None,
            hide_results_until_close=hide_results_until_close or None,
            subscribers_only=members_only or None,
            countries_iso2=[str(c).upper() for c in countries] if countries else None,
        )

        # ponytail: Telethon 1.44 declares and serialises `correct_answers` as
        # Vector<int> while the published schema calls it Vector<bytes>; handing it
        # the answer's `option` blob raises struct.error, so the index goes out as
        # the int the installed library asks for. If a live quiz ever grades the
        # wrong answer, the upgrade path is a project-local InputMediaPoll wire
        # class next to the ones in tools/topics.py.
        media = types.InputMediaPoll(poll=poll, attached_media=attached)
        if quiz_mode:
            media.correct_answers = [int(index) for index in correct]
            if explanation:
                solution = poll_build.parse(explanation, parse_mode)
                media.solution = solution.text
                media.solution_entities = solution.entities
            media.solution_media = solution_media

        caption = poll_build.parse(description or "", parse_mode)
        result = await cl(
            functions.messages.SendMediaRequest(
                peer=entity,
                media=media,
                message=caption.text,
                entities=caption.entities or None,
                random_id=random.randint(0, 2**63 - 1),
            )
        )

        # SendMedia answers with an Updates; the new message is the one carrying
        # the poll. Without its id the caller cannot read the results back, vote,
        # or take the poll down again.
        message_id = None
        for update in getattr(result, "updates", None) or []:
            candidate = getattr(update, "message", None)
            if candidate is not None and getattr(candidate, "id", None):
                message_id = candidate.id
                break
            if getattr(update, "id", None) and getattr(update, "poll_id", None) is None:
                message_id = update.id

        return format_tool_result(
            [{"message_id": message_id, "question": display_text(question)}],
            {"chat_id": str(chat_id), "created": True},
        )
    except RPCError as e:
        if type(e).__name__.startswith("FloodWait"):
            return log_and_format_error("create_poll", e, chat_id=chat_id)
        # Telegram's error code names the setting it refused and carries no user text.
        return f"Telegram refused the poll: {e.message}. Nothing was sent."
    except Exception as e:
        # The question and its options are user-supplied text. They identify
        # nothing a reader of the log needs and they are exactly what a failure
        # report must not copy, so only the shape of the poll goes out.
        return log_and_format_error("create_poll", e, chat_id=chat_id, option_count=len(options))


__all__ = ["create_poll"]
