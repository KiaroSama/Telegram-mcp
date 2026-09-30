"""Service message actions: every Action* variant and ParseServiceAction.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.{h,cpp}
Action* structs, ServiceAction, ParseServiceAction, ParseStarGift), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .model import (
    Birthday,
    CreditsAmount,
    CreditsType,
    ParseMediaContext,
    Photo,
    TextPart,
    credits_amount_from_tl,
    parse_peer_id,
    parse_photo,
    parse_text,
    prepare_photo_file_name,
    to_time,
)
from .model_format import fill_amount_and_currency
from .model_media import TodoListItem, parse_todo_list_item


@dataclass
class ActionChatCreate:
    title: str = ""
    user_ids: list[int] = field(default_factory=list)


@dataclass
class ActionChatEditTitle:
    title: str = ""


@dataclass
class ActionChatEditPhoto:
    photo: Photo = field(default_factory=Photo)


@dataclass
class ActionChatDeletePhoto:
    pass


@dataclass
class ActionChatAddUser:
    user_ids: list[int] = field(default_factory=list)


@dataclass
class ActionChatDeleteUser:
    user_id: int = 0


@dataclass
class ActionChatJoinedByLink:
    inviter_id: int = 0


@dataclass
class ActionChannelCreate:
    title: str = ""


@dataclass
class ActionChatMigrateTo:
    channel_id: int = 0


@dataclass
class ActionChannelMigrateFrom:
    title: str = ""
    chat_id: int = 0


@dataclass
class ActionPinMessage:
    pass


@dataclass
class ActionHistoryClear:
    pass


@dataclass
class ActionGameScore:
    game_id: int = 0
    score: int = 0


@dataclass
class ActionPaymentSent:
    currency: str = ""
    amount: int = 0
    recurring_init: bool = False
    recurring_used: bool = False


@dataclass
class ActionPhoneCall:
    class State(Enum):
        Unknown = 0
        Missed = 1
        Disconnect = 2
        Hangup = 3
        Busy = 4
        MigrateConferenceCall = 5
        Invitation = 6
        Active = 7

    conference_id: int = 0
    state: ActionPhoneCall.State = State.Unknown
    duration: int = 0


@dataclass
class ActionScreenshotTaken:
    pass


@dataclass
class ActionCustomAction:
    message: str = ""


@dataclass
class ActionBotAllowed:
    app_id: int = 0
    app: str = ""
    domain: str = ""
    attach_menu: bool = False
    from_request: bool = False


@dataclass
class ActionSecureValuesSent:
    class Type(Enum):
        PersonalDetails = 0
        Passport = 1
        DriverLicense = 2
        IdentityCard = 3
        InternalPassport = 4
        Address = 5
        UtilityBill = 6
        BankStatement = 7
        RentalAgreement = 8
        PassportRegistration = 9
        TemporaryRegistration = 10
        Phone = 11
        Email = 12

    types: list[ActionSecureValuesSent.Type] = field(default_factory=list)


@dataclass
class ActionContactSignUp:
    pass


@dataclass
class ActionPhoneNumberRequest:
    pass


@dataclass
class ActionGeoProximityReached:
    from_id: int = 0
    to_id: int = 0
    distance: int = 0
    from_self: bool = False
    to_self: bool = False


@dataclass
class ActionGroupCall:
    duration: int = 0


@dataclass
class ActionInviteToGroupCall:
    user_ids: list[int] = field(default_factory=list)


@dataclass
class ActionSetMessagesTTL:
    period: int = 0


@dataclass
class ActionGroupCallScheduled:
    date: int = 0


@dataclass
class ActionSetChatTheme:
    emoji: str = ""


@dataclass
class ActionChatJoinedByRequest:
    pass


@dataclass
class ActionChatJoinedViaCommunity:
    community_id: int = 0


@dataclass
class ActionWebViewDataSent:
    text: str = ""


@dataclass
class ActionGiftPremium:
    cost: str = ""
    days: int = 0


@dataclass
class ActionTopicCreate:
    title: str = ""


@dataclass
class ActionTopicEdit:
    title: str = ""
    # std::optional<uint64> iconEmojiId = 0: engaged with 0 unless the action carries one.
    icon_emoji_id: int | None = 0


@dataclass
class ActionSuggestProfilePhoto:
    photo: Photo = field(default_factory=Photo)


@dataclass
class ActionSetChatWallPaper:
    same: bool = False
    both: bool = False


@dataclass
class ActionGiftCode:
    code: str = ""
    boost_peer_id: int = 0
    days: int = 0
    via_giveaway: bool = False
    unclaimed: bool = False


@dataclass
class ActionRequestedPeer:
    peers: list[int] = field(default_factory=list)
    button_id: int = 0


@dataclass
class ActionGiveawayLaunch:
    pass


@dataclass
class ActionGiveawayResults:
    winners: int = 0
    unclaimed: int = 0
    credits: bool = False


@dataclass
class ActionBoostApply:
    boosts: int = 0


@dataclass
class ActionPaymentRefunded:
    peer_id: int = 0
    currency: str = ""
    amount: int = 0
    transaction_id: str = ""


@dataclass
class ActionGiftCredits:
    cost: str = ""
    amount: CreditsAmount = field(default_factory=CreditsAmount)


@dataclass
class ActionPrizeStars:
    peer_id: int = 0
    amount: int = 0
    transaction_id: str = ""
    giveaway_msg_id: int = 0
    is_unclaimed: bool = False


@dataclass
class ActionStarGift:
    gift_id: int = 0
    stars: int = 0
    text: list[TextPart] = field(default_factory=list)
    anonymous: bool = False
    limited: bool = False
    offer_price: CreditsAmount = field(default_factory=CreditsAmount)
    offer_expire_at: int = 0
    offer: bool = False
    offer_accepted: bool = False
    offer_declined: bool = False
    offer_expired: bool = False


@dataclass
class ActionPaidMessagesRefunded:
    messages: int = 0
    stars: int = 0


@dataclass
class ActionPaidMessagesPrice:
    stars: int = 0
    broadcast_allowed: bool = False


@dataclass
class ActionTodoCompletions:
    completed: list[int] = field(default_factory=list)
    incompleted: list[int] = field(default_factory=list)


@dataclass
class ActionTodoAppendTasks:
    items: list[TodoListItem] = field(default_factory=list)


@dataclass
class ActionPollAppendAnswer:
    option: str = ""


@dataclass
class ActionPollDeleteAnswer:
    option: str = ""


@dataclass
class ActionSuggestedPostApproval:
    reject_comment: str = ""
    schedule_date: int = 0
    price: CreditsAmount = field(default_factory=CreditsAmount)
    rejected: bool = False
    balance_too_low: bool = False


@dataclass
class ActionSuggestedPostSuccess:
    price: CreditsAmount = field(default_factory=CreditsAmount)


@dataclass
class ActionSuggestedPostRefund:
    payer_initiated: bool = False


@dataclass
class ActionSuggestBirthday:
    birthday: Birthday = field(default_factory=Birthday)


@dataclass
class ActionNoForwardsToggle:
    new_value: bool = False


@dataclass
class ActionNoForwardsRequest:
    expired: bool = False
    new_value: bool = False


@dataclass
class ActionNewCreatorPending:
    new_creator_id: int = 0


@dataclass
class ActionChangeCreator:
    new_creator_id: int = 0


@dataclass
class ActionManagedBotCreated:
    bot_id: int = 0


@dataclass
class ServiceAction:
    # One of the Action* dataclasses above, or None (v::null_t).
    content: Any = None


_PHONE_STATES = {
    tl.PhoneCallDiscardReasonMissed: ActionPhoneCall.State.Missed,
    tl.PhoneCallDiscardReasonDisconnect: ActionPhoneCall.State.Disconnect,
    tl.PhoneCallDiscardReasonHangup: ActionPhoneCall.State.Hangup,
    tl.PhoneCallDiscardReasonBusy: ActionPhoneCall.State.Busy,
    tl.PhoneCallDiscardReasonMigrateConferenceCall: ActionPhoneCall.State.MigrateConferenceCall,
}

_SV = ActionSecureValuesSent.Type
_SECURE_TYPES = {
    tl.SecureValueTypePersonalDetails: _SV.PersonalDetails,
    tl.SecureValueTypePassport: _SV.Passport,
    tl.SecureValueTypeDriverLicense: _SV.DriverLicense,
    tl.SecureValueTypeIdentityCard: _SV.IdentityCard,
    tl.SecureValueTypeInternalPassport: _SV.InternalPassport,
    tl.SecureValueTypeAddress: _SV.Address,
    tl.SecureValueTypeUtilityBill: _SV.UtilityBill,
    tl.SecureValueTypeBankStatement: _SV.BankStatement,
    tl.SecureValueTypeRentalAgreement: _SV.RentalAgreement,
    tl.SecureValueTypePassportRegistration: _SV.PassportRegistration,
    tl.SecureValueTypeTemporaryRegistration: _SV.TemporaryRegistration,
    tl.SecureValueTypePhone: _SV.Phone,
    tl.SecureValueTypeEmail: _SV.Email,
}


def parse_star_gift(gift: Any) -> ActionStarGift:
    if isinstance(gift, tl.StarGift):
        return ActionStarGift(gift_id=gift.id, stars=gift.stars, limited=bool(gift.limited))
    return ActionStarGift(gift_id=gift.id)


def _poll_answer_option(answer: Any) -> str:
    return answer.text.text if isinstance(answer, tl.PollAnswer) else ""


def _phone_call(data: tl.MessageActionPhoneCall) -> ActionPhoneCall:
    result = ActionPhoneCall(duration=data.duration or 0)
    if data.reason is not None:
        result.state = _PHONE_STATES.get(type(data.reason), ActionPhoneCall.State.Unknown)
    return result


def _conference_call(data: tl.MessageActionConferenceCall) -> ActionPhoneCall:
    state = ActionPhoneCall.State
    return ActionPhoneCall(
        conference_id=data.call_id,
        duration=data.duration or 0,
        state=(
            state.Missed
            if data.missed
            else (
                state.Active
                if data.active
                else state.Hangup if data.duration else state.Invitation
            )
        ),
    )


def _bot_allowed(data: tl.MessageActionBotAllowed) -> ActionBotAllowed:
    result = ActionBotAllowed(
        attach_menu=bool(data.attach_menu),
        from_request=bool(data.from_request),
        domain=data.domain or "",
    )
    if isinstance(data.app, tl.BotApp):
        result.app_id, result.app = data.app.id, data.app.title
    return result


def _geo_proximity(context: ParseMediaContext, data: Any) -> ActionGeoProximityReached:
    result = ActionGeoProximityReached(
        from_id=parse_peer_id(data.from_id),
        to_id=parse_peer_id(data.to_id),
        distance=data.distance,
    )
    result.from_self = result.from_id == context.self_peer_id
    result.to_self = result.to_id == context.self_peer_id
    return result


def _chat_theme(data: tl.MessageActionSetChatTheme) -> ActionSetChatTheme | None:
    if isinstance(data.theme, tl.ChatTheme):
        return ActionSetChatTheme(emoji=data.theme.emoticon)
    if isinstance(data.theme, tl.ChatThemeUniqueGift):
        return ActionSetChatTheme()
    return None


def _topic_edit(data: tl.MessageActionTopicEdit) -> ActionTopicEdit:
    result = ActionTopicEdit(title=data.title or "")
    if data.icon_emoji_id is not None:
        result.icon_emoji_id = data.icon_emoji_id
    return result


def _gift_ton(data: tl.MessageActionGiftTon) -> ActionGiftCredits:
    # tdesktop splits the fiat `amount` as nano-TON, uint64 arithmetic included.
    unsigned = data.amount & 0xFFFFFFFFFFFFFFFF
    return ActionGiftCredits(
        cost=fill_amount_and_currency(data.amount, data.currency),
        amount=CreditsAmount.make(
            unsigned // 1_000_000_000, unsigned % 1_000_000_000, CreditsType.Ton
        ),
    )


def _star_gift(data: tl.MessageActionStarGift) -> ActionStarGift:
    result = parse_star_gift(data.gift)
    if data.message is not None:
        result.text = parse_text(data.message.text, data.message.entities)
    result.anonymous = bool(data.name_hidden)
    return result


def _purchase_offer(data: tl.MessageActionStarGiftPurchaseOffer) -> ActionStarGift:
    result = parse_star_gift(data.gift)
    result.offer = True
    result.offer_price = credits_amount_from_tl(data.price)
    result.offer_expire_at = to_time(data.expires_at)
    result.offer_accepted = bool(data.accepted)
    result.offer_declined = bool(data.declined)
    return result


def _purchase_offer_declined(
    data: tl.MessageActionStarGiftPurchaseOfferDeclined,
) -> ActionStarGift:
    result = parse_star_gift(data.gift)
    result.offer = True
    result.offer_declined = True
    result.offer_expired = bool(data.expired)
    result.offer_price = credits_amount_from_tl(data.price)
    return result


_Parser = Callable[[ParseMediaContext, Any, str, int], Any]
_PARSERS: dict[type, _Parser] = {
    tl.MessageActionChatCreate: lambda c, d, f, t: ActionChatCreate(d.title, list(d.users)),
    tl.MessageActionChatEditTitle: lambda c, d, f, t: ActionChatEditTitle(d.title),
    tl.MessageActionChatEditPhoto: lambda c, d, f, t: ActionChatEditPhoto(_photo(c, d, f, t)),
    tl.MessageActionChatDeletePhoto: lambda c, d, f, t: ActionChatDeletePhoto(),
    tl.MessageActionChatAddUser: lambda c, d, f, t: ActionChatAddUser(list(d.users)),
    tl.MessageActionChatDeleteUser: lambda c, d, f, t: ActionChatDeleteUser(d.user_id),
    tl.MessageActionChatJoinedByLink: lambda c, d, f, t: ActionChatJoinedByLink(d.inviter_id),
    tl.MessageActionChannelCreate: lambda c, d, f, t: ActionChannelCreate(d.title),
    tl.MessageActionChatMigrateTo: lambda c, d, f, t: ActionChatMigrateTo(d.channel_id),
    tl.MessageActionChannelMigrateFrom: lambda c, d, f, t: ActionChannelMigrateFrom(
        d.title, d.chat_id
    ),
    tl.MessageActionPinMessage: lambda c, d, f, t: ActionPinMessage(),
    tl.MessageActionHistoryClear: lambda c, d, f, t: ActionHistoryClear(),
    tl.MessageActionGameScore: lambda c, d, f, t: ActionGameScore(d.game_id, d.score),
    # PaymentSentMe, SecureValuesSentMe, WebViewDataSentMe, RequestedPeerSentMe: "Should not be
    # in user inbox", so they stay null like MessageActionEmpty and ChangeCommunity.
    tl.MessageActionPaymentSent: lambda c, d, f, t: ActionPaymentSent(
        d.currency, d.total_amount, bool(d.recurring_init), bool(d.recurring_used)
    ),
    tl.MessageActionPhoneCall: lambda c, d, f, t: _phone_call(d),
    tl.MessageActionScreenshotTaken: lambda c, d, f, t: ActionScreenshotTaken(),
    tl.MessageActionCustomAction: lambda c, d, f, t: ActionCustomAction(d.message),
    tl.MessageActionBotAllowed: lambda c, d, f, t: _bot_allowed(d),
    tl.MessageActionSecureValuesSent: lambda c, d, f, t: ActionSecureValuesSent(
        [_SECURE_TYPES[type(v)] for v in d.types]
    ),
    tl.MessageActionContactSignUp: lambda c, d, f, t: ActionContactSignUp(),
    tl.MessageActionGeoProximityReached: lambda c, d, f, t: _geo_proximity(c, d),
    tl.MessageActionGroupCall: lambda c, d, f, t: ActionGroupCall(d.duration or 0),
    tl.MessageActionInviteToGroupCall: lambda c, d, f, t: ActionInviteToGroupCall(list(d.users)),
    tl.MessageActionSetMessagesTTL: lambda c, d, f, t: ActionSetMessagesTTL(d.period),
    tl.MessageActionGroupCallScheduled: lambda c, d, f, t: ActionGroupCallScheduled(
        to_time(d.schedule_date)
    ),
    tl.MessageActionSetChatTheme: lambda c, d, f, t: _chat_theme(d),
    tl.MessageActionChatJoinedByRequest: lambda c, d, f, t: ActionChatJoinedByRequest(),
    tl.MessageActionWebViewDataSent: lambda c, d, f, t: ActionWebViewDataSent(d.text),
    tl.MessageActionGiftPremium: lambda c, d, f, t: ActionGiftPremium(
        fill_amount_and_currency(d.amount, d.currency), d.days
    ),
    tl.MessageActionTopicCreate: lambda c, d, f, t: ActionTopicCreate(d.title),
    tl.MessageActionTopicEdit: lambda c, d, f, t: _topic_edit(d),
    tl.MessageActionSuggestProfilePhoto: lambda c, d, f, t: ActionSuggestProfilePhoto(
        _photo(c, d, f, t)
    ),
    tl.MessageActionSetChatWallPaper: lambda c, d, f, t: ActionSetChatWallPaper(
        bool(d.same), bool(d.for_both)
    ),
    tl.MessageActionRequestedPeer: lambda c, d, f, t: ActionRequestedPeer(
        [parse_peer_id(p) for p in d.peers], d.button_id
    ),
    tl.MessageActionGiftCode: lambda c, d, f, t: ActionGiftCode(
        code=d.slug,
        boost_peer_id=parse_peer_id(d.boost_peer) if d.boost_peer is not None else 0,
        days=d.days,
        via_giveaway=bool(d.via_giveaway),
        unclaimed=bool(d.unclaimed),
    ),
    tl.MessageActionGiveawayLaunch: lambda c, d, f, t: ActionGiveawayLaunch(),
    tl.MessageActionGiveawayResults: lambda c, d, f, t: ActionGiveawayResults(
        d.winners_count, d.unclaimed_count, bool(d.stars)
    ),
    tl.MessageActionBoostApply: lambda c, d, f, t: ActionBoostApply(d.boosts),
    tl.MessageActionPaymentRefunded: lambda c, d, f, t: ActionPaymentRefunded(
        parse_peer_id(d.peer), d.currency, d.total_amount, d.charge.id
    ),
    tl.MessageActionGiftStars: lambda c, d, f, t: ActionGiftCredits(
        fill_amount_and_currency(d.amount, d.currency),
        CreditsAmount.make(d.stars, 0, CreditsType.Stars),
    ),
    tl.MessageActionGiftTon: lambda c, d, f, t: _gift_ton(d),
    tl.MessageActionPrizeStars: lambda c, d, f, t: ActionPrizeStars(
        parse_peer_id(d.boost_peer),
        d.stars,
        d.transaction_id,
        d.giveaway_msg_id,
        bool(d.unclaimed),
    ),
    tl.MessageActionStarGift: lambda c, d, f, t: _star_gift(d),
    tl.MessageActionStarGiftUnique: lambda c, d, f, t: parse_star_gift(d.gift),
    tl.MessageActionPaidMessagesRefunded: lambda c, d, f, t: ActionPaidMessagesRefunded(
        d.count, d.stars
    ),
    tl.MessageActionPaidMessagesPrice: lambda c, d, f, t: ActionPaidMessagesPrice(
        d.stars, bool(d.broadcast_messages_allowed)
    ),
    tl.MessageActionTodoCompletions: lambda c, d, f, t: ActionTodoCompletions(
        list(d.completed), list(d.incompleted)
    ),
    tl.MessageActionTodoAppendTasks: lambda c, d, f, t: ActionTodoAppendTasks(
        [parse_todo_list_item(item) for item in d.list]
    ),
    # tdesktop matches PollAppendAnswer twice; the first (empty) handler is the one that runs.
    tl.MessageActionPollAppendAnswer: lambda c, d, f, t: ActionPollAppendAnswer(),
    tl.MessageActionSuggestedPostApproval: lambda c, d, f, t: ActionSuggestedPostApproval(
        reject_comment=d.reject_comment or "",
        schedule_date=to_time(d.schedule_date),
        price=credits_amount_from_tl(d.price),
        rejected=bool(d.rejected),
        balance_too_low=bool(d.balance_too_low),
    ),
    tl.MessageActionSuggestedPostSuccess: lambda c, d, f, t: ActionSuggestedPostSuccess(
        credits_amount_from_tl(d.price)
    ),
    tl.MessageActionSuggestedPostRefund: lambda c, d, f, t: ActionSuggestedPostRefund(
        bool(d.payer_initiated)
    ),
    tl.MessageActionConferenceCall: lambda c, d, f, t: _conference_call(d),
    tl.MessageActionSuggestBirthday: lambda c, d, f, t: ActionSuggestBirthday(
        Birthday.make(d.birthday.day, d.birthday.month, d.birthday.year or 0)
    ),
    tl.MessageActionStarGiftPurchaseOffer: lambda c, d, f, t: _purchase_offer(d),
    tl.MessageActionStarGiftPurchaseOfferDeclined: lambda c, d, f, t: _purchase_offer_declined(d),
    tl.MessageActionNewCreatorPending: lambda c, d, f, t: ActionNewCreatorPending(
        d.new_creator_id
    ),
    tl.MessageActionChangeCreator: lambda c, d, f, t: ActionChangeCreator(d.new_creator_id),
    tl.MessageActionNoForwardsToggle: lambda c, d, f, t: ActionNoForwardsToggle(bool(d.new_value)),
    tl.MessageActionNoForwardsRequest: lambda c, d, f, t: ActionNoForwardsRequest(
        bool(d.expired), bool(d.new_value)
    ),
    tl.MessageActionManagedBotCreated: lambda c, d, f, t: ActionManagedBotCreated(d.bot_id),
    tl.MessageActionPollDeleteAnswer: lambda c, d, f, t: ActionPollDeleteAnswer(
        _poll_answer_option(d.answer)
    ),
    tl.MessageActionChatJoinedViaCommunity: lambda c, d, f, t: ActionChatJoinedViaCommunity(
        d.community_id
    ),
}


def _photo(context: ParseMediaContext, data: Any, media_folder: str, date: int) -> Photo:
    context.photos += 1
    name = prepare_photo_file_name(context.photos, date)
    return parse_photo(data.photo, media_folder + "photos/" + name)


def parse_service_action(
    context: ParseMediaContext, data: Any, media_folder: str, date: int
) -> ServiceAction:
    parser = _PARSERS.get(type(data))
    return ServiceAction(parser(context, data, media_folder, date) if parser else None)
