"""tdesktop HTML export: the English service-message sentence of every service action.

Each expectation is derived by hand from the `serviceText` v::match of Telegram Desktop v7.2.10's
export_output_html.cpp (HtmlWriter::Wrap::pushMessage), channel/group variants and quirks
included (the bot-allowed app/domain swap, the unescaped poll option). Synthetic data only.
"""

from datetime import timezone

import pytest

from telegram_mcp.tdexport import model_actions as act
from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.html_service import qt_number, service_text
from telegram_mcp.tdexport.html_text import PeersMap
from telegram_mcp.tdexport.model import (
    Birthday,
    CreditsAmount,
    CreditsType,
    TextPart,
    peer_from_channel,
    peer_from_user,
)
from telegram_mcp.tdexport.model_dialogs import DialogInfo
from telegram_mcp.tdexport.model_media import TodoListItem
from telegram_mcp.tdexport.model_message import Message
from telegram_mcp.tdexport.model_peers import Chat, ContactInfo, Peer, User

DATE = 86400 * 365  # 01.01.1971 00:00:00 UTC
ANN = peer_from_user(5)
SELF = peer_from_user(9)
NEWS = peer_from_channel(3)
GROUP = DialogInfo(type=DialogInfo.Type.PrivateGroup, name="G", peer_id=ANN)
CHANNEL = DialogInfo(type=DialogInfo.Type.PublicChannel, name="C")
S = act.ActionSecureValuesSent.Type


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def user(bare_id, first, last="", **extra):
    return Peer(User(bare_id=bare_id, info=ContactInfo(bare_id, first, last), **extra))


def peers():
    return PeersMap(
        {
            ANN: user(5, "Ann", "Lee"),
            SELF: user(9, "Me", is_self=True),
            peer_from_user(7): user(7, "Bot", username="robot", is_bot=True),
            NEWS: Peer(Chat(bare_id=3, title="News", is_broadcast=True)),
        }
    )


def link(text):
    return f'<a href="#go_to_message8" onclick="return GoToMessage(8)">{text}</a>'


def text(content, dialog=GROUP, **fields):
    message = Message(id=1, date=DATE, from_id=ANN, reply_to_msg_id=8, **fields)
    message.action.content = content
    return service_text(message, dialog, peers(), "https://t.me/", "", link)


