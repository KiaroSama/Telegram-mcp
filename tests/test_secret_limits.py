"""What a secret chat cannot do, and whether we say so.

Two jobs, one module under test. The entity gate decides which formatting
survives a given chat, and the capability table decides what the status tool
reports as impossible. Both are the same kind of claim -- a limit -- and both
fail the same way if they are wrong: silently, in the direction of promising
more than the protocol delivers.

The load-bearing assertion here is the rounding direction. A chat whose layer
this server could not read is treated as the protocol FLOOR, not as current,
because reporting a format as delivered when it was dropped is the failure this
whole feature exists to remove; reporting one as dropped when it survived costs
a caller one redundant re-send.

The entities are Telethon's, because that is what the sender parses into since
the backend swap. Two rows this suite used to carry are GONE rather than renamed:
media timestamp and formatted date were synthesised by the previous client and have
no MTProto entity at all, so nine "never carried" became seven. Expandable
blockquote is `MessageEntityBlockquote` with `collapsed`, one class and two
published kinds.
"""

import pytest
from telethon.tl import types

from telegram_mcp import secret_limits as sl


def _entity(type_name: str, **kwargs):
    """One Telethon entity, as the client's own parser returns it.

    Built by name so an unknown type can be exercised too: the table does not
    update itself, and "reported as unknown" is a behaviour with its own test.
    """
    factory = getattr(types, type_name, None)
    if factory is None:
        return type(type_name, (), {})()
    # Three of them require a field beyond offset/length, and a constructor that
    # raises would fail the test for the wrong reason.
    required = {
        "MessageEntityCustomEmoji": {"document_id": 5},
        "MessageEntityPre": {"language": ""},
        "MessageEntityTextUrl": {"url": "https://example.invalid"},
    }
    return factory(offset=0, length=4, **{**required.get(type_name, {}), **kwargs})


def _kinds(dropped: list) -> set:
    return {item["kind"] for item in dropped}


# --------------------------------------------------------------------------
# The ones that never survive
# --------------------------------------------------------------------------

# Named individually rather than looped over a list imported from the module:
# a test that reads its expectations out of the code under test asserts only
# that the code equals itself.
ALWAYS_DROPPED = [
    "MessageEntityCashtag",
    "MessageEntityBotCommand",
    "MessageEntityPhone",
    "MessageEntityBankCard",
    "MessageEntityBlockquote",
]


@pytest.mark.parametrize("type_name", ALWAYS_DROPPED)
def test_the_formats_the_protocol_never_carries_are_dropped_at_every_layer(type_name):
    """Even at the current layer. These are not old-client problems - the
    encrypted protocol has no representation for them at all, and blockquote's is
    written into every client and deliberately disabled."""
    dropped = sl.dropped_entities([_entity(type_name)], layer=sl.CURRENT_LAYER)

    assert len(dropped) == 1, f"{type_name} survived a chat that cannot carry it"
    assert dropped[0]["kind"]
    assert "protocol" in dropped[0]["reason"], "blamed the other side's app for a protocol gap"
    assert "needs_layer" not in dropped[0], "offered a layer that would fix an unfixable drop"


# --------------------------------------------------------------------------
# The four that depend on how old the other side's app is
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    "type_name,floor",
    [
        ("MessageEntityUnderline", 101),
        ("MessageEntityStrike", 101),
        ("MessageEntitySpoiler", 144),
        ("MessageEntityCustomEmoji", 144),
    ],
)
def test_a_layer_gated_format_is_dropped_below_its_floor_and_kept_at_it(type_name, floor):
    below = sl.dropped_entities([_entity(type_name)], layer=floor - 1)
    assert len(below) == 1, f"{type_name} claimed to survive below layer {floor}"
    assert below[0]["needs_layer"] == floor
    assert "app" in below[0]["reason"], "blamed the protocol for an old-client problem"

    at = sl.dropped_entities([_entity(type_name)], layer=floor)
    assert at == [], f"{type_name} reported as dropped at its own floor"


