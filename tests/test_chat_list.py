"""Finding and ordering the owner's chats (spec 008, US1 and US2), on a fake client.

What these pin, because the return value alone would not show it:

  - A folder is matched the way Telegram shows it: chats named in it (pinned or
    included) are in, excluded ones are out, and everything else is decided by the
    folder's rules (bots, groups, contacts...) and its exclusions (muted, read,
    archived). Telegram has no call that lists a rule-based folder's chats.
  - Pinning inside a folder rewrites only that folder, under the same lock the
    other folder tools hold, and refuses a chat the folder does not show.
  - "Already pinned" is read from Telegram before anything is sent.
"""

import json
from datetime import datetime, timedelta, timezone

import pytest
from telethon import utils
from telethon.tl import functions, types

from telegram_mcp.tools import chat_list as mod
from telegram_mcp.tools import folders as folders_mod


def _user(uid, first, username=None, bot=False, contact=False):
    return types.User(
        id=uid, first_name=first, username=username, bot=bot, contact=contact, access_hash=uid
    )


BOT4 = _user(8695614338, "Numera Group Bot 4", "NumeraGroup4Bot", bot=True)
FRIEND = _user(42, "Sara", "sara_k", contact=True)
CHANNEL = types.Channel(
    id=555,
    title="Announcements",
    photo=types.ChatPhotoEmpty(),
    date=None,
    broadcast=True,
    access_hash=5,
)
GROUP = types.Channel(
    id=777,
    title="Numera Talk",
    photo=types.ChatPhotoEmpty(),
    date=None,
    megagroup=True,
    access_hash=7,
)


def _raw(entity, *, archived=False, unread=0, muted=False, pinned=False, unread_mark=False):
    until = datetime.now(timezone.utc) + timedelta(days=1) if muted else None
    return types.Dialog(
        peer=utils.get_peer(entity),
        top_message=1,
        read_inbox_max_id=0,
        read_outbox_max_id=0,
        unread_count=unread,
        unread_mentions_count=0,
        unread_reactions_count=0,
        unread_poll_votes_count=0,
        notify_settings=types.PeerNotifySettings(mute_until=until),
        pinned=pinned,
        unread_mark=unread_mark,
        folder_id=1 if archived else None,
    )


class _Dialog:
    """The fields of Telethon's custom Dialog the module may read."""

    def __init__(self, entity, **raw):
        self.entity = entity
        self.id = utils.get_peer_id(entity)
        self.name = utils.get_display_name(entity)
        self.dialog = _raw(entity, **raw)
        self.archived = self.dialog.folder_id == 1


def _folder(fid, title, *, include=(), pinned=(), exclude=(), **rules):
    return types.DialogFilter(
        id=fid,
        title=types.TextWithEntities(text=title, entities=[]),
        pinned_peers=[utils.get_input_peer(e) for e in pinned],
        include_peers=[utils.get_input_peer(e) for e in include],
        exclude_peers=[utils.get_input_peer(e) for e in exclude],
        **rules,
    )


class FakeClient:
    def __init__(self, dialogs, filters):
        self.dialogs = dialogs
        self.filters = filters
        self.sent = []
        self.answers = {}
        self.peer_dialog_calls = 0
        self.search_extra = []

    async def iter_dialogs(self, *args, **kwargs):
        raise AssertionError("walking every dialog outlives the tool budget on a real account")
        yield  # pragma: no cover

    async def get_me(self, input_peer=False):
        return types.InputPeerUser(user_id=1, access_hash=1)

    async def __call__(self, request):
        if isinstance(request, functions.messages.GetDialogFiltersRequest):
            return types.messages.DialogFilters(filters=list(self.filters))
        if isinstance(request, functions.messages.GetPeerDialogsRequest):
            self.peer_dialog_calls += 1
            wanted = {utils.get_peer_id(p.peer) for p in request.peers}
            return types.messages.PeerDialogs(
                dialogs=[d.dialog for d in self.dialogs if d.id in wanted],
                messages=[],
                chats=[],
                users=[],
                state=None,
            )
        if isinstance(request, functions.contacts.SearchRequest):
            # Telegram's own search: the owner's chats in my_results, strangers in results.
            q = request.q.casefold()
            hits = [
                d.entity
                for d in self.dialogs
                if q in d.name.casefold()
                or q in (getattr(d.entity, "username", "") or "").casefold()
            ] + list(self.search_extra)
            stranger = types.User(id=99, first_name="Numera Stranger", access_hash=9)
            return types.contacts.Found(
                my_results=[utils.get_peer(e) for e in hits],
                results=[utils.get_peer(stranger)],
                chats=[e for e in hits if not isinstance(e, types.User)],
                users=[e for e in hits if isinstance(e, types.User)] + [stranger],
            )
        self.sent.append(request)
        return self.answers.get(type(request), True)


