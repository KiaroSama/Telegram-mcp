"""tdesktop HTML export: message bubbles, service texts, media cards, polls, giveaways, buttons
and reactions (spec 030, stream C). Expected HTML is derived by hand from
export_output_html.cpp. Synthetic data only.
"""

from datetime import timezone

import pytest

from telegram_mcp.tdexport import model_actions as act
from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.html_message import MessageInfo, MessageMixin
from telegram_mcp.tdexport.html_service import service_text
from telegram_mcp.tdexport.html_text import PeersMap
from telegram_mcp.tdexport.model import (
    Birthday,
    CreditsAmount,
    CreditsType,
    Document,
    File,
    TextPart,
    peer_from_channel,
    peer_from_user,
)
from telegram_mcp.tdexport.model_dialogs import DialogInfo
from telegram_mcp.tdexport.model_media import (
    GeoPoint,
    GiveawayResults,
    GiveawayStart,
    Media,
    Poll,
    TodoList,
    TodoListItem,
    UnsupportedMedia,
)
from telegram_mcp.tdexport.model_message import HistoryMessageMarkupButton, Message, Reaction
from telegram_mcp.tdexport.model_peers import Chat, ContactInfo, Peer, User

DATE = 86400 * 365  # 01.01.1971 00:00:00 UTC
ANN = peer_from_user(5)
SELF = peer_from_user(9)
GROUP = DialogInfo(type=DialogInfo.Type.PrivateGroup, name="G")
CHANNEL = DialogInfo(type=DialogInfo.Type.PrivateChannel, name="C")


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
            peer_from_channel(3): Peer(Chat(bare_id=3, title="News", is_broadcast=True)),
        }
    )


def link(message_id, text):
    target = f'href="#go_to_message{message_id}"'
    return f'<a {target} onclick="return GoToMessage({message_id})">{text}</a>'


def render(message, previous=None, dialog=GROUP):
    return MessageMixin("").push_message(
        message, previous, dialog, "", peers(), "https://t.me/", link
    )


def text_message(**fields):
    base = dict(id=10, date=DATE, from_id=ANN, text=[TextPart(text="Hello")])
    base.update(fields)
    return Message(**base)


ANN_USERPIC = (
    '\n <div class="pull_left userpic_wrap">\n'
    '\n  <div class="userpic userpic4" style="width: 42px; height: 42px">\n'
    '\n   <div class="initials" style="line-height: 42px">\n'
    "AL"
    "\n   </div>\n"
    "\n  </div>\n"
    "\n </div>\n"
)
DATE_DIV = (
    '\n  <div class="pull_right date details" title="01.01.1971 00:00:00 UTC+00:00">\n'
    "00:00"
    "\n  </div>\n"
)


def test_first_message_gets_userpic_and_name():
    info, html = render(text_message())
    assert info.type == MessageInfo.Type.Default
    assert html == (
        '\n<div class="message default clearfix" id="message10">\n'
        + ANN_USERPIC
        + '\n <div class="body">\n'
        + DATE_DIV
        + '\n  <div class="from_name">\nAnn Lee\n  </div>\n'
        + '\n  <div class="text">\nHello\n  </div>\n'
        + "\n </div>\n"
        + "\n</div>\n"
    )


def test_follow_up_message_joins():
    first, _ = render(text_message())
    _, html = render(text_message(id=11, date=DATE + 900, reply_to_msg_id=7), first)
    assert html == (
        '\n<div class="message default clearfix joined" id="message11">\n'
        '\n <div class="body">\n'
        '\n  <div class="pull_right date details" title="01.01.1971 00:15:00 UTC+00:00">\n'
        "00:15"
        "\n  </div>\n"
        '\n  <div class="reply_to details">\n'
        "In reply to " + link(7, "this message") + "\n  </div>\n"
        '\n  <div class="text">\nHello\n  </div>\n'
        "\n </div>\n"
        "\n</div>\n"
    )
    _, late = render(text_message(id=12, date=DATE + 901), first)
    assert 'class="message default clearfix"' in late
    _, other_chat = render(text_message(reply_to_msg_id=7, reply_to_peer_id=SELF), None)
    assert "In reply to a message in another chat" in other_chat