@pytest.mark.parametrize(
    "type_name",
    [
        "MessageEntityBold",
        "MessageEntityItalic",
        "MessageEntityCode",
        "MessageEntityPre",
        "MessageEntityTextUrl",
        "MessageEntityUrl",
        "MessageEntityEmail",
        "MessageEntityMention",
        "MessageEntityHashtag",
    ],
)
def test_the_formats_every_layer_carries_are_never_reported_as_dropped(type_name):
    """A false drop is cheap but not free: it teaches a caller to avoid
    formatting that works perfectly well."""
    assert sl.dropped_entities([_entity(type_name)], layer=73) == []


def test_an_expandable_blockquote_is_named_apart_from_an_ordinary_one():
    """One Telethon class, two published kinds. A caller rewriting the message is
    rewriting a different thing in each case, so collapsing them would lose that."""
    plain = sl.dropped_entities([_entity("MessageEntityBlockquote")], layer=144)
    collapsed = sl.dropped_entities(
        [_entity("MessageEntityBlockquote", collapsed=True)], layer=144
    )

    assert _kinds(plain) == {"blockquote"}
    assert _kinds(collapsed) == {"expandable blockquote"}


# --------------------------------------------------------------------------
# The rounding direction
# --------------------------------------------------------------------------


@pytest.mark.parametrize("unknown", [0, None])
def test_an_unknown_layer_rounds_down_to_the_protocol_floor(unknown):
    """The whole point of the report is that a caller can trust it. Treating an
    unreadable layer as current would make the report claim delivery it cannot
    know about, which is worse than useless - it is confidently wrong."""
    dropped = sl.dropped_entities([_entity("MessageEntitySpoiler")], layer=unknown)

    assert _kinds(dropped) == {"spoiler"}, "an unknown layer was treated as current"
    assert dropped[0]["needs_layer"] == 144


def test_nothing_dropped_returns_an_empty_list_not_a_placeholder():
    """The caller's signal is the field's PRESENCE, so an empty result must be
    falsy rather than a list holding 'none'."""
    assert sl.dropped_entities([], layer=144) == []
    assert not sl.dropped_entities([_entity("MessageEntityBold")], layer=144)


def test_every_dropped_entity_is_reported_once_per_occurrence():
    """Two spoilers in one message are two losses, and a caller rewriting the
    text needs to know it is not one."""
    two = [_entity("MessageEntitySpoiler"), _entity("MessageEntitySpoiler")]
    assert len(sl.dropped_entities(two, layer=73)) == 2


def test_an_entity_type_this_server_does_not_know_is_not_silently_passed():
    """Telegram gains entity types; this table does not update itself. An unknown
    type is reported as unknown rather than assumed safe, because assuming safe
    is how a new droppable format becomes a silent loss again."""
    dropped = sl.dropped_entities([_entity("MessageEntitySomethingNewIn2027")], layer=144)

    assert len(dropped) == 1
    assert "unknown" in dropped[0]["reason"].lower()


# --------------------------------------------------------------------------
# The capability table
# --------------------------------------------------------------------------


def test_every_impossible_capability_carries_a_real_reason():
    """'Not supported' is not a reason - it is the absence of one, and it is
    exactly what a caller cannot act on. Every impossible verdict must cite the
    protocol gap or the refusal Telegram itself emits."""
    impossible = [c for c in sl.CAPABILITIES if c["verdict"] == "impossible"]

    assert impossible, "the table claims a secret chat can do everything"
    for capability in impossible:
        note = capability.get("note", "")
        assert note, f"{capability['operation']} is impossible for no stated reason"
        assert note.lower() != "not supported"
        assert len(note) > 20, f"{capability['operation']}'s reason is too thin to act on"


def test_every_available_capability_names_the_tool_that_does_it():
    """A report saying a thing is possible, without saying how, sends the caller
    hunting through a 200-tool list."""
    for capability in sl.CAPABILITIES:
        if capability["verdict"] in ("available", "differs"):
            assert capability.get("tool"), f"{capability['operation']} names no tool"


