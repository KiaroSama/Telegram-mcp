"""The JSON export writer: every service action as SerializeMessage writes it into result.json.

Each expectation is hand-derived from the ServiceAction v::match of Telegram Desktop v7.2.10's
export_output_json.cpp: its push()/pushBare() choice per field (numbers bare, strings quoted and
dropped when empty, PeerId as "userN"/"chatN"/"channelN"), the pushActor() prefix and the key
order. Synthetic data only.
"""

from datetime import datetime, timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport import model_actions as act
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.json_actions import MessageValues, serialize_todo_items
from telegram_mcp.tdexport.json_message import serialize_message
from telegram_mcp.tdexport.json_serialize import K_ARRAY, K_OBJECT, JsonContext
from telegram_mcp.tdexport.model import (
    Birthday,
    CreditsAmount,
    CreditsType,
    File,
    Image,
    Photo,
    TextPart,
    peer_from_channel,
    peer_from_chat,
    peer_from_user,
)
from telegram_mcp.tdexport.model_media import TodoListItem
from telegram_mcp.tdexport.model_message import Message
from telegram_mcp.tdexport.model_peers import Chat, ContactInfo, Peer, User

D = int(datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc).timestamp())
ANN = peer_from_user(100)
BOT = peer_from_user(300)
NEWS = peer_from_channel(9)
ACTOR = (("actor", '"Ann"'), ("actor_id", '"user100"'))


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def peers() -> dict:
    return {
        ANN: Peer(User(bare_id=100, info=ContactInfo(user_id=100, first_name="Ann"))),
        BOT: Peer(User(bare_id=300, info=ContactInfo(user_id=300, first_name="Bot"))),
        NEWS: Peer(Chat(bare_id=9, title="News", is_broadcast=True)),
    }


def service_json(action, **fields) -> bytes:
    message = Message(id=7, date=D, action=act.ServiceAction(action), **fields)
    return serialize_message(JsonContext([K_OBJECT, K_ARRAY]), message, peers(), "")


def expected(*pairs: tuple[str, str]) -> bytes:
    """The whole service message: header, the action's pairs, then the empty text."""
    body = [
        "{",
        '   "id": 7,',
        '   "type": "service",',
        '   "date": "2024-01-12T10:00:05",',
        '   "date_unixtime": "1705053605",',
        *(f'   "{key}": {value},' for key, value in pairs),
        '   "text": "",',
        '   "text_entities": []',
        "  }",
    ]
    return "\n".join(body).encode("utf-8")


def members(*names: str) -> str:
    """An array value one level deeper than the message's own keys."""
    return "[\n" + ",\n".join(f"    {name}" for name in names) + "\n   ]"


def photo(width: int, height: int, **file) -> Photo:
    return Photo(image=Image(width=width, height=height, file=File(**file)))


FROM_ANN = {"from_id": ANN}
S = act.ActionSecureValuesSent.Type
State = act.ActionPhoneCall.State