def test_forwarded_message_block():
    message = text_message(
        forwarded=True,
        forwarded_from_name="Bob  Smith",
        forwarded_date=DATE - 60,
        signature="Editor",
        via_bot_id=7,
    )
    _, html = render(message)
    assert html == (
        '\n<div class="message default clearfix" id="message10">\n'
        + ANN_USERPIC
        + '\n <div class="body">\n'
        + DATE_DIV
        + '\n  <div class="from_name">\nAnn Lee\n  </div>\n'
        + '\n  <div class="pull_left forwarded userpic_wrap">\n'
        '\n   <div class="userpic userpic2" style="width: 42px; height: 42px">\n'
        '\n    <div class="initials" style="line-height: 42px">\n'
        "BS"
        "\n    </div>\n"
        "\n   </div>\n"
        "\n  </div>\n"
        '\n  <div class="forwarded body">\n'
        '\n   <div class="from_name">\n'
        "Bob Smith via @robot"
        '<span class="date details" title="31.12.1970 23:59:00 UTC+00:00">'
        " 31.12.1970 23:59:00</span>"
        "\n   </div>\n"
        '\n   <div class="text">\nHello\n   </div>\n'
        '\n   <div class="signature details">\nEditor\n   </div>\n'
        "\n  </div>\n"
        "\n </div>\n"
        "\n</div>\n"
    )


def test_service_messages():
    rename = Message(id=3, date=DATE, from_id=ANN)
    rename.action.content = act.ActionChatEditTitle("New & title")
    info, html = render(rename)
    assert info.type == MessageInfo.Type.Service
    assert html == (
        '\n<div class="message service" id="message3">\n'
        '\n <div class="body details">\n'
        "Ann Lee changed group title to &laquo;New &amp; title&raquo;"
        "\n </div>\n"
        "\n</div>\n"
    )
    _, channel = render(rename, dialog=CHANNEL)
    assert "Channel title changed to &laquo;New &amp; title&raquo;" in channel
    unsupported = Message(id=4, date=DATE, from_id=ANN, media=Media(UnsupportedMedia()))
    _, html = render(unsupported)
    assert "This message is not supported by this version of Telegram Desktop." in html


def _service(content, dialog=GROUP, **fields):
    message = Message(id=1, date=DATE, from_id=ANN, reply_to_msg_id=8, **fields)
    message.action.content = content
    return service_text(message, dialog, peers(), "https://t.me/", "", lambda t: link(8, t))


def test_service_texts():
    assert _service(act.ActionChatCreate("G", [5, 6, 9])) == (
        "Ann Lee created group &laquo;G&raquo; with members " "Ann Lee, Deleted Account and Me"
    )
    assert _service(act.ActionPinMessage()) == "Ann Lee pinned " + link(8, "this message")
    assert _service(act.ActionPhoneCall()) == ""
    assert _service(None) == ""
    assert _service(act.ActionCustomAction("<raw>")) == "<raw>"
    assert _service(act.ActionBotAllowed(app="", domain="x.org")) == (
        "You allowed this bot to message you when you opened "
    )
    secure = act.ActionSecureValuesSent(
        [act.ActionSecureValuesSent.Type.Passport, act.ActionSecureValuesSent.Type.Email]
    )
    assert _service(secure) == "You have sent the following documents: Passport and Email"
    near = act.ActionGeoProximityReached(ANN, SELF, 1550, False, True)
    assert _service(near) == "Ann Lee is now within 1.55 km from you"
    assert _service(act.ActionGeoProximityReached(ANN, SELF, 1)) == (
        "Ann Lee is now within 1 meter from Me"
    )
    assert _service(act.ActionSetMessagesTTL(86400), CHANNEL) == (
        "New messages will auto-delete in 24 hours"
    )
    assert _service(act.ActionTopicEdit("T", 0)) == (
        "Ann Lee changed topic title to &laquo;T&raquo;,icon to &laquo;0&raquo;"
    )
    assert _service(act.ActionTopicEdit("", None)) == "Ann Lee changed topic "
    assert _service(act.ActionGiftCode(days=1, via_giveaway=True)) == (
        "You won a Telegram Premium for 1 day prize in a giveaway organized by a channel."
    )
    credits = act.ActionGiftCredits("$5", CreditsAmount.make(2, 500_000_000, CreditsType.Ton))
    assert _service(credits) == "Ann Lee sent you a gift for $5: 2.5 TON."
    assert _service(act.ActionPrizeStars(peer_from_channel(3), 50)) == (
        "You won a prize in a giveaway organized by News.\n Your prize is 50 Telegram Stars."
    )
    assert _service(act.ActionPaidMessagesRefunded(3, 30), out=True) == (
        "You refunded 30 Stars for 3 messages to Deleted"
    )
    assert _service(act.ActionTodoCompletions([1, 2, 3], [])) == (
        "Ann Lee marked 1, 2 and 3 as done in " + link(8, "this todo list") + "."
    )
    tasks = act.ActionTodoAppendTasks([TodoListItem([TextPart(TextPart.Type.Bold, "a")])])
    assert _service(tasks) == "Ann Lee added tasks: &quot;<strong>a</strong>&quot;"
    approval = act.ActionSuggestedPostApproval(
        reject_comment="no <way>",
        schedule_date=DATE + 3600 * 5 + 60 * 7,
        price=CreditsAmount.make(15),
        rejected=True,
    )
    assert _service(approval) == (
        "Ann Lee rejected your suggested post, for 15 stars, 1 January 1971 at 05:07,"
        " with comment: &quot;no &lt;way&gt;&quot;"
    )
    assert _service(act.ActionSuggestBirthday(Birthday.make(2, 3))) == (
        "Ann Lee suggests to add a date of birth: 2 March"
    )
    assert _service(act.ActionNewCreatorPending(9)) == (
        "Me will become the new main admin in 7 days if Ann Lee does not return"
    )