def test_the_table_covers_the_operations_the_owner_would_actually_try():
    """The report is only honest if it is complete for the things a person does
    in a chat. These six are the ones an agent reaches for and cannot have."""
    operations = " ".join(c["operation"].lower() for c in sl.CAPABILITIES)

    for expected in ("react", "edit", "pin", "schedul", "forward", "poll"):
        assert expected in operations, f"no verdict for {expected}"


def test_no_capability_carries_a_verdict_outside_the_three():
    for capability in sl.CAPABILITIES:
        assert capability["verdict"] in ("available", "differs", "impossible")


# --------------------------------------------------------------------------
# The readiness guard
# --------------------------------------------------------------------------


class _Chat:
    """One chat in the shape the guard reads it: a state with a value, and a layer."""

    def __init__(self, state, layer=144):
        self.state = type("_State", (), {"value": state})()
        self.layer = layer


class _Manager:
    """Answers `status` from a scripted chat, or raises what the real one raises."""

    def __init__(self, chat=None):
        self.chat = chat

    def status(self, chat_id):
        if self.chat is None:
            raise KeyError(f"no secret chat {chat_id} in this manager")
        return self.chat


def test_a_ready_chat_passes_the_guard():
    assert sl.require_ready_chat(_Manager(_Chat("ready")), 1) is None


def test_a_rekeying_chat_passes_too():
    """A routine key rotation holds two keys and sends under whichever settles.
    Refusing here would make it read as an outage."""
    assert sl.require_ready_chat(_Manager(_Chat("rekeying")), 1) is None


@pytest.mark.parametrize("state", ["requested", "pending"])
def test_an_unfinished_exchange_is_refused_by_name(state):
    """A send here fails inside the protocol with something the caller cannot act
    on, so it is stopped here instead - and told what clears it."""
    refusal = sl.require_ready_chat(_Manager(_Chat(state)), 1)

    assert refusal and state in refusal.lower()
    assert "other side" in refusal.lower()


def test_a_closed_chat_is_refused_and_told_it_cannot_reopen():
    """The key is discarded on both sides. A caller who does not know that will
    retry forever."""
    refusal = sl.require_ready_chat(_Manager(_Chat("closed")), 1)

    assert refusal and "closed" in refusal.lower()
    assert "cannot be reopened" in refusal.lower()


def test_a_chat_this_login_does_not_have_is_refused_by_name():
    """Secret chats are per-device, so one on the account's phone is invisible
    here and always will be. That is the sentence the caller needs."""
    refusal = sl.require_ready_chat(_Manager(None), 1)

    assert refusal and "per-device" in refusal.lower()


def test_a_state_this_server_does_not_recognise_is_reported_as_unknown():
    """Principle III: an unknown outcome is reported as unknown, never guessed
    at in the direction that lets the send through."""
    refusal = sl.require_ready_chat(_Manager(_Chat("teleporting")), 1)

    assert refusal and "unknown" in refusal.lower()


@pytest.mark.asyncio
async def test_the_layer_is_read_from_the_live_chat():
    assert await sl.secret_chat_layer(_Manager(_Chat("ready", layer=144)), 1) == 144


@pytest.mark.asyncio
async def test_a_layer_still_at_the_initial_value_rounds_down_rather_than_up():
    """Measured on a real chat: two sides reported 46 and 144 seconds apart,
    because the layer is the PEER's capability and rises only when they announce
    it. So the initial value means NOT YET KNOWN, and rounding down costs one
    redundant re-send where rounding up would claim a delivery that did not
    happen."""
    assert await sl.secret_chat_layer(_Manager(_Chat("ready", layer=46)), 1) == sl.FLOOR_LAYER


@pytest.mark.asyncio
async def test_a_chat_that_cannot_be_read_does_not_fail_the_send():
    """A formatting report is a courtesy attached to a send. It must not be the
    reason the send fails."""
    assert await sl.secret_chat_layer(_Manager(None), 1) == sl.FLOOR_LAYER