@pytest.fixture
def client(monkeypatch):
    dialogs = [
        _Dialog(BOT4),
        _Dialog(FRIEND, archived=True),
        _Dialog(CHANNEL, muted=True),
        _Dialog(GROUP, unread=3),
    ]
    filters = [
        types.DialogFilterDefault(),
        _folder(13, "My Bots", include=[BOT4]),
        _folder(3, "Robot", bots=True),
        _folder(5, "Groups", groups=True, exclude=[GROUP]),
        _folder(4, "Channels", broadcasts=True, exclude_muted=True),
        _folder(2, "Personal", contacts=True, exclude_archived=True),
    ]
    fake = FakeClient(dialogs, filters)
    by_name = {"@NumeraGroup4Bot": BOT4, "@announcements": CHANNEL, "@sara_k": FRIEND}

    async def _resolve(value, cl=None, account=None):
        return by_name[value]

    monkeypatch.setattr(mod, "get_client", lambda account=None: fake)
    monkeypatch.setattr(mod, "resolve_entity", _resolve)
    return fake


def _rows(text):
    return json.loads(text)["results"]


# --- US1: search ---------------------------------------------------------------------


@pytest.mark.asyncio
async def test_search_matches_name_or_username_ignoring_case(client):
    rows = _rows(await mod.search_my_chats(query="numeragroup4"))
    assert [r["name"] for r in rows] == ["Numera Group Bot 4"]
    assert rows[0]["type"] == "bot" and rows[0]["id"] == 8695614338
    assert rows[0]["folders"] == ["My Bots", "Robot"]
    assert rows[0]["archived"] is False
    assert rows[0]["pinned"] is False and rows[0]["unread"] == 0
    assert rows[0]["muted_until"] is None and rows[0]["silent"] is False
    assert client.peer_dialog_calls == 1, "the found chats' dialogs are read in one request"


@pytest.mark.asyncio
async def test_strangers_from_the_global_search_are_not_listed(client):
    rows = _rows(await mod.search_my_chats(query="numera"))
    assert "Numera Stranger" not in [r["name"] for r in rows]


@pytest.mark.asyncio
async def test_a_muted_chat_shows_until_when(client):
    row = _rows(await mod.search_my_chats(query="announce"))[0]
    assert row["muted_until"] is not None and row["muted_until"].endswith("UTC")


@pytest.mark.asyncio
async def test_archived_chats_are_searched_and_marked(client):
    rows = _rows(await mod.search_my_chats(query="sara"))
    assert rows[0]["archived"] is True
    assert "Personal" not in rows[0]["folders"], "exclude_archived was ignored"


@pytest.mark.asyncio
async def test_folder_rules_and_exclusions(client):
    group = _rows(await mod.search_my_chats(query="numera talk"))[0]
    assert "Groups" not in group["folders"], "an excluded chat was shown"
    channel = _rows(await mod.search_my_chats(query="announce"))[0]
    assert "Channels" not in channel["folders"], "exclude_muted was ignored"


@pytest.mark.asyncio
async def test_search_inside_a_folder_lists_only_what_it_shows(client):
    rows = _rows(await mod.search_my_chats(query="numera", folder="robot"))
    assert [r["name"] for r in rows] == ["Numera Group Bot 4"]


@pytest.mark.asyncio
async def test_an_unknown_folder_names_the_real_ones(client):
    text = await mod.search_my_chats(query="x", folder="Nope")
    assert "My Bots" in text and "Robot" in text and "Nope" in text


@pytest.mark.asyncio
async def test_no_match_says_so_rather_than_listing_everything(client):
    text = await mod.search_my_chats(query="zzzz")
    assert "No chat" in text and "Numera" not in text


@pytest.mark.asyncio
async def test_the_row_count_is_bounded_before_client_work(client):
    assert "Error" in await mod.search_my_chats(query="a", limit=0)
    rows = _rows(await mod.search_my_chats(query="a", limit=1))
    assert len(rows) == 1


