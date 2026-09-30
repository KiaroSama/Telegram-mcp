"""JSON for one message's values: the push helpers of SerializeMessage and service actions.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_json.cpp
SerializeMessage: its push/pushBare/pushFrom/... lambdas and the ServiceAction v::match), GPL-3.0.

tdesktop's `push(key, value)` picks its format from the C++ type of `value`; here each call site
names it instead: `number` (arithmetic), `boolean`, `peer` (PeerId) or `string` (byte strings,
left out when empty).
"""

from __future__ import annotations

from typing import Any

from .files import SkipReason
from .json_serialize import (
    K_ARRAY,
    JsonContext,
    Pairs,
    format_file_path,
    number,
    serialize_array,
    serialize_object,
    serialize_string,
    serialize_text,
    string_allow_null,
    u64,
)
from .model import File, Image, peer_from_user, peer_to_channel, peer_to_chat, peer_to_user
from .model_actions import (
    ActionBoostApply,
    ActionBotAllowed,
    ActionChangeCreator,
    ActionChannelCreate,
    ActionChannelMigrateFrom,
    ActionChatAddUser,
    ActionChatCreate,
    ActionChatDeletePhoto,
    ActionChatDeleteUser,
    ActionChatEditPhoto,
    ActionChatEditTitle,
    ActionChatJoinedByLink,
    ActionChatJoinedByRequest,
    ActionChatJoinedViaCommunity,
    ActionChatMigrateTo,
    ActionContactSignUp,
    ActionCustomAction,
    ActionGameScore,
    ActionGeoProximityReached,
    ActionGiftCode,
    ActionGiftCredits,
    ActionGiftPremium,
    ActionGiveawayLaunch,
    ActionGiveawayResults,
    ActionGroupCall,
    ActionGroupCallScheduled,
    ActionHistoryClear,
    ActionInviteToGroupCall,
    ActionManagedBotCreated,
    ActionNewCreatorPending,
    ActionNoForwardsRequest,
    ActionNoForwardsToggle,
    ActionPaidMessagesPrice,
    ActionPaidMessagesRefunded,
    ActionPaymentRefunded,
    ActionPaymentSent,
    ActionPhoneCall,
    ActionPhoneNumberRequest,
    ActionPinMessage,
    ActionPollAppendAnswer,
    ActionPollDeleteAnswer,
    ActionPrizeStars,
    ActionRequestedPeer,
    ActionScreenshotTaken,
    ActionSecureValuesSent,
    ActionSetChatTheme,
    ActionSetChatWallPaper,
    ActionSetMessagesTTL,
    ActionStarGift,
    ActionSuggestBirthday,
    ActionSuggestedPostApproval,
    ActionSuggestedPostRefund,
    ActionSuggestedPostSuccess,
    ActionSuggestProfilePhoto,
    ActionTodoAppendTasks,
    ActionTodoCompletions,
    ActionTopicCreate,
    ActionTopicEdit,
    ActionWebViewDataSent,
)
from .model_message import Message
from .model_peers import Peer, User

_SKIP_TEXTS = {
    SkipReason.Unavailable: "(File unavailable, please try again later)",
    SkipReason.FileSize: (
        "(File exceeds maximum size. Change data exporting settings to download.)"
    ),
    SkipReason.FileType: "(File not included. Change data exporting settings to download.)",
}


