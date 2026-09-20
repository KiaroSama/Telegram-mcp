"""The media kind vocabulary, with no backend attached.

`KINDS` and the family table lived inside `secret_media_content.py`, which exists to
build TDLib request bodies and is scheduled for deletion with TDLib. Deleting it
would have taken the only description of a complete media surface with it, so the
vocabulary moved out first. `docs/adr/0003-the-media-kind-vocabulary-outlives-tdlib.md`
records why.

These cases are about the vocabulary itself. What each kind means on the wire is the
backend's business and is tested where that backend lives.
"""

import inspect

from telegram_mcp import media_kinds


def test_there_are_exactly_the_eight_kinds_the_protocol_carries():
    """Eight is not a round number here - it is what the encrypted protocol carries,
    and the parity test asserts against this tuple."""
    assert media_kinds.KINDS == (
        "photo",
        "video",
        "document",
        "audio",
        "animation",
        "sticker",
        "video_note",
        "voice_note",
    )


def test_the_two_kinds_with_no_caption_field_are_named():
    assert media_kinds.NO_CAPTION == frozenset({"sticker", "video_note"})
    assert media_kinds.NO_CAPTION <= set(media_kinds.KINDS)


def test_every_family_allows_its_own_default():
    """A default outside its own allowed set would make `infer_kind` produce a kind
    the very next validation rejects."""
    for name, family in media_kinds.FAMILIES.items():
        assert family["default"] in family["allows"], f"{name}'s default is not allowed"
        assert family["allows"] <= set(media_kinds.KINDS), f"{name} allows a kind that is not one"


def test_an_unrecognised_extension_infers_the_kind_that_carries_anything():
    """Guessing a specific kind at an unknown extension fails at the protocol;
    `document` carries any bytes at all."""
    assert media_kinds.infer_kind("mystery.qqq") == "document"
    assert media_kinds.infer_kind("no-extension-at-all") == "document"


def test_ogg_infers_a_voice_note_and_mp3_infers_audio():
    """Telegram's voice format defaults to a voice note - the commoner intent by a
    wide margin - while music defaults to music. Both remain reachable as the other,
    because both are inside the same family."""
    assert media_kinds.infer_kind("recording.ogg") == "voice_note"
    assert media_kinds.infer_kind("song.mp3") == "audio"
    assert "audio" in media_kinds.family_of("recording.ogg")["allows"]
    # The reverse is NOT true, and deliberately: an mp3 is not a voice message.
    # Only Telegram's own OGG/Opus container carries both readings.
    assert "voice_note" not in media_kinds.family_of("song.mp3")["allows"]


def test_every_family_allows_document():
    """ "Send as file" is the one choice available for anything."""
    for name, family in media_kinds.FAMILIES.items():
        assert "document" in family["allows"], f"{name} cannot be sent as a file"


def test_an_unknown_path_has_no_family_rather_than_a_wrong_one():
    assert media_kinds.family_of("mystery.qqq") is None


def test_the_vocabulary_names_no_backend():
    """The whole point of the move. A TDLib identifier here would put the
    vocabulary back on the thing it has to outlive.

    Code only - the docstring names the ADR, whose filename ends in `tdlib`, and
    that reference is the reason the move happened rather than a dependency on it.
    Checking the whole source made the test fail for citing its own rationale.
    """
    source = inspect.getsource(media_kinds)
    body = source.split('"""', 2)[-1]  # past the module docstring
    code = " ".join(line for line in body.splitlines() if not line.lstrip().startswith("#"))

    for forbidden in ("tdlib", "td_api", "inputMessage", "inputPhoto", "telethon"):
        assert forbidden not in code, f"the vocabulary reaches for {forbidden}"

    # Its only import, and the claim is that it stays that way. `pathlib` is the
    # standard library; anything else here would be a backend arriving sideways.
    imports = [line.strip() for line in body.splitlines() if line.startswith(("import ", "from "))]
    assert imports == ["from pathlib import Path"], f"the vocabulary grew imports: {imports}"
