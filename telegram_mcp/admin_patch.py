"""Merge explicit admin patches (upstream dcf9ccb, Apache-2.0 adaptation)."""

from telethon import functions, types
from telegram_mcp import admin_rights_sets as sets


class PatchError(ValueError):
    """A patch that must not reach EditAdmin."""


async def prepare(client, chat, user, given, rank):
    if rank is not None and (not isinstance(rank, str) or len(rank) > 16):
        raise PatchError("rank must be a string of at most 16 characters.")
    changes = {name: value for name, value in given.items() if value is not None}
    if any(type(value) is not bool for value in changes.values()):
        raise PatchError("Rights must be true or false.")
    got = await client(functions.channels.GetParticipantRequest(channel=chat, participant=user))
    participant = got.participant
    current = getattr(participant, "admin_rights", None)
    if current is None:
        raise PatchError("This participant has no admin rights to patch.")
    if (
        getattr(user, "is_self", False)
        or isinstance(user, (types.InputPeerSelf, types.InputUserSelf))
        or getattr(participant, "is_self", False)
    ):
        if not isinstance(participant, types.ChannelParticipantCreator):
            raise PatchError("Only the creator can change their own admin settings.")
        if set(changes) - {"anonymous"}:
            raise PatchError("The creator can change only their own anonymous flag and title.")
    before = {name: bool(getattr(current, name, False)) for name in sets.telethon_fields()}
    after = {**before, **changes}
    if not any(on for name, on in after.items() if name != "other"):
        raise PatchError("A patch cannot remove all admin rights; use demote_admin.")
    previous_rank = getattr(participant, "rank", None) or ""
    title = previous_rank if rank is None else rank
    changed = [name for name in after if after[name] != before.get(name)]
    return after, title, not changed and title == previous_rank
