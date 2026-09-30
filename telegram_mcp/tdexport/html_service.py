"""Service message texts: the English sentence tdesktop writes for every service action.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
HtmlWriter::Wrap::pushMessage, the `serviceText` v::match), GPL-3.0. An empty result means the
message is not a service line (a phone call, or no action at all).
"""

from __future__ import annotations

from typing import Any, Callable

from . import model_actions as act
from .html_text import (
    PeersMap,
    format_date_text,
    format_text,
    format_time_text,
    month_name,
    serialize_list,
    serialize_string,
)
from .model_dialogs import DialogInfo
from .model_format import format_date_time, format_money_amount, number_to_string
from .model_message import Message

WrapLink = Callable[[str], str]

_SECURE_NAMES = {
    act.ActionSecureValuesSent.Type.PersonalDetails: "Personal details",
    act.ActionSecureValuesSent.Type.Passport: "Passport",
    act.ActionSecureValuesSent.Type.DriverLicense: "Driver license",
    act.ActionSecureValuesSent.Type.IdentityCard: "Identity card",
    act.ActionSecureValuesSent.Type.InternalPassport: "Internal passport",
    act.ActionSecureValuesSent.Type.Address: "Address information",
    act.ActionSecureValuesSent.Type.UtilityBill: "Utility bill",
    act.ActionSecureValuesSent.Type.BankStatement: "Bank statement",
    act.ActionSecureValuesSent.Type.RentalAgreement: "Rental agreement",
    act.ActionSecureValuesSent.Type.PassportRegistration: "Passport registration",
    act.ActionSecureValuesSent.Type.TemporaryRegistration: "Temporary registration",
    act.ActionSecureValuesSent.Type.Phone: "Phone number",
    act.ActionSecureValuesSent.Type.Email: "Email",
}


def qt_number(value: float) -> str:
    """QString::number(double): format 'g', precision 6."""
    return f"{value:g}"


def _quoted(title: str) -> str:
    return "&laquo;" + serialize_string(title) + "&raquo;"


def _days(days: int) -> str:
    return number_to_string(days) + (" days" if days > 1 else " day")