class MessageValues:
    """The `values` vector of SerializeMessage with the lambdas that fill it."""

    def __init__(self, context: JsonContext, message: Message, peers: dict[int, Peer]) -> None:
        self.context = context
        self.message = message
        self.peers = peers
        self.values: Pairs = []

    def bare(self, key: str, value: bytes) -> None:
        """pushBare."""
        if value:
            self.values.append((key, value))

    def number(self, key: str, value: int | float) -> None:
        self.bare(key, number(value))

    def boolean(self, key: str, value: bool) -> None:
        self.bare(key, b"true" if value else b"false")

    def peer(self, key: str, peer_id: int) -> None:
        self.bare(key, self.wrap_peer_id(peer_id))

    def string(self, key: str, value: str | bytes) -> None:
        if value:
            self.bare(key, serialize_string(value))

    @staticmethod
    def wrap_peer_id(peer_id: int) -> bytes:
        if chat := peer_to_chat(peer_id):
            return serialize_string("chat" + str(chat))
        if channel := peer_to_channel(peer_id):
            return serialize_string("channel" + str(channel))
        return serialize_string("user" + str(peer_to_user(peer_id)))

    def peer_of(self, peer_id: int) -> Peer:
        return self.peers.get(peer_id) or Peer(User())

    def user_of(self, user_id: int) -> User:
        return self.peer_of(peer_from_user(user_id)).user() or User()

    def wrap_peer_name(self, peer_id: int) -> bytes:
        return string_allow_null(self.peer_of(peer_id).name())

    def wrap_user_name(self, user_id: int) -> bytes:
        return string_allow_null(self.user_of(user_id).name())

    def push_from(self, label: str = "from") -> None:
        if self.message.from_id:
            self.bare(label, self.wrap_peer_name(self.message.from_id))
            self.peer(label + "_id", self.message.from_id)

    def push_reply_to_msg_id(self, label: str = "reply_to_message_id") -> None:
        if self.message.reply_to_msg_id:
            self.number(label, self.message.reply_to_msg_id)
            if self.message.reply_to_peer_id:
                self.peer("reply_to_peer_id", self.message.reply_to_peer_id)

    def push_user_names(self, user_ids: list[int], label: str = "members") -> None:
        names = [self.wrap_user_name(user_id) for user_id in user_ids]
        self.bare(label, serialize_array(self.context, names))

    def push_actor(self) -> None:
        self.push_from("actor")

    def push_action(self, action: str) -> None:
        self.string("action", action)

    def push_ttl(self, label: str = "self_destruct_period_seconds") -> None:
        if ttl := self.message.media.ttl:
            self.number(label, ttl)

    def push_path(self, file: File, label: str) -> None:
        if file.skip_reason == SkipReason.None_:
            self.string(label, format_file_path(file))
        elif file.skip_reason in _SKIP_TEXTS:
            self.string(label, _SKIP_TEXTS[file.skip_reason])
        else:
            raise ValueError("Skip reason while writing file path.")

    def push_photo(self, image: Image) -> None:
        self.push_path(image.file, "photo")
        self.number("photo_file_size", image.file.size)
        if image.width and image.height:
            self.number("width", image.width)
            self.number("height", image.height)

    def push_spoiler(self, media: Any) -> None:
        if media.spoilered:
            self.boolean("media_spoiler", True)


_SECURE_NAMES = {
    ActionSecureValuesSent.Type.PersonalDetails: "personal_details",
    ActionSecureValuesSent.Type.Passport: "passport",
    ActionSecureValuesSent.Type.DriverLicense: "driver_license",
    ActionSecureValuesSent.Type.IdentityCard: "identity_card",
    ActionSecureValuesSent.Type.InternalPassport: "internal_passport",
    ActionSecureValuesSent.Type.Address: "address_information",
    ActionSecureValuesSent.Type.UtilityBill: "utility_bill",
    ActionSecureValuesSent.Type.BankStatement: "bank_statement",
    ActionSecureValuesSent.Type.RentalAgreement: "rental_agreement",
    ActionSecureValuesSent.Type.PassportRegistration: "passport_registration",
    ActionSecureValuesSent.Type.TemporaryRegistration: "temporary_registration",
    ActionSecureValuesSent.Type.Phone: "phone_number",
    ActionSecureValuesSent.Type.Email: "email",
}
_DISCARD_REASONS = {
    ActionPhoneCall.State.Busy: "busy",
    ActionPhoneCall.State.Disconnect: "disconnect",
    ActionPhoneCall.State.Hangup: "hangup",
    ActionPhoneCall.State.Missed: "missed",
    ActionPhoneCall.State.MigrateConferenceCall: "migrate_conference_all",
}

# Actions whose whole output is pushActor() + pushAction(name).
_SIMPLE_ACTOR_ACTIONS: dict[type, str] = {
    ActionChatDeletePhoto: "delete_group_photo",
    ActionChatMigrateTo: "migrate_to_supergroup",
    ActionHistoryClear: "clear_history",
    ActionScreenshotTaken: "take_screenshot",
    ActionContactSignUp: "joined_telegram",
    ActionPhoneNumberRequest: "requested_phone_number",
    ActionChatJoinedByRequest: "join_group_by_request",
}
# pushActor() + pushAction(name) + push("title", data.title).
_TITLE_ACTIONS: dict[type, str] = {
    ActionChatEditTitle: "edit_group_title",
    ActionChannelCreate: "create_channel",
    ActionChannelMigrateFrom: "migrate_from_group",
    ActionTopicCreate: "topic_created",
}


