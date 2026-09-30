"""Recent actions as Telegram Desktop 7.2.10 draws them (spec 033 R10).

Every admin log action becomes what Desktop's Recent actions view shows for it
(history_admin_log_item.cpp GenerateItems, the English lang.strings): its sentence, the
message it carries, the "Original message" block, the rights lists. Synthetic Telethon
events, no network. `shown` flattens the parts: a service line is its text, `note:` a
message from the actor, `message:<id>` the carried message, `original:<title>|<text>`.
"""

from datetime import date, datetime, timezone

import pytest
import telethon.tl.types as tl

from telegram_mcp import admin_log_events as ev
from telegram_mcp import admin_log_text as text
from telegram_mcp.admin_log_text import Line, LogChat, Logged, Note
from telegram_mcp.tdexport import model_format

WHEN = datetime(2026, 10, 1, 12, 0, tzinfo=timezone.utc)
LATER = datetime(2026, 10, 3, 9, 30, tzinfo=timezone.utc)
SARA = tl.User(id=42, first_name="Sara", last_name="Karimi", username="sara_admin")
BOB = tl.User(id=7, first_name="Bob")
TALK = tl.Channel(id=900, title="Talk", photo=tl.ChatPhotoEmpty(), date=None, megagroup=True)
MINUS = "\u2212"
ARROW = "\u2192"


@pytest.fixture(autouse=True)
def pinned(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)
    monkeypatch.setattr(text, "_today", lambda: date(2026, 10, 1))


def chat(kind="group", **kwargs):
    return LogChat(
        channel_id=777,
        megagroup=kind == "group",
        broadcast=kind == "channel",
        self_id=1,
        users={42: SARA, 7: BOB},
        chats={900: TALK},
        **kwargs,
    )


def parts(action, kind="group", actor=42, **kwargs):
    event = tl.ChannelAdminLogEvent(id=1, date=WHEN, user_id=actor, action=action)
    return ev.event_parts(event, chat(kind, **kwargs))


def shown(action, kind="group", actor=42, **kwargs):
    out = []
    for part in parts(action, kind, actor, **kwargs):
        if isinstance(part, Line):
            out.append(part.text)
        elif isinstance(part, Note):
            out.append("note:" + "".join(p.text for p in part.parts))
        elif isinstance(part, Logged):
            out.append(f"message:{part.message.id}")
        else:
            out.append(f"original:{part.title}|" + "".join(p.text for p in part.parts))
    return out


def message(text_="hi", media=None, **kwargs):
    return tl.Message(
        id=9, peer_id=tl.PeerChannel(777), date=WHEN, message=text_, media=media, **kwargs
    )


def photo(photo_id):
    return tl.MessageMediaPhoto(
        photo=tl.Photo(
            id=photo_id,
            access_hash=1,
            file_reference=b"r",
            date=WHEN,
            sizes=[tl.PhotoSize(type="y", w=10, h=10, size=3)],
            dc_id=2,
        )
    )


def member(user_id=7, **kwargs):
    return tl.ChannelParticipant(user_id=user_id, date=WHEN, **kwargs)


def banned(until=None, peer=None, **rights):
    return tl.ChannelParticipantBanned(
        peer=peer or tl.PeerUser(7),
        kicked_by=42,
        date=WHEN,
        banned_rights=tl.ChatBannedRights(until_date=until, **rights),
    )


def admin(**rights):
    return tl.ChannelParticipantAdmin(
        user_id=7, promoted_by=42, date=WHEN, admin_rights=tl.ChatAdminRights(**rights)
    )


def invite(**kwargs):
    return tl.ChatInviteExported(link="https://t.me/+abc", admin_id=42, date=WHEN, **kwargs)