def _distance(meters: int) -> str:
    if meters >= 1000:
        return qt_number((10 * (meters // 10)) / 1000.0) + " km"
    if meters == 1:
        return "1 meter"
    return str(meters) + " meters"


def _todo_list(values: list[int]) -> str:
    items = [str(value) for value in values]
    if not items:
        return ""
    if len(items) > 1:
        return ", ".join(items[:-1]) + " and " + items[-1]
    return items[0]


class _ServiceText:
    def __init__(
        self,
        message: Message,
        dialog: DialogInfo,
        peers: PeersMap,
        internal_links_domain: str,
        relative_base: str,
        link: WrapLink,
    ) -> None:
        self.message = message
        self.dialog = dialog
        self.peers = peers
        self.domain = internal_links_domain
        self.base = relative_base
        self.link = link
        self.from_ = peers.wrap_peer_name(message.from_id)
        channel = (DialogInfo.Type.PrivateChannel, DialogInfo.Type.PublicChannel)
        self.is_channel = dialog.type in channel

    def text(self, data: Any) -> str:
        handler = getattr(self, "_" + type(data).__name__, None)
        return handler(data) if handler is not None else ""

    def _ActionChatCreate(self, data: act.ActionChatCreate) -> str:
        members = " with members " + self.peers.wrap_user_names(data.user_ids)
        return (
            self.from_
            + " created group "
            + _quoted(data.title)
            + (members if data.user_ids else "")
        )

    def _ActionChatEditTitle(self, data: act.ActionChatEditTitle) -> str:
        if self.is_channel:
            return "Channel title changed to " + _quoted(data.title)
        return self.from_ + " changed group title to " + _quoted(data.title)

    def _ActionChatEditPhoto(self, data: act.ActionChatEditPhoto) -> str:
        return "Channel photo changed" if self.is_channel else self.from_ + " changed group photo"

    def _ActionChatDeletePhoto(self, data: act.ActionChatDeletePhoto) -> str:
        return "Channel photo removed" if self.is_channel else self.from_ + " removed group photo"

    def _ActionChatAddUser(self, data: act.ActionChatAddUser) -> str:
        return self.from_ + " invited " + self.peers.wrap_user_names(data.user_ids)

    def _ActionChatDeleteUser(self, data: act.ActionChatDeleteUser) -> str:
        return self.from_ + " removed " + self.peers.wrap_user_name(data.user_id)

    def _ActionChatJoinedByLink(self, data: act.ActionChatJoinedByLink) -> str:
        return (
            self.from_ + " joined group by link from " + self.peers.wrap_user_name(data.inviter_id)
        )

    def _ActionChannelCreate(self, data: act.ActionChannelCreate) -> str:
        return "Channel " + _quoted(data.title) + " created"

    def _ActionChatMigrateTo(self, data: act.ActionChatMigrateTo) -> str:
        return self.from_ + " converted this group to a supergroup"

    def _ActionChannelMigrateFrom(self, data: act.ActionChannelMigrateFrom) -> str:
        return self.from_ + " converted a basic group to this supergroup " + _quoted(data.title)

    def _ActionPinMessage(self, data: act.ActionPinMessage) -> str:
        return self.from_ + " pinned " + self.link("this message")

    def _ActionHistoryClear(self, data: act.ActionHistoryClear) -> str:
        return "History cleared"

    def _ActionGameScore(self, data: act.ActionGameScore) -> str:
        score = number_to_string(data.score)
        return self.from_ + " scored " + score + " in " + self.link("this game")

    def _ActionPaymentSent(self, data: act.ActionPaymentSent) -> str:
        amount = format_money_amount(data.amount, data.currency)
        if data.recurring_used:
            return "You were charged " + amount + " via recurring payment"
        result = "You have successfully transferred " + amount + " for "
        result += self.link("this invoice")
        if data.recurring_init:
            result += " and allowed future recurring payments"
        return result

    def _ActionScreenshotTaken(self, data: act.ActionScreenshotTaken) -> str:
        return self.from_ + " took a screenshot"

    def _ActionCustomAction(self, data: act.ActionCustomAction) -> str:
        return data.message

    def _ActionBotAllowed(self, data: act.ActionBotAllowed) -> str:
        allowed = "You allowed this bot to message you "
        if data.attach_menu:
            return allowed + "when you added it in the attachment menu."
        if data.from_request:
            return allowed + "in his web-app."
        if not data.app:
            return allowed + "when you opened " + serialize_string(data.app)
        return allowed + "when you logged in on " + serialize_string(data.domain)

    def _ActionSecureValuesSent(self, data: act.ActionSecureValuesSent) -> str:
        names = [_SECURE_NAMES.get(kind, "") for kind in data.types]
        return "You have sent the following documents: " + serialize_list(names)

    def _ActionContactSignUp(self, data: act.ActionContactSignUp) -> str:
        return self.from_ + " joined Telegram"

    def _ActionGeoProximityReached(self, data: act.ActionGeoProximityReached) -> str:
        from_name = self.peers.wrap_peer_name(data.from_id)
        to_name = self.peers.wrap_peer_name(data.to_id)
        distance = _distance(data.distance)
        if data.from_self:
            return "You are now within " + distance + " from " + to_name
        if data.to_self:
            return from_name + " is now within " + distance + " from you"
        return from_name + " is now within " + distance + " from " + to_name

    def _ActionPhoneNumberRequest(self, data: act.ActionPhoneNumberRequest) -> str:
        return self.from_ + " requested your phone number"

    def _ActionGroupCall(self, data: act.ActionGroupCall) -> str:
        duration = " (" + str(data.duration) + " seconds)" if data.duration else ""
        if self.is_channel:
            return "Voice chat" + duration
        return self.from_ + " started voice chat" + duration

    def _ActionInviteToGroupCall(self, data: act.ActionInviteToGroupCall) -> str:
        names = self.peers.wrap_user_names(data.user_ids)
        return self.from_ + " invited " + names + " to the voice chat"

    def _ActionSetMessagesTTL(self, data: act.ActionSetMessagesTTL) -> str:
        period = {7 * 86400: "7 days", 86400: "24 hours"}.get(data.period, "")
        if self.is_channel:
            if data.period:
                return "New messages will auto-delete in " + period
            return "New messages will not auto-delete"
        if data.period:
            return self.from_ + " has set messages to auto-delete in " + period
        return self.from_ + " has set messages not to auto-delete"

    def _ActionGroupCallScheduled(self, data: act.ActionGroupCallScheduled) -> str:
        date = format_date_time(data.date)
        if self.is_channel:
            return "Voice chat scheduled for " + date
        return self.from_ + " scheduled a voice chat for " + date

    def _ActionSetChatTheme(self, data: act.ActionSetChatTheme) -> str:
        if not data.emoji:
            if self.is_channel:
                return "Channel theme was disabled"
            return self.from_ + " disabled chat theme"
        if self.is_channel:
            return "Channel theme was changed to " + data.emoji
        return self.from_ + " changed chat theme to " + data.emoji

    def _ActionChatJoinedByRequest(self, data: act.ActionChatJoinedByRequest) -> str:
        return self.from_ + " joined group by request"

    def _ActionChatJoinedViaCommunity(self, data: act.ActionChatJoinedViaCommunity) -> str:
        return self.from_ + " joined group via a community"

    def _ActionWebViewDataSent(self, data: act.ActionWebViewDataSent) -> str:
        return (
            "You have just successfully transferred data from the "
            + _quoted(data.text)
            + " button to the bot"
        )

    def _ActionGiftPremium(self, data: act.ActionGiftPremium) -> str:
        if not data.days or not data.cost:
            return self.from_ + " sent you a gift."
        return (
            self.from_
            + " sent you a gift for "
            + data.cost
            + ": Telegram Premium for "
            + str(data.days)
            + " days."
        )

    def _ActionTopicCreate(self, data: act.ActionTopicCreate) -> str:
        return self.from_ + " created topic " + _quoted(data.title)

    def _ActionTopicEdit(self, data: act.ActionTopicEdit) -> str:
        parts = []
        if data.title:
            parts.append("title to " + _quoted(data.title))
        if data.icon_emoji_id is not None:
            parts.append("icon to &laquo;" + str(data.icon_emoji_id) + "&raquo;")
        return self.from_ + " changed topic " + ",".join(parts)

    def _ActionSuggestProfilePhoto(self, data: act.ActionSuggestProfilePhoto) -> str:
        return self.from_ + " suggests to use this photo"

    def _ActionRequestedPeer(self, data: act.ActionRequestedPeer) -> str:
        return "requested: "

    def _ActionSetChatWallPaper(self, data: act.ActionSetChatWallPaper) -> str:
        if data.same:
            return self.from_ + " set " + self.link("the same background") + " for this chat"
        return self.from_ + " set a new background for this chat"

    def _ActionGiftCode(self, data: act.ActionGiftCode) -> str:
        days = _days(data.days)
        if data.unclaimed:
            return (
                "This is an unclaimed Telegram Premium for "
                + days
                + " prize in a giveaway organized by a channel."
            )
        if data.via_giveaway:
            return (
                "You won a Telegram Premium for "
                + days
                + " prize in a giveaway organized by a channel."
            )
        return "You've received a Telegram Premium for " + days + " gift from a channel."

    def _ActionGiveawayLaunch(self, data: act.ActionGiveawayLaunch) -> str:
        return (
            self.from_
            + " just started a giveaway of Telegram Premium subscriptions to its followers."
        )

    def _ActionGiveawayResults(self, data: act.ActionGiveawayResults) -> str:
        selected = "Some winners of the giveaway were randomly selected by Telegram and received "
        if not data.winners:
            return "No winners of the giveaway could be selected."
        if data.credits and data.unclaimed:
            return selected + "their prize."
        if not data.credits and data.unclaimed:
            return selected + "private messages with giftcodes."
        winners = number_to_string(data.winners)
        chosen = " of the giveaway was randomly selected by Telegram and received "
        if data.credits:
            return winners + chosen + "their prize."
        return winners + chosen + "private messages with giftcodes."

    def _ActionBoostApply(self, data: act.ActionBoostApply) -> str:
        times = " times" if data.boosts > 1 else " time"
        return self.from_ + " boosted the group " + str(data.boosts) + times

    def _ActionPaymentRefunded(self, data: act.ActionPaymentRefunded) -> str:
        amount = format_money_amount(data.amount, data.currency)
        return self.peers.wrap_peer_name(data.peer_id) + " refunded back " + amount

    def _ActionGiftCredits(self, data: act.ActionGiftCredits) -> str:
        if not data.amount or not data.cost:
            return self.from_ + " sent you a gift."
        unit = " TON." if data.amount.ton() else " Telegram Stars."
        return (
            self.from_
            + " sent you a gift for "
            + data.cost
            + ": "
            + qt_number(data.amount.value())
            + unit
        )

    def _ActionPrizeStars(self, data: act.ActionPrizeStars) -> str:
        return (
            "You won a prize in a giveaway organized by "
            + self.peers.wrap_peer_name(data.peer_id)
            + ".\n Your prize is "
            + str(data.amount)
            + " Telegram Stars."
        )

    def _ActionStarGift(self, data: act.ActionStarGift) -> str:
        return self.from_ + " sent you a gift of " + str(data.stars) + " Telegram Stars."

    def _ActionPaidMessagesRefunded(self, data: act.ActionPaidMessagesRefunded) -> str:
        peer = self.peers.wrap_peer_name(self.dialog.peer_id)
        stars, messages = str(data.stars), str(data.messages)
        if self.message.out:
            return "You refunded " + stars + " Stars for " + messages + " messages to " + peer
        return peer + " refunded " + stars + " Stars for " + messages + " messages to you"

    def _ActionPaidMessagesPrice(self, data: act.ActionPaidMessagesPrice) -> str:
        stars = str(data.stars)
        if self.is_channel:
            if not data.broadcast_allowed:
                return "Direct messages were disabled."
            return "Price per direct message changed to " + stars + " Telegram Stars."
        return "Price per message changed to " + stars + " Telegram Stars."

    def _ActionTodoCompletions(self, data: act.ActionTodoCompletions) -> str:
        completed, incompleted = _todo_list(data.completed), _todo_list(data.incompleted)
        link = self.link("this todo list") + "."
        if not data.completed and data.incompleted:
            return self.from_ + " marked " + incompleted + " as not done yet in " + link
        if data.completed and not data.incompleted:
            return self.from_ + " marked " + completed + " as done in " + link
        return (
            self.from_
            + " marked "
            + completed
            + " as done and "
            + incompleted
            + " as not done yet in "
            + link
        )

    def _ActionTodoAppendTasks(self, data: act.ActionTodoAppendTasks) -> str:
        tasks = [
            "&quot;" + format_text(task.text, self.domain, self.base) + "&quot;"
            for task in data.items
        ]
        return self.from_ + " added tasks: " + ", ".join(tasks)

    def _ActionPollAppendAnswer(self, data: act.ActionPollAppendAnswer) -> str:
        return self.from_ + " added &quot;" + data.option + "&quot; to the poll."

    def _ActionPollDeleteAnswer(self, data: act.ActionPollDeleteAnswer) -> str:
        return self.from_ + " removed &quot;" + data.option + "&quot; from the poll."

    def _ActionSuggestedPostApproval(self, data: act.ActionSuggestedPostApproval) -> str:
        result = self.from_ + (" rejected " if data.rejected else " approved ")
        result += "your suggested post"
        if data.price:
            unit = " TON" if data.price.ton() else " stars"
            result += ", for " + qt_number(data.price.value()) + unit
        if data.schedule_date:
            date = data.schedule_date
            result += ", " + format_date_text(date) + " at " + format_time_text(date)
        if not data.reject_comment:
            return result + "."
        return result + ", with comment: &quot;" + serialize_string(data.reject_comment) + "&quot;"

    def _ActionSuggestedPostSuccess(self, data: act.ActionSuggestedPostSuccess) -> str:
        unit = " TON" if data.price.ton() else " stars"
        return (
            "The paid post was shown for 24 hours and "
            + qt_number(data.price.value())
            + unit
            + " were transferred to the channel."
        )

    def _ActionSuggestedPostRefund(self, data: act.ActionSuggestedPostRefund) -> str:
        if data.payer_initiated:
            return "The user refunded the payment, post was deleted."
        return "The admin deleted the post early, the payment was refunded."

    def _ActionSuggestBirthday(self, data: act.ActionSuggestBirthday) -> str:
        birthday = data.birthday
        month = birthday.month()
        name = " " + month_name(month) if 1 <= month <= 12 else ""
        year = " " + str(birthday.year()) if birthday.year() else ""
        return (
            self.from_ + " suggests to add a date of birth: " + str(birthday.day()) + name + year
        )

    def _ActionNoForwardsToggle(self, data: act.ActionNoForwardsToggle) -> str:
        state = " disabled" if data.new_value else " enabled"
        return self.from_ + state + " sharing in this chat"

    def _ActionNoForwardsRequest(self, data: act.ActionNoForwardsRequest) -> str:
        return self.from_ + " requested to enable sharing in this chat"

    def _ActionNewCreatorPending(self, data: act.ActionNewCreatorPending) -> str:
        return (
            self.peers.wrap_user_name(data.new_creator_id)
            + " will become the new main admin in 7 days if "
            + self.from_
            + " does not return"
        )

    def _ActionChangeCreator(self, data: act.ActionChangeCreator) -> str:
        return (
            self.from_
            + " made "
            + self.peers.wrap_user_name(data.new_creator_id)
            + " the new main admin of the group"
        )

    def _ActionManagedBotCreated(self, data: act.ActionManagedBotCreated) -> str:
        return self.from_ + " created a bot " + self.peers.wrap_user_name(data.bot_id)


def service_text(
    message: Message,
    dialog: DialogInfo,
    peers: PeersMap,
    internal_links_domain: str,
    relative_base: str,
    wrap_reply_to_link: WrapLink,
) -> str:
    return _ServiceText(
        message, dialog, peers, internal_links_domain, relative_base, wrap_reply_to_link
    ).text(message.action.content)