def _push_price(v: MessageValues, price: Any) -> None:
    v.string("price_amount_whole", number(price.whole()))
    v.string("price_amount_nano", number(price.nano()))
    v.string("price_currency", "TON" if price.ton() else "Stars")


def push_service_action(v: MessageValues, data: Any) -> None:
    """The v::match over message.action.content."""
    kind = type(data)
    if data is None:
        return
    if kind in _SIMPLE_ACTOR_ACTIONS:
        v.push_actor()
        v.push_action(_SIMPLE_ACTOR_ACTIONS[kind])
    elif kind in _TITLE_ACTIONS:
        v.push_actor()
        v.push_action(_TITLE_ACTIONS[kind])
        v.string("title", data.title)
    elif isinstance(data, ActionChatCreate):
        v.push_actor()
        v.push_action("create_group")
        v.string("title", data.title)
        v.push_user_names(data.user_ids)
    elif isinstance(data, (ActionChatEditPhoto, ActionSuggestProfilePhoto)):
        v.push_actor()
        edit = isinstance(data, ActionChatEditPhoto)
        v.push_action("edit_group_photo" if edit else "suggest_profile_photo")
        v.push_photo(data.photo.image)
        v.push_spoiler(data.photo)
    elif isinstance(data, (ActionChatAddUser, ActionInviteToGroupCall)):
        v.push_actor()
        add = isinstance(data, ActionChatAddUser)
        v.push_action("invite_members" if add else "invite_to_group_call")
        v.push_user_names(data.user_ids)
    elif isinstance(data, ActionChatDeleteUser):
        v.push_actor()
        v.push_action("remove_members")
        v.push_user_names([data.user_id])
    elif isinstance(data, ActionChatJoinedByLink):
        v.push_actor()
        v.push_action("join_group_by_link")
        v.bare("inviter", v.wrap_user_name(data.inviter_id))
    elif isinstance(data, ActionPinMessage):
        v.push_actor()
        v.push_action("pin_message")
        v.push_reply_to_msg_id("message_id")
    elif isinstance(data, ActionGameScore):
        v.push_actor()
        v.push_action("score_in_game")
        v.push_reply_to_msg_id("game_message_id")
        v.number("score", data.score)
    elif isinstance(data, ActionPaymentSent):
        v.push_action("send_payment")
        v.number("amount", data.amount)
        v.string("currency", data.currency)
        v.push_reply_to_msg_id("invoice_message_id")
        if data.recurring_used:
            v.string("recurring", "used")
        elif data.recurring_init:
            v.string("recurring", "init")
    elif isinstance(data, ActionPhoneCall):
        _push_phone_call(v, data)
    elif isinstance(data, ActionCustomAction):
        v.push_actor()
        v.string("information_text", data.message)
    elif isinstance(data, ActionBotAllowed):
        _push_bot_allowed(v, data)
    elif isinstance(data, ActionSecureValuesSent):
        v.push_action("send_passport_values")
        names = [serialize_string(_SECURE_NAMES.get(one, "")) for one in data.types]
        v.bare("values", serialize_array(v.context, names))
    elif isinstance(data, ActionGeoProximityReached):
        v.push_action("proximity_reached")
        if data.from_id:
            v.bare("from", v.wrap_peer_name(data.from_id))
            v.peer("from_id", data.from_id)
        if data.to_id:
            v.bare("to", v.wrap_peer_name(data.to_id))
            v.peer("to_id", data.to_id)
        v.number("distance", data.distance)
    else:
        _push_later_action(v, data)


def _push_phone_call(v: MessageValues, data: ActionPhoneCall) -> None:
    v.push_actor()
    v.push_action("conference_call" if data.conference_id else "phone_call")
    if data.duration:
        v.number("duration_seconds", data.duration)
    if data.conference_id:
        v.boolean("is_active", data.state == ActionPhoneCall.State.Active)
        v.boolean("is_missed", data.state == ActionPhoneCall.State.Missed)
    else:
        v.string("discard_reason", _DISCARD_REASONS.get(data.state, ""))