def test_generic_file_card():
    document = Document(name="a<b>.pdf", file=File(size=2048, relative_path="files/a.pdf"))
    _, html = render(text_message(text=[], media=Media(document)))
    assert (
        '\n  <div class="media_wrap clearfix">\n'
        '\n   <a class="media clearfix pull_left block_link media_file" href="files/a.pdf">\n'
        '\n    <div class="fill pull_left">\n'
        "\n    </div>\n"
        '\n    <div class="body">\n'
        '\n     <div class="title bold">\na&lt;b&gt;.pdf\n     </div>\n'
        '\n     <div class="status details">\n2.0 KB\n     </div>\n'
        "\n    </div>\n"
        "\n   </a>\n"
        "\n  </div>\n"
    ) in html


def test_media_cards_without_files():
    voice = Document(
        is_voice_message=True,
        duration=65,
        file=File(size=10, skip_reason=SkipReason.FileType),
    )
    _, html = render(text_message(text=[], media=Media(voice)))
    assert '<div class="media clearfix pull_left media_voice_message">' in html
    assert "Voice message" in html and "01:05, 10 B" in html
    assert "Not included, change data exporting settings to download." in html
    geo = GeoPoint(55.75, 37.62, True)
    _, html = render(text_message(text=[], media=Media(geo)))
    assert (
        'href="https://maps.google.com/maps?q=55.750000,37.620000&amp;ll=55.750000,37.620000'
        '&amp;z=16"'
    ) in html
    assert "55.750000, 37.620000" in html
    call = Message(id=2, date=DATE, from_id=SELF, peer_id=ANN, out=True)
    call.action.content = act.ActionPhoneCall(duration=12, state=act.ActionPhoneCall.State.Hangup)
    _, html = render(call)
    assert 'class="media clearfix pull_left media_call success"' in html
    assert "Ann Lee" in html and "Outgoing (12 seconds)" in html


def test_poll_and_todo_list():
    poll = Poll(
        question=[TextPart(text="Q?")],
        answers=[
            Poll.Answer([TextPart(text="A")], votes=2, my=True),
            Poll.Answer([TextPart(text="B")], votes=1),
            Poll.Answer([TextPart(text="C")]),
        ],
        total_votes=3,
    )
    wrap = MessageMixin("")
    assert wrap.push_poll(poll, "https://t.me/", "") == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <div class="media_poll">\n'
        '\n  <div class="question bold">\nQ?\n  </div>\n'
        '\n  <div class="details">\nAnonymous poll\n  </div>\n'
        '\n  <div class="answer">\n- A <span class="details">2 votes, chosen vote</span>'
        "\n  </div>\n"
        '\n  <div class="answer">\n- B <span class="details">1 vote</span>\n  </div>\n'
        '\n  <div class="answer">\n- C\n  </div>\n'
        '\n  <div class="total details&#x09;">\n3 votes\n  </div>\n'
        "\n </div>\n"
        "\n</div>\n"
    )
    todo = TodoList(title=[TextPart(text="T")], items=[TodoListItem([TextPart(text="x")])])
    assert wrap.push_todo_list(todo, "", "") == (
        '\n<div class="media_wrap clearfix">\n'
        '\n <div class="media_poll">\n'
        '\n  <div class="question bold">\nT\n  </div>\n'
        '\n  <div class="details">\nTo-do List\n  </div>\n'
        '\n  <div class="answer">\n- x\n  </div>\n'
        "\n </div>\n"
        "\n</div>\n"
    )


