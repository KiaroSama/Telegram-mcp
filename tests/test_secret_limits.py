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

Evidence for every verdict, so a future reader can check rather than trust:
`get_input_secret_message_entities` in TDLib's MessageEntity.cpp drops nine
kinds as empty `case ... break;` arms, two of them (blockquote and its
expandable form) with the `push_back` written and commented out behind a
layer-101 check. The layer constants are SecretChatLayer.h.
"""

import pytest

from telegram_mcp import secret_limits as sl


def _entity(type_name: str) -> dict:
    """One td_api textEntity, shaped as `parseTextEntities` returns it."""
    return {"@type": "textEntity", "offset": 0, "length": 4, "type": {"@type": type_name}}


def _kinds(dropped: list) -> set:
    return {item["kind"] for item in dropped}


# --------------------------------------------------------------------------
# The nine that never survive
# --------------------------------------------------------------------------

# Named individually rather than looped over a list imported from the module:
# a test that reads its expectations out of the code under test asserts only
# that the code equals itself.
ALWAYS_DROPPED = [
    "textEntityTypeCashtag",
    "textEntityTypeBotCommand",
    "textEntityTypePhoneNumber",
    "textEntityTypeBankCardNumber",
    "textEntityTypeMentionName",
    "textEntityTypeMediaTimestamp",
    "textEntityTypeDateTime",
    "textEntityTypeBlockQuote",
    "textEntityTypeExpandableBlockQuote",
]


@pytest.mark.parametrize("type_name", ALWAYS_DROPPED)
def test_the_nine_the_protocol_never_carries_are_dropped_at_every_layer(type_name):
    """Even at the current layer. These are not old-client problems - the
    protocol has no representation for them at all, and blockquote's is written
    into TDLib and deliberately commented out."""
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
        ("textEntityTypeUnderline", 101),
        ("textEntityTypeStrikethrough", 101),
        ("textEntityTypeSpoiler", 144),
        ("textEntityTypeCustomEmoji", 144),
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
        "textEntityTypeBold",
        "textEntityTypeItalic",
        "textEntityTypeCode",
        "textEntityTypePre",
        "textEntityTypePreCode",
        "textEntityTypeTextUrl",
        "textEntityTypeUrl",
        "textEntityTypeEmailAddress",
        "textEntityTypeMention",
        "textEntityTypeHashtag",
    ],
)
def test_the_formats_every_layer_carries_are_never_reported_as_dropped(type_name):
    """A false drop is cheap but not free: it teaches a caller to avoid
    formatting that works perfectly well."""
    assert sl.dropped_entities([_entity(type_name)], layer=73) == []


# --------------------------------------------------------------------------
# The rounding direction
# --------------------------------------------------------------------------


@pytest.mark.parametrize("unknown", [0, None])
def test_an_unknown_layer_rounds_down_to_the_protocol_floor(unknown):
    """The whole point of the report is that a caller can trust it. Treating an
    unreadable layer as current would make the report claim delivery it cannot
    know about, which is worse than useless - it is confidently wrong."""
    dropped = sl.dropped_entities([_entity("textEntityTypeSpoiler")], layer=unknown)

    assert _kinds(dropped) == {"spoiler"}, "an unknown layer was treated as current"
    assert dropped[0]["needs_layer"] == 144


def test_nothing_dropped_returns_an_empty_list_not_a_placeholder():
    """The caller's signal is the field's PRESENCE, so an empty result must be
    falsy rather than a list holding 'none'."""
    assert sl.dropped_entities([], layer=144) == []
    assert not sl.dropped_entities([_entity("textEntityTypeBold")], layer=144)


def test_every_dropped_entity_is_reported_once_per_occurrence():
    """Two spoilers in one message are two losses, and a caller rewriting the
    text needs to know it is not one."""
    two = [_entity("textEntityTypeSpoiler"), _entity("textEntityTypeSpoiler")]
    assert len(sl.dropped_entities(two, layer=73)) == 2


def test_an_entity_type_this_server_does_not_know_is_not_silently_passed():
    """TDLib gains entity types; this table does not update itself. An unknown
    type is reported as unknown rather than assumed safe, because assuming safe
    is how a new droppable format becomes a silent loss again."""
    dropped = sl.dropped_entities([_entity("textEntityTypeSomethingNewIn2027")], layer=144)

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


class _Client:
    """Answers getChat/getSecretChat from a scripted state, recording calls."""

    def __init__(self, state, chat_type="chatTypeSecret"):
        self.state = state
        self.chat_type = chat_type
        self.requests = []

    async def request(self, obj, timeout=30.0):
        self.requests.append(obj)
        if obj["@type"] == "getChat":
            return {"id": obj["chat_id"], "type": {"@type": self.chat_type, "secret_chat_id": 7}}
        if obj["@type"] == "getSecretChat":
            return {"@type": "secretChat", "state": {"@type": self.state}, "layer": 144}
        raise AssertionError(f"guard sent {obj['@type']}, which it had no business sending")


@pytest.mark.asyncio
async def test_a_ready_chat_passes_the_guard():
    client = _Client("secretChatStateReady")
    assert await sl.require_ready_chat(client, 1) is None


@pytest.mark.asyncio
async def test_a_pending_chat_is_refused_by_name():
    """The key exchange has not finished. A send here fails inside the protocol
    with something the caller cannot act on, so it is stopped here instead."""
    client = _Client("secretChatStatePending")

    refusal = await sl.require_ready_chat(client, 1)

    assert refusal and "pending" in refusal.lower()
    assert "not" in refusal.lower()


@pytest.mark.asyncio
async def test_a_closed_chat_is_refused_and_told_it_cannot_reopen():
    """The key is discarded on both sides. A caller who does not know that will
    retry forever."""
    client = _Client("secretChatStateClosed")

    refusal = await sl.require_ready_chat(client, 1)

    assert refusal and "closed" in refusal.lower()
    assert "reopen" in refusal.lower() or "cannot be reopened" in refusal.lower()


@pytest.mark.asyncio
async def test_a_chat_that_is_not_secret_at_all_is_refused():
    """An ordinary chat id reaching a secret-chat tool is a caller mistake worth
    naming, not a request to send an unencrypted message."""
    client = _Client("secretChatStateReady", chat_type="chatTypePrivate")

    refusal = await sl.require_ready_chat(client, 1)

    assert refusal and "secret" in refusal.lower()
    assert "getSecretChat" not in [r["@type"] for r in client.requests]


@pytest.mark.asyncio
async def test_the_layer_comes_back_with_the_verdict_so_callers_need_one_round_trip():
    """Formatting needs the layer and sending needs the state, and both come
    from the same two calls. Fetching them twice would double every send."""
    client = _Client("secretChatStateReady")

    assert await sl.secret_chat_layer(client, 1) == 144