def _push_bot_allowed(v: MessageValues, data: ActionBotAllowed) -> None:
    if data.attach_menu:
        v.push_action("attach_menu_bot_allowed")
    elif data.from_request:
        v.push_action("web_app_bot_allowed")
    elif data.app_id:
        v.push_action("allow_sending_messages")
        v.number("reason_app_id", u64(data.app_id))
        v.string("reason_app_name", data.app)
    else:
        v.push_action("allow_sending_messages")
        v.string("reason_domain", data.domain)


def _push_later_action(v: MessageValues, data: Any) -> None:
    if isinstance(data, ActionGroupCall):
        v.push_actor()
        v.push_action("group_call")
        if data.duration:
            v.number("duration", data.duration)
    elif isinstance(data, ActionSetMessagesTTL):
        v.push_actor()
        v.push_action("set_messages_ttl")
        v.number("period", data.period)
    elif isinstance(data, ActionGroupCallScheduled):
        v.push_actor()
        v.push_action("group_call_scheduled")
        v.number("schedule_date", data.date)
    elif isinstance(data, ActionSetChatTheme):
        v.push_actor()
        v.push_action("edit_chat_theme")
        if data.emoji:
            v.string("emoticon", data.emoji)
    elif isinstance(data, ActionChatJoinedViaCommunity):
        v.push_actor()
        v.push_action("join_group_via_community")
        v.number("community_id", data.community_id)
    elif isinstance(data, ActionWebViewDataSent):
        v.push_action("send_webview_data")
        v.string("text", data.text)
    elif isinstance(data, ActionGiftPremium):
        v.push_actor()
        v.push_action("send_premium_gift")
        if data.cost:
            v.string("cost", data.cost)
        if data.days:
            v.number("days", data.days)
    elif isinstance(data, ActionTopicEdit):
        v.push_actor()
        v.push_action("topic_edit")
        if data.title:
            v.string("new_title", data.title)
        if data.icon_emoji_id is not None:
            v.number("new_icon_emoji_id", u64(data.icon_emoji_id))
    elif isinstance(data, ActionRequestedPeer):
        v.push_actor()
        v.push_action("requested_peer")
        v.number("button_id", data.button_id)
        peers = [number(one) for one in data.peers]
        # push() of a QByteArray: the serialized array lands in the file as a string.
        v.string("peers", serialize_array(v.context, peers))
    elif isinstance(data, ActionGiftCode):
        v.push_action("gift_code_prize")
        v.string("gift_code", data.code)
        if data.boost_peer_id:
            v.peer("boost_peer_id", data.boost_peer_id)
        v.number("days", data.days)
        v.boolean("unclaimed", data.unclaimed)
        v.boolean("via_giveaway", data.via_giveaway)
    elif isinstance(data, ActionGiveawayLaunch):
        v.push_action("giveaway_launch")
    elif isinstance(data, ActionGiveawayResults):
        v.push_action("giveaway_results")
        v.number("winners", data.winners)
        v.number("unclaimed", data.unclaimed)
        v.boolean("stars", data.credits)
    elif isinstance(data, ActionSetChatWallPaper):
        v.push_actor()
        v.push_action("set_same_chat_wallpaper" if data.same else "set_chat_wallpaper")
        v.push_reply_to_msg_id("message_id")
    elif isinstance(data, ActionBoostApply):
        v.push_actor()
        v.push_action("boost_apply")
        v.number("boosts", data.boosts)
    elif isinstance(data, ActionPaymentRefunded):
        v.push_action("refunded_payment")
        v.number("amount", data.amount)
        v.string("currency", data.currency)
        v.bare("peer_name", v.wrap_peer_name(data.peer_id))
        v.peer("peer_id", data.peer_id)
        v.string("charge_id", data.transaction_id)
    elif isinstance(data, ActionGiftCredits):
        v.push_actor()
        v.push_action("send_ton_gift" if data.amount.ton() else "send_stars_gift")
        if data.cost:
            v.string("cost", data.cost)
        if data.amount:
            v.number("amount_whole", data.amount.whole())
            v.number("amount_nano", data.amount.nano())
    elif isinstance(data, ActionPrizeStars):
        v.push_actor()
        v.push_action("stars_prize")
        v.peer("boost_peer_id", data.peer_id)
        v.bare("boost_peer_name", v.wrap_peer_name(data.peer_id))
        v.number("stars", data.amount)
        v.boolean("is_unclaimed", data.is_unclaimed)
        v.number("giveaway_msg_id", data.giveaway_msg_id)
        v.string("transaction_id", data.transaction_id)
    else:
        _push_recent_action(v, data)