def topic(topic_id=5, title="News", **kwargs):
    return tl.ForumTopic(
        id=topic_id,
        date=WHEN,
        peer=tl.PeerChannel(777),
        title=title,
        icon_color=0,
        top_message=1,
        read_inbox_max_id=0,
        read_outbox_max_id=0,
        unread_count=0,
        unread_mentions_count=0,
        unread_reactions_count=0,
        unread_poll_votes_count=0,
        from_id=tl.PeerUser(42),
        notify_settings=tl.PeerNotifySettings(),
        **kwargs,
    )


def call_member(**kwargs):
    return tl.GroupCallParticipant(peer=tl.PeerUser(7), date=WHEN, source=1, **kwargs)


A = tl
S = "Sara Karimi"
CASES = [
    (
        A.ChannelAdminLogEventActionChangeTitle("a", "New"),
        "group",
        [f"{S} changed group name to \u00abNew\u00bb"],
    ),
    (
        A.ChannelAdminLogEventActionChangeTitle("a", "New"),
        "channel",
        [f"{S} changed channel name to \u00abNew\u00bb"],
    ),
    (
        A.ChannelAdminLogEventActionChangeAbout("old", "new"),
        "group",
        [f"{S} edited group description:", "note:new", "original:Previous description|old"],
    ),
    (
        A.ChannelAdminLogEventActionChangeAbout("old", ""),
        "channel",
        [f"{S} removed channel description", "note:", "original:Previous description|old"],
    ),
    (
        A.ChannelAdminLogEventActionChangeUsername("old", "new"),
        "group",
        [
            f"{S} changed group link:",
            "note:https://t.me/new",
            "original:Previous link|https://t.me/old",
        ],
    ),
    (
        A.ChannelAdminLogEventActionChangeUsername("old", ""),
        "channel",
        [f"{S} removed channel link", "note:", "original:Previous link|https://t.me/old"],
    ),
    (
        A.ChannelAdminLogEventActionChangePhoto(tl.PhotoEmpty(0), photo(3).photo),
        "group",
        [f"{S} changed group photo"],
    ),
    (
        A.ChannelAdminLogEventActionChangePhoto(photo(3).photo, tl.PhotoEmpty(0)),
        "channel",
        [f"{S} removed channel photo"],
    ),
    (A.ChannelAdminLogEventActionToggleInvites(True), "group", [f"{S} enabled group invites"]),
    (A.ChannelAdminLogEventActionToggleInvites(False), "group", [f"{S} disabled group invites"]),
    (A.ChannelAdminLogEventActionToggleSignatures(True), "channel", [f"{S} enabled signatures"]),
    (
        A.ChannelAdminLogEventActionUpdatePinned(message(pinned=True)),
        "group",
        [f"{S} pinned message:", "message:9"],
    ),
    (
        A.ChannelAdminLogEventActionUpdatePinned(message()),
        "group",
        [f"{S} unpinned message", "message:9"],
    ),
    (
        A.ChannelAdminLogEventActionUpdatePinned(tl.MessageEmpty(9, peer_id=tl.PeerChannel(777))),
        "group",
        [f"{S} unpinned message"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(message("old"), message("new")),
        "group",
        [f"{S} edited message:", "message:9", "original:Original message|old"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(message(""), message("new")),
        "group",
        [f"{S} edited message:", "message:9", "original:Original message|Empty"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(
            message("old", photo(3)), message("new", photo(3))
        ),
        "group",
        [f"{S} edited caption:", "message:9", "original:Original caption|old"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(message("cap", photo(3)), message("", photo(3))),
        "group",
        [f"{S} removed caption", "message:9", "original:Original caption|cap"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(message("c", photo(3)), message("c", photo(4))),
        "group",
        [f"{S} edited media:", "message:9", "original:Original caption|c"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(message("a", photo(3)), message("b", photo(4))),
        "group",
        [f"{S} edited media and caption:", "message:9", "original:Original caption|a"],
    ),
    (
        A.ChannelAdminLogEventActionEditMessage(message("a", photo(3)), message("", photo(4))),
        "group",
        [f"{S} edited media and removed caption:", "message:9", "original:Original caption|a"],
    ),
    (
        A.ChannelAdminLogEventActionDeleteMessage(message()),
        "group",
        [f"{S} deleted message:", "message:9"],
    ),
    (A.ChannelAdminLogEventActionParticipantJoin(), "group", [f"{S} joined the group"]),
    (A.ChannelAdminLogEventActionParticipantJoin(), "channel", [f"{S} joined the channel"]),
    (A.ChannelAdminLogEventActionParticipantLeave(), "group", [f"{S} left the group"]),
    (A.ChannelAdminLogEventActionParticipantLeave(), "channel", [f"{S} left the channel"]),
    (A.ChannelAdminLogEventActionParticipantInvite(member()), "group", ["note:invited Bob"]),
    (
        A.ChannelAdminLogEventActionParticipantInvite(member(42)),
        "group",
        [f"note:invited {S} (@sara_admin)"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleBan(member(), banned(view_messages=True)),
        "group",
        ["note:banned Bob"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleBan(
            member(), banned(LATER, view_messages=True)
        ),
        "group",
        ["note:banned Bob until Oct 3 at 09:30"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleBan(
            member(), banned(send_photos=True, send_gifs=True)
        ),
        "group",
        [
            f"note:changed restrictions for Bob indefinitely\n\n{MINUS}Send stickers & GIFs\n{MINUS}Send photos"
        ],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleBan(banned(view_messages=True), member()),
        "group",
        ["note:changed restrictions for Bob indefinitely\n\n+Read messages"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleBan(
            banned(peer=tl.PeerChannel(900), view_messages=True), banned(peer=tl.PeerChannel(900))
        ),
        "group",
        ["note:unbanned Talk"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleAdmin(
            member(), admin(change_info=True, invite_users=True, manage_call=True)
        ),
        "group",
        [
            "note:changed privileges for Bob\n\n+Change info\n+Invite users via link\n+Manage video chats"
        ],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleAdmin(
            member(), admin(change_info=True, invite_users=True, manage_call=True)
        ),
        "channel",
        ["note:changed privileges for Bob\n\n+Change info\n+Add users\n+Manage live streams"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleAdmin(admin(delete_messages=True), member()),
        "group",
        [f"note:changed privileges for Bob\n\n{MINUS}Delete messages"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleAdmin(
            admin(), tl.ChannelParticipantCreator(user_id=7, admin_rights=tl.ChatAdminRights())
        ),
        "group",
        ["note:transferred ownership to Bob"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantToggleAdmin(
            tl.ChannelParticipantCreator(user_id=7, admin_rights=tl.ChatAdminRights()), admin()
        ),
        "group",
        [],
    ),
    (
        A.ChannelAdminLogEventActionChangeStickerSet(
            tl.InputStickerSetEmpty(), tl.InputStickerSetID(1, 2)
        ),
        "group",
        [f"{S} changed the group sticker set"],
    ),
    (
        A.ChannelAdminLogEventActionChangeStickerSet(
            tl.InputStickerSetID(1, 2), tl.InputStickerSetEmpty()
        ),
        "group",
        [f"{S} removed the group sticker set"],
    ),
    (
        A.ChannelAdminLogEventActionChangeEmojiStickerSet(
            tl.InputStickerSetEmpty(), tl.InputStickerSetID(1, 2)
        ),
        "group",
        [f"{S} changed the group's emoji set"],
    ),
    (
        A.ChannelAdminLogEventActionChangeEmojiStickerSet(
            tl.InputStickerSetID(1, 2), tl.InputStickerSetEmpty()
        ),
        "group",
        [f"{S} removed the group's emoji set"],
    ),
    (
        A.ChannelAdminLogEventActionTogglePreHistoryHidden(True),
        "group",
        [f"{S} made the group history hidden for new members"],
    ),
    (
        A.ChannelAdminLogEventActionTogglePreHistoryHidden(False),
        "group",
        [f"{S} made group history visible for new members"],
    ),
    (
        A.ChannelAdminLogEventActionDefaultBannedRights(
            tl.ChatBannedRights(None, edit_rank=True), tl.ChatBannedRights(None, send_polls=True)
        ),
        "group",
        [f"note:changed default permissions\n\n+Edit own tags\n{MINUS}Send polls"],
    ),
    (
        A.ChannelAdminLogEventActionStopPoll(message()),
        "group",
        [f"{S} stopped poll:", "message:9"],
    ),
    (
        A.ChannelAdminLogEventActionChangeLinkedChat(0, 900),
        "channel",
        [f"{S} changed the discussion group to \u00abTalk\u00bb"],
    ),
    (
        A.ChannelAdminLogEventActionChangeLinkedChat(0, 900),
        "group",
        [f"{S} changed the linked channel to \u00abTalk\u00bb"],
    ),
    (
        A.ChannelAdminLogEventActionChangeLinkedChat(900, 0),
        "channel",
        [f"{S} removed the discussion group"],
    ),
    (
        A.ChannelAdminLogEventActionChangeLinkedChat(900, 0),
        "group",
        [f"{S} removed the linked channel"],
    ),
    (
        A.ChannelAdminLogEventActionChangeLocation(
            tl.ChannelLocationEmpty(), tl.ChannelLocation(tl.GeoPointEmpty(), "Tehran")
        ),
        "group",
        [f"{S} changed the group location to Tehran"],
    ),
    (
        A.ChannelAdminLogEventActionChangeLocation(
            tl.ChannelLocation(tl.GeoPointEmpty(), "x"), tl.ChannelLocationEmpty()
        ),
        "group",
        [f"{S} removed the group location"],
    ),
    (
        A.ChannelAdminLogEventActionToggleSlowMode(0, 30),
        "group",
        [f"{S} changed slow mode to 30 seconds"],
    ),
    (
        A.ChannelAdminLogEventActionToggleSlowMode(0, 60),
        "group",
        [f"{S} changed slow mode to 1 minute"],
    ),
    (A.ChannelAdminLogEventActionToggleSlowMode(60, 0), "group", [f"{S} disabled slow mode"]),
    (
        A.ChannelAdminLogEventActionStartGroupCall(tl.InputGroupCall(1, 2)),
        "group",
        [f"{S} started a new video chat"],
    ),
    (
        A.ChannelAdminLogEventActionStartGroupCall(tl.InputGroupCall(1, 2)),
        "channel",
        [f"{S} started a new live stream"],
    ),
    (
        A.ChannelAdminLogEventActionDiscardGroupCall(tl.InputGroupCall(1, 2)),
        "group",
        [f"{S} ended the video chat"],
    ),
    (
        A.ChannelAdminLogEventActionDiscardGroupCall(tl.InputGroupCall(1, 2)),
        "channel",
        [f"{S} ended the live stream"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantMute(call_member()),
        "group",
        [f"{S} muted Bob in a video chat"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantUnmute(call_member()),
        "channel",
        [f"{S} unmuted Bob in a live stream"],
    ),
    (
        A.ChannelAdminLogEventActionToggleGroupCallSetting(True),
        "group",
        [f"{S} muted new video chat participants"],
    ),
    (
        A.ChannelAdminLogEventActionToggleGroupCallSetting(False),
        "channel",
        [f"{S} allowed new live stream participants to speak"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantJoinByInvite(invite()),
        "group",
        [f"{S} joined the group via t.me/+abc"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantJoinByInvite(invite(title="Friends")),
        "channel",
        [f"{S} joined the channel via Friends"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantJoinByInvite(invite(), via_chatlist=True),
        "group",
        [f"{S} joined via folder invite link t.me/+abc"],
    ),
    (
        A.ChannelAdminLogEventActionExportedInviteDelete(invite()),
        "group",
        [f"{S} deleted the invite link t.me/+abc"],
    ),
    (
        A.ChannelAdminLogEventActionExportedInviteRevoke(invite()),
        "group",
        [f"{S} revoked invite link t.me/+abc"],
    ),
    (
        A.ChannelAdminLogEventActionExportedInviteEdit(
            invite(), invite(title="VIP", usage_limit=10, request_needed=True, expire_date=LATER)
        ),
        "group",
        [
            "note:edited invite link VIP\n"
            f"\nName:  {ARROW} VIP"
            f"\nExpiry date: No limit {ARROW} Oct 3 at 09:30"
            f"\nUsage limit: No limit {ARROW} 10"
            "\nAdmin approval is now required to join."
        ],
    ),
    (
        A.ChannelAdminLogEventActionParticipantVolume(call_member(volume=5000)),
        "group",
        [f"{S} changed video chat volume for Bob to 50%"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantVolume(call_member()),
        "channel",
        [f"{S} changed live stream volume for Bob to 100%"],
    ),
    (
        A.ChannelAdminLogEventActionChangeHistoryTTL(0, 86400),
        "group",
        [f"{S} enabled messages auto-delete after 1 day"],
    ),
    (
        A.ChannelAdminLogEventActionChangeHistoryTTL(86400, 0),
        "group",
        [f"{S} disabled messages auto-deletion after 1 day"],
    ),
    (
        A.ChannelAdminLogEventActionChangeHistoryTTL(5, 604800),
        "group",
        [f"{S} changed messages auto-delete period from 5 seconds to 1 week"],
    ),
    (
        A.ChannelAdminLogEventActionChangeHistoryTTL(0, 691200),
        "group",
        [f"{S} enabled messages auto-delete after 1 week 1 day"],
    ),
    (
        A.ChannelAdminLogEventActionChangeHistoryTTL(0, 2678400),
        "group",
        [f"{S} enabled messages auto-delete after 1 month"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantJoinByRequest(invite(), 7),
        "group",
        [f"{S} was approved to join the group via t.me/+abc by Bob"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantJoinByRequest(tl.ChatInvitePublicJoinRequests(), 7),
        "channel",
        [f"{S} joined the channel via public request, approved by Bob"],
    ),
    (
        A.ChannelAdminLogEventActionToggleNoForwards(True),
        "group",
        [f"{S} restricted saving content"],
    ),
    (
        A.ChannelAdminLogEventActionToggleNoForwards(False),
        "group",
        [f"{S} allowed saving content"],
    ),
    (
        A.ChannelAdminLogEventActionSendMessage(message()),
        "group",
        [f"{S} sent this message:", "message:9"],
    ),
    (
        A.ChannelAdminLogEventActionChangeAvailableReactions(
            tl.ChatReactionsNone(),
            tl.ChatReactionsSome([tl.ReactionEmoji("\U0001f44d"), tl.ReactionCustomEmoji(5)]),
        ),
        "group",
        [f"{S} updated the list of allowed reactions to: \U0001f44d, \U0001f4ad"],
    ),
    (
        A.ChannelAdminLogEventActionChangeAvailableReactions(
            tl.ChatReactionsNone(), tl.ChatReactionsAll(allow_custom=True)
        ),
        "group",
        [f"{S} allowed all reactions"],
    ),
    (
        A.ChannelAdminLogEventActionChangeAvailableReactions(
            tl.ChatReactionsNone(), tl.ChatReactionsAll()
        ),
        "group",
        [f"{S} allowed all official reactions"],
    ),
    (
        A.ChannelAdminLogEventActionChangeAvailableReactions(
            tl.ChatReactionsAll(), tl.ChatReactionsNone()
        ),
        "group",
        [f"{S} disabled reactions"],
    ),
    (
        A.ChannelAdminLogEventActionChangeUsernames(["a"], ["b"]),
        "channel",
        [
            f"{S} changed channel link:",
            "note:https://t.me/b",
            "original:Previous link|https://t.me/a",
        ],
    ),
    (
        A.ChannelAdminLogEventActionChangeUsernames(["a", "b"], ["b", "a"]),
        "group",
        [
            f"{S} reordered group links:",
            "note:https://t.me/b\nhttps://t.me/a\n",
            "original:Previous order|https://t.me/a\nhttps://t.me/b\n",
        ],
    ),
    (
        A.ChannelAdminLogEventActionChangeUsernames(["a"], ["a", "b"]),
        "group",
        [f"{S} activated @b username"],
    ),
    (
        A.ChannelAdminLogEventActionChangeUsernames(["a", "b"], ["a"]),
        "group",
        [f"{S} deactivated @b username"],
    ),
    (A.ChannelAdminLogEventActionToggleForum(True), "group", [f"{S} enabled topics"]),
    (A.ChannelAdminLogEventActionCreateTopic(topic()), "group", [f"{S} created topic News"]),
    (
        A.ChannelAdminLogEventActionCreateTopic(topic(icon_emoji_id=5)),
        "group",
        [f"{S} created topic @ News"],
    ),
    (
        A.ChannelAdminLogEventActionCreateTopic(topic(1, "General")),
        "group",
        [f"{S} created topic # General"],
    ),
    (
        A.ChannelAdminLogEventActionEditTopic(topic(), topic(title="Daily", closed=True)),
        "group",
        [f"{S} renamed topic News to Daily", f"{S} closed topic Daily"],
    ),
    (
        A.ChannelAdminLogEventActionEditTopic(topic(hidden=True), topic()),
        "group",
        [f"{S} unhid topic News"],
    ),
    (A.ChannelAdminLogEventActionDeleteTopic(topic()), "group", [f"{S} deleted topic News"]),
    (
        A.ChannelAdminLogEventActionDeleteTopic(tl.ForumTopicDeleted(5)),
        "group",
        [f"{S} deleted topic Deleted"],
    ),
    (A.ChannelAdminLogEventActionPinTopic(None, topic()), "group", [f"{S} pinned topic News"]),
    (A.ChannelAdminLogEventActionPinTopic(topic(), None), "group", [f"{S} unpinned topic News"]),
    (
        A.ChannelAdminLogEventActionToggleAntiSpam(True),
        "group",
        [f"{S} enabled aggressive anti-spam"],
    ),
    (
        A.ChannelAdminLogEventActionChangePeerColor(
            tl.PeerColor(color=1), tl.PeerColor(color=3, background_emoji_id=5)
        ),
        "channel",
        [f"{S} changed channel color from #2 to #4", f"{S} set channel background emoji to @"],
    ),
    (
        A.ChannelAdminLogEventActionChangeProfilePeerColor(
            tl.PeerColor(background_emoji_id=5), tl.PeerColor(color=2)
        ),
        "group",
        [
            f"{S} changed group profile color from #1 to #3",
            f"{S} removed group profile background emoji @",
        ],
    ),
    (
        A.ChannelAdminLogEventActionChangeWallpaper(
            tl.WallPaperNoFile(0, tl.WallPaperSettings()),
            tl.WallPaperNoFile(1, tl.WallPaperSettings()),
        ),
        "channel",
        [f"{S} changed channel wallpaper"],
    ),
    (
        A.ChannelAdminLogEventActionChangeEmojiStatus(
            tl.EmojiStatusEmpty(), tl.EmojiStatus(5, until=LATER)
        ),
        "channel",
        [f"{S} set channel emoji status to @ until Oct 3 at 09:30"],
    ),
    (
        A.ChannelAdminLogEventActionChangeEmojiStatus(tl.EmojiStatus(5), tl.EmojiStatus(6)),
        "channel",
        [f"{S} changed channel emoji status from @ to @"],
    ),
    (
        A.ChannelAdminLogEventActionChangeEmojiStatus(tl.EmojiStatus(5), tl.EmojiStatusEmpty()),
        "channel",
        [f"{S} removed channel emoji status @"],
    ),
    (
        A.ChannelAdminLogEventActionToggleSignatureProfiles(True),
        "channel",
        [f"{S} enabled showing authors' profiles"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantSubExtend(
            member(),
            member(subscription_until_date=datetime(2027, 11, 1, 10, 0, tzinfo=timezone.utc)),
        ),
        "channel",
        ["Bob renewed subscription until November 1, 2027 at 10:00"],
    ),
    (A.ChannelAdminLogEventActionParticipantSubExtend(member(), member()), "channel", []),
    (
        A.ChannelAdminLogEventActionToggleAutotranslation(True),
        "channel",
        [f"{S} enabled automatic translation"],
    ),
    (
        A.ChannelAdminLogEventActionParticipantEditRank(7, "", "mod"),
        "group",
        [f'{S} set tag for Bob to "mod"'],
    ),
    (
        A.ChannelAdminLogEventActionParticipantEditRank(7, "mod", ""),
        "group",
        [f'{S} removed tag for Bob (was "mod")'],
    ),
    (
        A.ChannelAdminLogEventActionParticipantEditRank(42, "a", "b"),
        "group",
        [f'{S} changed own tag from "a" to "b"'],
    ),
]


@pytest.mark.parametrize("action, kind, expected", CASES)
def test_each_action_reads_as_telegram_desktop_shows_it(action, kind, expected):
    assert shown(action, kind) == expected


def test_every_admin_log_action_of_the_layer_is_drawn():
    everything = {
        getattr(tl, name)
        for name in dir(tl)
        if name.startswith("ChannelAdminLogEventAction")
        and hasattr(getattr(tl, name), "CONSTRUCTOR_ID")
    }
    assert everything == set(ev.HANDLED)
    assert {type(action) for action, _, _ in CASES} == everything


def test_a_new_chat_photo_rides_under_its_line():
    new = photo(3).photo
    (line,) = parts(A.ChannelAdminLogEventActionChangePhoto(tl.PhotoEmpty(0), new))
    assert line.photo is new


def test_the_owner_of_the_session_sees_their_own_rights_on_a_transfer():
    creator = tl.ChannelParticipantCreator(
        user_id=1, admin_rights=tl.ChatAdminRights(ban_users=True)
    )
    users = {42: SARA, 1: tl.User(id=1, first_name="Me")}
    event = tl.ChannelAdminLogEvent(
        id=1,
        date=WHEN,
        user_id=42,
        action=A.ChannelAdminLogEventActionParticipantToggleAdmin(admin(), creator),
    )
    (note,) = ev.event_parts(
        event, LogChat(channel_id=777, megagroup=True, broadcast=False, self_id=1, users=users)
    )
    assert note.parts[0].text == "changed privileges for Me\n\n+Ban users"


def test_an_invite_right_reads_add_users_when_members_may_not_invite():
    action = A.ChannelAdminLogEventActionParticipantToggleAdmin(member(), admin(invite_users=True))
    assert shown(action, anyone_can_add=False) == ["note:changed privileges for Bob\n\n+Add users"]


def test_rights_notes_are_italic_like_desktops():
    action = A.ChannelAdminLogEventActionParticipantInvite(member())
    (note,) = parts(action)
    assert [p.type.name for p in note.parts] == ["Italic"]


def test_an_edited_media_keeps_the_previous_photo_for_its_original_block():
    prev = message("a", photo(3))
    *_, original = parts(A.ChannelAdminLogEventActionEditMessage(prev, message("a", photo(4))))
    assert original.message is prev
    *_, same = parts(A.ChannelAdminLogEventActionEditMessage(prev, message("b", photo(3))))
    assert same.message is None


def test_a_far_date_carries_its_year():
    assert text.date_time(int(datetime(2024, 3, 5, 7, 0, tzinfo=timezone.utc).timestamp())) == (
        "Mar 5, 2024 at 07:00"
    )


def test_an_action_this_layer_does_not_know_draws_nothing():
    event = tl.ChannelAdminLogEvent(id=1, date=WHEN, user_id=42, action=tl.PeerUser(1))
    assert ev.event_parts(event, chat()) == []
