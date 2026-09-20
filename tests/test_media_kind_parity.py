"""Kind parity: every kind the vocabulary names is reachable from an ordinary chat.

This is the check the owner asked for in their own words — "whatever you add, add
it for both". A kind added to one send path and forgotten on the other is exactly
the defect nobody reports, because the caller assumes the richer path is the
special case and stops looking.

It measures this repository against itself, which is weaker than comparing the two
repositories and is deliberate: the sibling Telethon Secret Chat package is not a
dependency here yet, so a cross-repository assertion could not run until the middle
of the backend swap — the moment it is least wanted.
`docs/adr/0003-the-media-kind-vocabulary-outlives-tdlib.md` records that choice.

It keeps running after TDLib is deleted, because nothing in it mentions TDLib.
"""

import pytest

from telegram_mcp import media_kinds, media_send


def test_every_kind_has_a_route_out_of_an_ordinary_chat():
    """`flags_for` is the route. A kind with no entry raises KeyError on a real
    send, after the file has been opened and the entity resolved."""
    missing = [kind for kind in media_kinds.KINDS if kind not in _routed()]
    assert not missing, (
        "no ordinary-chat route for: "
        + ", ".join(missing)
        + ". Add it to _FLAGS in telegram_mcp/media_send.py, or this kind can be "
        "asked for and cannot be sent."
    )


def test_animation_is_accounted_for_by_the_video_route_rather_than_a_flag():
    """Stated here because a reader checking parity WILL notice `animation` has
    no flag of its own and assume the route is missing.

    A video with no audio track is displayed as an animated GIF by Telegram
    already — Telethon's own `nosound_video` documentation says exactly that — so
    a flag here would name behaviour that happens without it. research.md records
    the decision. The route is real; it is the video route.
    """
    flags = media_send.flags_for("animation")
    assert flags == {"force_document": False}
    assert "nosound_video" not in flags
    assert media_kinds.infer_kind("loop.gif") == "animation"


def test_every_kind_can_be_asked_for_by_name_on_some_real_file():
    """A route that exists but that `resolve_kind` refuses for every file is not
    a route. Each kind needs at least one extension the families allow it on."""
    for kind in media_kinds.KINDS:
        allowed = [
            name
            for family in media_kinds.FAMILIES.values()
            if kind in family["allows"]
            for name in (f"sample{suffix}" for suffix in sorted(family["suffixes"]))
        ]
        assert allowed, f"{kind} is allowed on no file at all"
        assert media_send.resolve_kind(allowed[0], kind, caption="") == kind


def test_every_kind_is_placed_in_the_splitting_plan():
    """A kind the planner does not know is grouped by accident — it would share a
    message with whatever happened to precede it."""
    for kind in media_kinds.KINDS:
        ((indices, flags),) = media_send.group_sends([kind])
        assert indices == [0]
        assert flags == media_send.flags_for(kind)


@pytest.mark.parametrize("kind", sorted(media_kinds.NO_CAPTION))
def test_the_captionless_kinds_are_reachable_without_a_caption(kind):
    """They are refused WITH a caption on purpose. That must not be mistaken for
    being unreachable."""
    name = "sticker.tgs" if kind == "sticker" else "clip.mp4"
    assert media_send.resolve_kind(name, kind, caption="") == kind


def _routed():
    """The kinds the ordinary send path can actually produce."""
    return {kind for kind in media_kinds.KINDS if _has_route(kind)}


def _has_route(kind):
    try:
        media_send.flags_for(kind)
    except KeyError:
        return False
    return True