def _push_recent_action(v: MessageValues, data: Any) -> None:
    if isinstance(data, ActionStarGift):
        v.push_actor()
        v.push_action("send_star_gift")
        v.number("gift_id", u64(data.gift_id))
        v.number("stars", data.stars)
        v.boolean("is_limited", data.limited)
        v.boolean("is_anonymous", data.anonymous)
        v.bare("gift_text", serialize_text(v.context, data.text))
    elif isinstance(data, ActionPaidMessagesRefunded):
        v.push_actor()
        v.push_action("paid_messages_refund")
        v.number("messages_count", data.messages)
        v.number("stars_count", data.stars)
    elif isinstance(data, ActionPaidMessagesPrice):
        v.push_actor()
        v.push_action("paid_messages_price_change")
        v.number("price_stars", data.stars)
        v.boolean("is_broadcast_messages_allowed", data.broadcast_allowed)
    elif isinstance(data, ActionTodoCompletions):
        v.push_actor()
        v.push_action("todo_completions")
        v.bare("completed", b"[" + b",".join(str(i).encode() for i in data.completed) + b"]")
        v.bare("incompleted", b"[" + b",".join(str(i).encode() for i in data.incompleted) + b"]")
    elif isinstance(data, ActionTodoAppendTasks):
        v.push_actor()
        v.push_action("todo_append_tasks")
        v.bare("items", serialize_todo_items(v.context, data.items))
    elif isinstance(data, (ActionPollAppendAnswer, ActionPollDeleteAnswer)):
        v.push_actor()
        append = isinstance(data, ActionPollAppendAnswer)
        v.push_action("poll_append_answer" if append else "poll_delete_answer")
        v.string("option", data.option)
    elif isinstance(data, ActionSuggestedPostApproval):
        v.push_actor()
        v.push_action("process_suggested_post")
        if data.rejected:
            v.bare("rejected", b"true")
            if data.reject_comment:
                v.string("comment", data.reject_comment)
        else:
            _push_price(v, data.price)
            v.number("scheduled_date", data.schedule_date)
    elif isinstance(data, ActionSuggestedPostSuccess):
        v.push_actor()
        v.push_action("suggested_post_success")
        _push_price(v, data.price)
    elif isinstance(data, ActionSuggestedPostRefund):
        v.push_actor()
        v.push_action("suggested_post_refund")
        v.boolean("user_initiated", data.payer_initiated)
    elif isinstance(data, ActionSuggestBirthday):
        v.push_actor()
        v.push_action("suggest_birthday")
        v.number("day", data.birthday.day())
        v.number("month", data.birthday.month())
        if year := data.birthday.year():
            v.number("year", year)
    elif isinstance(data, ActionNoForwardsToggle):
        v.push_actor()
        v.push_action("no_forwards_toggle")
        v.boolean("new_value", data.new_value)
    elif isinstance(data, ActionNoForwardsRequest):
        v.push_actor()
        v.push_action("no_forwards_request")
        v.boolean("expired", data.expired)
        v.boolean("new_value", data.new_value)
    elif isinstance(data, (ActionNewCreatorPending, ActionChangeCreator)):
        v.push_actor()
        pending = isinstance(data, ActionNewCreatorPending)
        v.push_action("new_creator_pending" if pending else "change_creator")
        v.bare("new_creator", v.wrap_user_name(data.new_creator_id))
    elif isinstance(data, ActionManagedBotCreated):
        v.push_actor()
        v.push_action("managed_bot_created")
        v.bare("bot", v.wrap_user_name(data.bot_id))


def serialize_todo_items(context: JsonContext, items: list[Any]) -> bytes:
    """The TodoListItem array shared by ActionTodoAppendTasks and TodoList media."""
    values = []
    for item in items:
        context.nesting.append(K_ARRAY)
        values.append(
            serialize_object(
                context, [("text", serialize_text(context, item.text)), ("id", number(item.id))]
            )
        )
        context.nesting.pop()
    return serialize_array(context, values)