CASES = [
    # pushActor() + pushAction() only.
    (act.ActionChatDeletePhoto(), FROM_ANN, [*ACTOR, ("action", '"delete_group_photo"')]),
    (act.ActionChatMigrateTo(5), FROM_ANN, [*ACTOR, ("action", '"migrate_to_supergroup"')]),
    (act.ActionHistoryClear(), FROM_ANN, [*ACTOR, ("action", '"clear_history"')]),
    (act.ActionScreenshotTaken(), FROM_ANN, [*ACTOR, ("action", '"take_screenshot"')]),
    (act.ActionContactSignUp(), FROM_ANN, [*ACTOR, ("action", '"joined_telegram"')]),
    (act.ActionPhoneNumberRequest(), {}, [("action", '"requested_phone_number"')]),
    (act.ActionChatJoinedByRequest(), FROM_ANN, [*ACTOR, ("action", '"join_group_by_request"')]),
    # ... + push("title", data.title).
    (
        act.ActionChatEditTitle("T"),
        FROM_ANN,
        [*ACTOR, ("action", '"edit_group_title"'), ("title", '"T"')],
    ),
    (act.ActionChannelCreate("C"), {}, [("action", '"create_channel"'), ("title", '"C"')]),
    (
        act.ActionChannelMigrateFrom("Old", 4),
        {},
        [("action", '"migrate_from_group"'), ("title", '"Old"')],
    ),
    (act.ActionTopicCreate(""), FROM_ANN, [*ACTOR, ("action", '"topic_created"')]),
    # Photos.
    (
        act.ActionChatEditPhoto(photo(10, 20, relative_path="photos/p.jpg", size=5)),
        FROM_ANN,
        [
            *ACTOR,
            ("action", '"edit_group_photo"'),
            ("photo", '"photos/p.jpg"'),
            ("photo_file_size", "5"),
            ("width", "10"),
            ("height", "20"),
        ],
    ),
    (
        act.ActionSuggestProfilePhoto(photo(10, 0, skip_reason=SkipReason.Unavailable)),
        {},
        [
            ("action", '"suggest_profile_photo"'),
            ("photo", '"(File unavailable, please try again later)"'),
            ("photo_file_size", "0"),
        ],
    ),
    # Member lists: an unknown user is null.
    (
        act.ActionChatAddUser([100, 300]),
        FROM_ANN,
        [*ACTOR, ("action", '"invite_members"'), ("members", members('"Ann"', '"Bot"'))],
    ),
    (
        act.ActionInviteToGroupCall([55]),
        {},
        [("action", '"invite_to_group_call"'), ("members", members("null"))],
    ),
    (
        act.ActionChatDeleteUser(300),
        {},
        [("action", '"remove_members"'), ("members", members('"Bot"'))],
    ),
    (
        act.ActionChatJoinedByLink(300),
        FROM_ANN,
        [*ACTOR, ("action", '"join_group_by_link"'), ("inviter", '"Bot"')],
    ),
    # pushReplyToMsgId() under its own label, with the reply peer.
    (
        act.ActionPinMessage(),
        {"reply_to_msg_id": 4, "reply_to_peer_id": peer_from_chat(6)},
        [("action", '"pin_message"'), ("message_id", "4"), ("reply_to_peer_id", '"chat6"')],
    ),
    (
        act.ActionGameScore(1, 0),
        {"reply_to_msg_id": 3},
        [("action", '"score_in_game"'), ("game_message_id", "3"), ("score", "0")],
    ),
    (
        act.ActionSetChatWallPaper(same=True),
        {"reply_to_msg_id": 4},
        [("action", '"set_same_chat_wallpaper"'), ("message_id", "4")],
    ),
    (act.ActionSetChatWallPaper(), {}, [("action", '"set_chat_wallpaper"')]),
    # Payments: no actor even when the message has a sender.
    (
        act.ActionPaymentSent("USD", 150, recurring_init=True),
        {"reply_to_msg_id": 2, **FROM_ANN},
        [
            ("action", '"send_payment"'),
            ("amount", "150"),
            ("currency", '"USD"'),
            ("invoice_message_id", "2"),
            ("recurring", '"init"'),
        ],
    ),
    (
        act.ActionPaymentSent("", 1, recurring_init=True, recurring_used=True),
        {},
        [("action", '"send_payment"'), ("amount", "1"), ("recurring", '"used"')],
    ),
    (
        act.ActionPaymentRefunded(BOT, "EUR", 99, "tx1"),
        {},
        [
            ("action", '"refunded_payment"'),
            ("amount", "99"),
            ("currency", '"EUR"'),
            ("peer_name", '"Bot"'),
            ("peer_id", '"user300"'),
            ("charge_id", '"tx1"'),
        ],
    ),
    # Calls.
    (
        act.ActionPhoneCall(state=State.Busy, duration=9),
        {},
        [("action", '"phone_call"'), ("duration_seconds", "9"), ("discard_reason", '"busy"')],
    ),
    (act.ActionPhoneCall(), {}, [("action", '"phone_call"')]),
    (
        act.ActionPhoneCall(state=State.MigrateConferenceCall),
        {},
        [("action", '"phone_call"'), ("discard_reason", '"migrate_conference_all"')],
    ),
    (
        act.ActionPhoneCall(conference_id=3, state=State.Missed),
        {},
        [("action", '"conference_call"'), ("is_active", "false"), ("is_missed", "true")],
    ),
    (act.ActionGroupCall(), FROM_ANN, [*ACTOR, ("action", '"group_call"')]),
    (act.ActionGroupCall(30), {}, [("action", '"group_call"'), ("duration", "30")]),
    (
        act.ActionGroupCallScheduled(D),
        {},
        [("action", '"group_call_scheduled"'), ("schedule_date", "1705053605")],
    ),
    # A custom action has no "action" key at all.
    (act.ActionCustomAction("hi"), FROM_ANN, [*ACTOR, ("information_text", '"hi"')]),
    # Bots.
    (act.ActionBotAllowed(attach_menu=True), {}, [("action", '"attach_menu_bot_allowed"')]),
    (act.ActionBotAllowed(from_request=True), {}, [("action", '"web_app_bot_allowed"')]),
    (
        act.ActionBotAllowed(app_id=-1, app="App"),
        {},
        [
            ("action", '"allow_sending_messages"'),
            ("reason_app_id", "18446744073709551615"),
            ("reason_app_name", '"App"'),
        ],
    ),
    (
        act.ActionBotAllowed(domain="x.org"),
        {},
        [("action", '"allow_sending_messages"'), ("reason_domain", '"x.org"')],
    ),
    (
        act.ActionWebViewDataSent("btn"),
        {},
        # The action's own "text" comes first; the message text follows it.
        [("action", '"send_webview_data"'), ("text", '"btn"')],
    ),
    (
        act.ActionManagedBotCreated(300),
        FROM_ANN,
        [*ACTOR, ("action", '"managed_bot_created"'), ("bot", '"Bot"')],
    ),
    # Proximity: from/to only when set.
    (
        act.ActionGeoProximityReached(ANN, NEWS, 20),
        {},
        [
            ("action", '"proximity_reached"'),
            ("from", '"Ann"'),
            ("from_id", '"user100"'),
            ("to", '"News"'),
            ("to_id", '"channel9"'),
            ("distance", "20"),
        ],
    ),
    (
        act.ActionGeoProximityReached(0, 0, 0),
        {},
        [("action", '"proximity_reached"'), ("distance", "0")],
    ),
    # Chat settings.
    (act.ActionSetMessagesTTL(86400), {}, [("action", '"set_messages_ttl"'), ("period", "86400")]),
    (act.ActionSetChatTheme(""), {}, [("action", '"edit_chat_theme"')]),
    (
        act.ActionSetChatTheme("\u2764"),
        {},
        [("action", '"edit_chat_theme"'), ("emoticon", '"\u2764"')],
    ),
    (
        act.ActionChatJoinedViaCommunity(77),
        {},
        [("action", '"join_group_via_community"'), ("community_id", "77")],
    ),
    (
        act.ActionTopicEdit("New", 5),
        {},
        [("action", '"topic_edit"'), ("new_title", '"New"'), ("new_icon_emoji_id", "5")],
    ),
    (act.ActionBoostApply(2), {}, [("action", '"boost_apply"'), ("boosts", "2")]),
    (
        act.ActionNoForwardsToggle(True),
        {},
        [("action", '"no_forwards_toggle"'), ("new_value", "true")],
    ),
    (
        act.ActionNoForwardsRequest(True, False),
        {},
        [("action", '"no_forwards_request"'), ("expired", "true"), ("new_value", "false")],
    ),
    (
        act.ActionNewCreatorPending(300),
        {},
        [("action", '"new_creator_pending"'), ("new_creator", '"Bot"')],
    ),
    (act.ActionChangeCreator(999), {}, [("action", '"change_creator"'), ("new_creator", "null")]),
    # Gifts and giveaways.
    (
        act.ActionGiftPremium("$5", 30),
        {},
        [("action", '"send_premium_gift"'), ("cost", '"$5"'), ("days", "30")],
    ),
    (act.ActionGiftPremium(), {}, [("action", '"send_premium_gift"')]),
    (
        act.ActionGiftCode("CODE", NEWS, 90, via_giveaway=True),
        {},
        [
            ("action", '"gift_code_prize"'),
            ("gift_code", '"CODE"'),
            ("boost_peer_id", '"channel9"'),
            ("days", "90"),
            ("unclaimed", "false"),
            ("via_giveaway", "true"),
        ],
    ),
    (
        act.ActionGiftCode(days=1, unclaimed=True),
        {},
        [
            ("action", '"gift_code_prize"'),
            ("days", "1"),
            ("unclaimed", "true"),
            ("via_giveaway", "false"),
        ],
    ),
    (act.ActionGiveawayLaunch(), FROM_ANN, [("action", '"giveaway_launch"')]),
    (
        act.ActionGiveawayResults(3, 1, True),
        {},
        [
            ("action", '"giveaway_results"'),
            ("winners", "3"),
            ("unclaimed", "1"),
            ("stars", "true"),
        ],
    ),
    (
        act.ActionGiftCredits("$1", CreditsAmount.make(2, 500_000_000, CreditsType.Ton)),
        {},
        [
            ("action", '"send_ton_gift"'),
            ("cost", '"$1"'),
            ("amount_whole", "2"),
            ("amount_nano", "500000000"),
        ],
    ),
    (act.ActionGiftCredits(), {}, [("action", '"send_stars_gift"')]),
    (
        act.ActionPrizeStars(NEWS, 50, "tx", 12, True),
        {},
        [
            ("action", '"stars_prize"'),
            ("boost_peer_id", '"channel9"'),
            ("boost_peer_name", '"News"'),
            ("stars", "50"),
            ("is_unclaimed", "true"),
            ("giveaway_msg_id", "12"),
            ("transaction_id", '"tx"'),
        ],
    ),
    (
        act.ActionStarGift(gift_id=-2, stars=25, text=[TextPart(text="hi")], anonymous=True),
        {},
        [
            ("action", '"send_star_gift"'),
            ("gift_id", "18446744073709551614"),
            ("stars", "25"),
            ("is_limited", "false"),
            ("is_anonymous", "true"),
            ("gift_text", '"hi"'),
        ],
    ),
    # Paid messages.
    (
        act.ActionPaidMessagesRefunded(3, 30),
        {},
        [("action", '"paid_messages_refund"'), ("messages_count", "3"), ("stars_count", "30")],
    ),
    (
        act.ActionPaidMessagesPrice(10, True),
        {},
        [
            ("action", '"paid_messages_price_change"'),
            ("price_stars", "10"),
            ("is_broadcast_messages_allowed", "true"),
        ],
    ),
    # Todo lists and polls: the index lists are joined without spaces.
    (
        act.ActionTodoCompletions([1, 2], []),
        {},
        [("action", '"todo_completions"'), ("completed", "[1,2]"), ("incompleted", "[]")],
    ),
    (
        act.ActionPollAppendAnswer("Yes"),
        {},
        [("action", '"poll_append_answer"'), ("option", '"Yes"')],
    ),
    (
        act.ActionPollDeleteAnswer("No"),
        {},
        [("action", '"poll_delete_answer"'), ("option", '"No"')],
    ),
    # Suggested posts.
    (
        act.ActionSuggestedPostApproval(reject_comment="no", rejected=True),
        {},
        [("action", '"process_suggested_post"'), ("rejected", "true"), ("comment", '"no"')],
    ),
    (
        act.ActionSuggestedPostApproval(rejected=True),
        {},
        [("action", '"process_suggested_post"'), ("rejected", "true")],
    ),
    (
        act.ActionSuggestedPostSuccess(CreditsAmount.make(1, 5, CreditsType.Ton)),
        {},
        [
            ("action", '"suggested_post_success"'),
            ("price_amount_whole", '"1"'),
            ("price_amount_nano", '"5"'),
            ("price_currency", '"TON"'),
        ],
    ),
    (
        act.ActionSuggestedPostRefund(True),
        {},
        [("action", '"suggested_post_refund"'), ("user_initiated", "true")],
    ),
    # Birthdays: the year only when set.
    (
        act.ActionSuggestBirthday(Birthday.make(2, 3, 1990)),
        {},
        [("action", '"suggest_birthday"'), ("day", "2"), ("month", "3"), ("year", "1990")],
    ),
    (
        act.ActionSuggestBirthday(Birthday.make(29, 2)),
        {},
        [("action", '"suggest_birthday"'), ("day", "29"), ("month", "2")],
    ),
]