def test_giveaways():
    wrap = MessageMixin("")
    start = GiveawayStart(
        countries=["DE", "FR", "IR"],
        channels=[3],
        additional_prize="",
        until_date=DATE,
        credits=1,
        quantity=1,
        all=True,
    )
    html = wrap.push_giveaway_start(peers(), start)
    assert "\n<b>1</b> \n" in html
    assert "<b>1 Star</b> will be distributed to <b>1</b> winner." in html
    assert "\nAll subscribers of the channel:\n" in html and "News" not in html
    assert "from \U0001f1e9\U0001f1ea\u00a0Germany, \U0001f1eb\U0001f1f7\u00a0France and " in html
    assert html.endswith(
        '\n  <div class="section_body">\n01.01.1971 00:00:00\n  </div>\n\n </div>\n\n</div>\n'
    )
    results = GiveawayResults(winners=[5], winners_count=3, launch_id=4, credits=0)
    html = wrap.push_giveaway_results(peers(), results, link)
    assert "<b>3</b> winners of the " + link(4, "Giveaway") + " was randomly" in html
    assert "<b>Ann Lee</b> and 2 more!" in html
    assert "All winners received gift links in private messages." in html


def test_inline_buttons():
    rows = [
        [
            HistoryMessageMarkupButton(HistoryMessageMarkupButton.Type.Url, "Go", b"https://a.b"),
            HistoryMessageMarkupButton(HistoryMessageMarkupButton.Type.Callback, "It's", b"x\\y"),
        ]
    ]
    _, html = render(text_message(text=[], inline_button_rows=rows))
    assert (
        '\n  <table class="bot_buttons_table">\n'
        "\n   <tbody>\n"
        "\n    <tr>\n"
        '\n     <td class="bot_button_row">\n'
        '\n      <div class="bot_button">\n'
        '\n       <a ="" href="https://a.b">\n'
        "\n        <div>\nGo\n        </div>\n"
        "\n       </a>\n"
        "\n      </div>\n"
        '\n      <div class="bot_button_column_separator">\n'
        "\n      </div>\n"
        '\n      <div class="bot_button">\n'
        '\n       <a ="" onclick="return ShowTextCopied(&apos;Data: x\\\\y'
        ' | Type: callback&apos;);">\n'
        "\n        <div>\nIt&apos;s\n        </div>\n"
        "\n       </a>\n"
        "\n      </div>\n"
        "\n     </td>\n"
        "\n    </tr>\n"
        "\n   </tbody>\n"
        "\n  </table>\n"
    ) in html


def test_reactions():
    reactions = [
        Reaction(
            Reaction.Type.Emoji,
            emoji="\u2764",
            count=3,
            recent=[Reaction.Recent(SELF), Reaction.Recent(ANN)],
        ),
        Reaction(Reaction.Type.Paid, count=1),
        Reaction(Reaction.Type.CustomEmoji, document_id="stickers/e.webp", count=2),
    ]
    _, html = render(text_message(text=[], reactions=reactions))
    assert html.endswith(
        '\n  <span class="reactions">\n'
        '\n   <span class="reaction active">\n'
        '\n    <span class="emoji">\n\u2764\n    </span>\n'
        '\n    <span class="userpics">\n'
        '\n     <div class="userpic userpic1" style="width: 20px; height: 20px">\n'
        '\n      <div class="initials" style="line-height: 20px" title="Me">\nM\n      </div>\n'
        "\n     </div>\n"
        '\n     <div class="userpic userpic1" style="width: 20px; height: 20px">\n'
        '\n      <div class="initials" style="line-height: 20px" title="Ann Lee">\nAL'
        "\n      </div>\n"
        "\n     </div>\n"
        "\n    </span>\n"
        '\n    <span class="count">\n3\n    </span>\n'
        "\n   </span>\n"
        '\n   <span class="reaction paid">\n'
        '\n    <span class="emoji">\n\u2b50\n    </span>\n'
        '\n    <span class="count">\n1\n    </span>\n'
        "\n   </span>\n"
        '\n   <span class="reaction">\n'
        '\n    <span class="emoji">\n<a href = "stickers/e.webp">\U0001f44b</a>\n    </span>\n'
        '\n    <span class="count">\n2\n    </span>\n'
        "\n   </span>\n"
        "\n  </span>\n"
        "\n </div>\n"
        "\n</div>\n"
    )