GROUP_CASES = [
    (act.ActionChatCreate("G<", []), "Ann Lee created group &laquo;G&lt;&raquo;"),
    (act.ActionChatEditPhoto(), "Ann Lee changed group photo"),
    (act.ActionChatDeletePhoto(), "Ann Lee removed group photo"),
    (act.ActionChatAddUser([7]), "Ann Lee invited Bot"),
    (act.ActionChatDeleteUser(9), "Ann Lee removed Me"),
    (act.ActionChatDeleteUser(4), "Ann Lee removed Deleted Account"),
    (act.ActionChatJoinedByLink(7), "Ann Lee joined group by link from Bot"),
    (act.ActionChannelCreate("C&"), "Channel &laquo;C&amp;&raquo; created"),
    (act.ActionChatMigrateTo(4), "Ann Lee converted this group to a supergroup"),
    (
        act.ActionChannelMigrateFrom("Old"),
        "Ann Lee converted a basic group to this supergroup &laquo;Old&raquo;",
    ),
    (act.ActionHistoryClear(), "History cleared"),
    (act.ActionGameScore(score=12), "Ann Lee scored 12 in " + link("this game")),
    (
        act.ActionPaymentSent("USD", 150),
        "You have successfully transferred $1.50 for " + link("this invoice"),
    ),
    (
        act.ActionPaymentSent("USD", 150, recurring_init=True),
        "You have successfully transferred $1.50 for "
        + link("this invoice")
        + " and allowed future recurring payments",
    ),
    (
        act.ActionPaymentSent("USD", 150, recurring_used=True),
        "You were charged $1.50 via recurring payment",
    ),
    (act.ActionScreenshotTaken(), "Ann Lee took a screenshot"),
    (
        act.ActionBotAllowed(attach_menu=True),
        "You allowed this bot to message you when you added it in the attachment menu.",
    ),
    (
        act.ActionBotAllowed(from_request=True),
        "You allowed this bot to message you in his web-app.",
    ),
    # tdesktop prints the domain when the app name is set (and the empty app otherwise).
    (
        act.ActionBotAllowed(app="App", domain="x.org"),
        "You allowed this bot to message you when you logged in on x.org",
    ),
    (act.ActionContactSignUp(), "Ann Lee joined Telegram"),
    (
        act.ActionGeoProximityReached(SELF, ANN, 200, from_self=True),
        "You are now within 200 meters from Ann Lee",
    ),
    (
        act.ActionGeoProximityReached(ANN, NEWS, 1009),
        "Ann Lee is now within 1 km from News",
    ),
    (act.ActionPhoneNumberRequest(), "Ann Lee requested your phone number"),
    (act.ActionGroupCall(), "Ann Lee started voice chat"),
    (act.ActionGroupCall(45), "Ann Lee started voice chat (45 seconds)"),
    (
        act.ActionInviteToGroupCall([5, 7, 9]),
        "Ann Lee invited Ann Lee, Bot and Me to the voice chat",
    ),
    (
        act.ActionSetMessagesTTL(7 * 86400),
        "Ann Lee has set messages to auto-delete in 7 days",
    ),
    (act.ActionSetMessagesTTL(3600), "Ann Lee has set messages to auto-delete in "),
    (act.ActionSetMessagesTTL(0), "Ann Lee has set messages not to auto-delete"),
    (
        act.ActionGroupCallScheduled(DATE + 61),
        "Ann Lee scheduled a voice chat for 01.01.1971 00:01:01",
    ),
    (act.ActionSetChatTheme(""), "Ann Lee disabled chat theme"),
    (act.ActionSetChatTheme("\u2764"), "Ann Lee changed chat theme to \u2764"),
    (act.ActionChatJoinedByRequest(), "Ann Lee joined group by request"),
    (act.ActionChatJoinedViaCommunity(4), "Ann Lee joined group via a community"),
    (
        act.ActionWebViewDataSent("<b>"),
        "You have just successfully transferred data from the &laquo;&lt;b&gt;&raquo; button"
        " to the bot",
    ),
    (
        act.ActionGiftPremium("$5", 30),
        "Ann Lee sent you a gift for $5: Telegram Premium for 30 days.",
    ),
    (act.ActionGiftPremium("$5", 0), "Ann Lee sent you a gift."),
    (act.ActionGiftPremium("", 30), "Ann Lee sent you a gift."),
    (act.ActionTopicCreate("T"), "Ann Lee created topic &laquo;T&raquo;"),
    (act.ActionTopicEdit("", 77), "Ann Lee changed topic icon to &laquo;77&raquo;"),
    (act.ActionSuggestProfilePhoto(), "Ann Lee suggests to use this photo"),
    (act.ActionRequestedPeer([ANN], 1), "requested: "),
    (
        act.ActionSetChatWallPaper(same=True),
        "Ann Lee set " + link("the same background") + " for this chat",
    ),
    (act.ActionSetChatWallPaper(), "Ann Lee set a new background for this chat"),
    (
        act.ActionGiftCode(days=3, unclaimed=True),
        "This is an unclaimed Telegram Premium for 3 days prize in a giveaway organized by a"
        " channel.",
    ),
    (
        act.ActionGiftCode(days=1),
        "You've received a Telegram Premium for 1 day gift from a channel.",
    ),
    (
        act.ActionGiveawayLaunch(),
        "Ann Lee just started a giveaway of Telegram Premium subscriptions to its followers.",
    ),
    (act.ActionGiveawayResults(0, 1, True), "No winners of the giveaway could be selected."),
    (
        act.ActionGiveawayResults(2, 1, True),
        "Some winners of the giveaway were randomly selected by Telegram and received their"
        " prize.",
    ),
    (
        act.ActionGiveawayResults(2, 1, False),
        "Some winners of the giveaway were randomly selected by Telegram and received private"
        " messages with giftcodes.",
    ),
    (
        act.ActionGiveawayResults(2, 0, True),
        "2 of the giveaway was randomly selected by Telegram and received their prize.",
    ),
    (
        act.ActionGiveawayResults(1000, 0, False),
        "1000 of the giveaway was randomly selected by Telegram and received private messages"
        " with giftcodes.",
    ),
    (act.ActionBoostApply(1), "Ann Lee boosted the group 1 time"),
    (act.ActionBoostApply(3), "Ann Lee boosted the group 3 times"),
    (
        act.ActionPaymentRefunded(peer_from_user(7), "USD", 150),
        "Bot refunded back $1.50",
    ),
    (act.ActionGiftCredits("", CreditsAmount.make(40)), "Ann Lee sent you a gift."),
    (act.ActionGiftCredits("$2", CreditsAmount()), "Ann Lee sent you a gift."),
    (
        act.ActionGiftCredits("$2", CreditsAmount.make(40)),
        "Ann Lee sent you a gift for $2: 40 Telegram Stars.",
    ),
    (act.ActionStarGift(stars=25), "Ann Lee sent you a gift of 25 Telegram Stars."),
    (
        act.ActionPaidMessagesRefunded(3, 30),
        "Ann Lee refunded 30 Stars for 3 messages to you",
    ),
    (
        act.ActionPaidMessagesPrice(10, True),
        "Price per message changed to 10 Telegram Stars.",
    ),
    (
        act.ActionTodoCompletions([], [4]),
        "Ann Lee marked 4 as not done yet in " + link("this todo list") + ".",
    ),
    (
        act.ActionTodoCompletions([1], [2, 3]),
        "Ann Lee marked 1 as done and 2 and 3 as not done yet in " + link("this todo list") + ".",
    ),
    (
        act.ActionTodoCompletions([], []),
        "Ann Lee marked  as done and  as not done yet in " + link("this todo list") + ".",
    ),
    (
        act.ActionTodoAppendTasks(
            [TodoListItem([TextPart(text="x<y")]), TodoListItem([TextPart(text="z")])]
        ),
        "Ann Lee added tasks: &quot;x&lt;y&quot;, &quot;z&quot;",
    ),
    # The poll option goes in raw: tdesktop does not escape it.
    (act.ActionPollAppendAnswer("<b>"), "Ann Lee added &quot;<b>&quot; to the poll."),
    (act.ActionPollDeleteAnswer("No"), "Ann Lee removed &quot;No&quot; from the poll."),
    (act.ActionSuggestedPostApproval(), "Ann Lee approved your suggested post."),
    (
        act.ActionSuggestedPostApproval(price=CreditsAmount.make(1, 500_000_000, CreditsType.Ton)),
        "Ann Lee approved your suggested post, for 1.5 TON.",
    ),
    (
        act.ActionSuggestedPostSuccess(CreditsAmount.make(3)),
        "The paid post was shown for 24 hours and 3 stars were transferred to the channel.",
    ),
    (
        act.ActionSuggestedPostSuccess(CreditsAmount.make(0, 250_000_000, CreditsType.Ton)),
        "The paid post was shown for 24 hours and 0.25 TON were transferred to the channel.",
    ),
    (
        act.ActionSuggestedPostRefund(True),
        "The user refunded the payment, post was deleted.",
    ),
    (
        act.ActionSuggestedPostRefund(False),
        "The admin deleted the post early, the payment was refunded.",
    ),
    (
        act.ActionSuggestBirthday(Birthday.make(31, 12, 2000)),
        "Ann Lee suggests to add a date of birth: 31 December 2000",
    ),
    (act.ActionSuggestBirthday(Birthday()), "Ann Lee suggests to add a date of birth: 0"),
    (act.ActionNoForwardsToggle(True), "Ann Lee disabled sharing in this chat"),
    (act.ActionNoForwardsToggle(False), "Ann Lee enabled sharing in this chat"),
    (act.ActionNoForwardsRequest(), "Ann Lee requested to enable sharing in this chat"),
    (
        act.ActionChangeCreator(9),
        "Ann Lee made Me the new main admin of the group",
    ),
    (act.ActionManagedBotCreated(7), "Ann Lee created a bot Bot"),
]