@pytest.mark.parametrize(("action", "fields", "pairs"), CASES)
def test_service_action_json(action, fields, pairs):
    assert service_json(action, **fields) == expected(*pairs)


def test_passport_values_list_every_type_in_order():
    body = service_json(act.ActionSecureValuesSent(list(S)))
    names = [
        "personal_details",
        "passport",
        "driver_license",
        "identity_card",
        "internal_passport",
        "address_information",
        "utility_bill",
        "bank_statement",
        "rental_agreement",
        "passport_registration",
        "temporary_registration",
        "phone_number",
        "email",
    ]
    values = members(*(f'"{name}"' for name in names))
    assert body == expected(("action", '"send_passport_values"'), ("values", values))


def test_file_path_placeholders_and_unexpected_skip_reason():
    for reason, text in (
        (SkipReason.FileSize, "(File exceeds maximum size. Change data exporting settings"),
        (SkipReason.FileType, "(File not included. Change data exporting settings"),
    ):
        body = service_json(act.ActionChatEditPhoto(photo(0, 0, skip_reason=reason)))
        assert f'"photo": "{text} to download.)",'.encode() in body
    # tdesktop's Unexpected("Skip reason while writing file path.") for any other reason.
    with pytest.raises(ValueError, match="Skip reason while writing file path."):
        service_json(act.ActionChatEditPhoto(photo(0, 0, skip_reason=SkipReason.DateLimits)))


def test_peer_ids_and_names_of_unknown_peers():
    values = MessageValues(JsonContext(), Message(), {})
    assert values.wrap_peer_id(peer_from_chat(4)) == b'"chat4"'
    assert values.wrap_peer_id(peer_from_channel(4)) == b'"channel4"'
    assert values.wrap_peer_id(peer_from_user(4)) == b'"user4"'
    # A channel id asked for as a user resolves to the empty User: null.
    assert values.wrap_peer_name(peer_from_user(4)) == b"null"
    values.peers = peers()
    assert values.wrap_user_name(9) == b"null"
    assert values.wrap_user_name(100) == b'"Ann"'
    values.string("empty", "")
    values.bare("bare", b"")
    assert values.values == []


def test_todo_items_array_of_objects():
    items = [TodoListItem([TextPart(text="a")], 1), TodoListItem([], 2)]
    assert serialize_todo_items(JsonContext([K_OBJECT]), items) == (
        b'[\n  {\n   "text": "a",\n   "id": 1\n  },\n  {\n   "text": "",\n   "id": 2\n  }\n ]'
    )
    assert serialize_todo_items(JsonContext(), []) == b"[\n]"