# --- US2: pin, unpin, unread ---------------------------------------------------------


@pytest.mark.asyncio
async def test_pin_in_all_chats_toggles_the_dialog_pin(client):
    await mod.pin_chat(chat="@NumeraGroup4Bot")
    request = client.sent[0]
    assert isinstance(request, functions.messages.ToggleDialogPinRequest)
    assert request.pinned is True
    assert utils.get_peer_id(request.peer.peer) == BOT4.id


@pytest.mark.asyncio
async def test_pinning_a_pinned_chat_is_no_change(client):
    client.dialogs[0].dialog.pinned = True
    text = await mod.pin_chat(chat="@NumeraGroup4Bot")
    assert "no change" in text.lower() and client.sent == []
    await mod.unpin_chat(chat="@NumeraGroup4Bot")
    assert client.sent[0].pinned is False


@pytest.mark.asyncio
async def test_pin_in_a_folder_rewrites_only_that_folder(client, monkeypatch):
    held = []
    real_lock = folders_mod._folder_lock

    def _spy(account, folder_id):
        held.append(folder_id)
        return real_lock(account, folder_id)

    monkeypatch.setattr(mod, "_folder_lock", _spy)
    await mod.pin_chat(chat="@NumeraGroup4Bot", folder="My Bots")
    request = client.sent[0]
    assert isinstance(request, functions.messages.UpdateDialogFilterRequest)
    assert request.id == 13 and held == [13]
    ids = lambda peers: [utils.get_peer_id(p) for p in peers]  # noqa: E731
    assert ids(request.filter.pinned_peers) == [BOT4.id]
    assert BOT4.id not in ids(request.filter.include_peers)
    assert client.filters[1].pinned_peers == [], "the cached folder was mutated in place"


@pytest.mark.asyncio
async def test_pin_in_a_rule_folder_works_for_a_chat_the_rules_show(client):
    await mod.pin_chat(chat="@NumeraGroup4Bot", folder="Robot")
    assert client.sent[0].filter.bots is True


@pytest.mark.asyncio
async def test_a_chat_the_folder_does_not_show_cannot_be_pinned_there(client):
    text = await mod.pin_chat(chat="@announcements", folder="My Bots")
    assert "add_chat_to_folder" in text and client.sent == []


@pytest.mark.asyncio
async def test_unpin_in_a_folder_keeps_the_chat_in_it(client):
    client.filters[1] = _folder(13, "My Bots", pinned=[BOT4])
    await mod.unpin_chat(chat="@NumeraGroup4Bot", folder="My Bots")
    f = client.sent[0].filter
    assert f.pinned_peers == []
    assert [utils.get_peer_id(p) for p in f.include_peers] == [BOT4.id]


@pytest.mark.asyncio
async def test_mark_unread(client):
    await mod.mark_chat_unread(chat="@sara_k")
    request = client.sent[0]
    assert isinstance(request, functions.messages.MarkDialogUnreadRequest)
    assert request.unread is True and utils.get_peer_id(request.peer.peer) == FRIEND.id


@pytest.mark.asyncio
async def test_a_found_chat_with_no_dialog_is_not_one_of_your_chats(client):
    """Telegram's search also returns peers the owner has no chat with, such as a bot
    whose chat was just deleted; the chat list does not show them, so neither may this."""
    gone = client.dialogs.pop(0)  # the bot still matches the search, but has no dialog
    client.search_extra = [gone.entity]
    text = await mod.search_my_chats(query="numeragroup4")
    assert "No chat" in text


@pytest.mark.asyncio
async def test_unpin_in_a_rule_folder_does_not_add_the_chat_explicitly(client):
    """Robot shows every bot by its rule; unpinning there must leave no explicit entry."""
    client.filters[2] = _folder(3, "Robot", pinned=[BOT4], bots=True)
    await mod.unpin_chat(chat="@NumeraGroup4Bot", folder="Robot")
    f = client.sent[0].filter
    assert f.pinned_peers == [] and f.include_peers == []


@pytest.mark.asyncio
async def test_pinning_a_chat_not_in_the_list_says_so(client):
    client.dialogs.pop(0)
    text = await mod.pin_chat(chat="@NumeraGroup4Bot")
    assert "not in your chat list" in text and client.sent == []