@pytest.mark.parametrize(("content", "expected"), GROUP_CASES)
def test_group_service_text(content, expected):
    assert text(content) == expected


CHANNEL_CASES = [
    (act.ActionChatEditPhoto(), "Channel photo changed"),
    (act.ActionChatDeletePhoto(), "Channel photo removed"),
    (act.ActionGroupCall(), "Voice chat"),
    (act.ActionGroupCall(45), "Voice chat (45 seconds)"),
    (act.ActionSetMessagesTTL(0), "New messages will not auto-delete"),
    (act.ActionGroupCallScheduled(DATE), "Voice chat scheduled for 01.01.1971 00:00:00"),
    (act.ActionSetChatTheme(""), "Channel theme was disabled"),
    (act.ActionSetChatTheme("\U0001f389"), "Channel theme was changed to \U0001f389"),
    (act.ActionPaidMessagesPrice(10, False), "Direct messages were disabled."),
    (
        act.ActionPaidMessagesPrice(10, True),
        "Price per direct message changed to 10 Telegram Stars.",
    ),
]


@pytest.mark.parametrize(("content", "expected"), CHANNEL_CASES)
def test_channel_service_text(content, expected):
    assert text(content, CHANNEL) == expected


def test_outgoing_paid_refund_names_the_dialog_peer():
    assert text(act.ActionPaidMessagesRefunded(2, 20), out=True) == (
        "You refunded 20 Stars for 2 messages to Ann Lee"
    )


def test_passport_documents_are_listed_with_and():
    assert text(act.ActionSecureValuesSent(list(S))) == (
        "You have sent the following documents: Personal details, Passport, Driver license,"
        " Identity card, Internal passport, Address information, Utility bill, Bank statement,"
        " Rental agreement, Passport registration, Temporary registration, Phone number and"
        " Email"
    )


def test_qt_number_uses_six_significant_digits():
    assert qt_number(2.0) == "2"
    assert qt_number(0.000001) == "1e-06"
    assert qt_number(1234567.0) == "1.23457e+06"
