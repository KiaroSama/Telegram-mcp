"""Message media blocks without files: polls, to-do lists and giveaways.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
HtmlWriter::Wrap::pushPoll/pushTodoList/pushGiveaway), GPL-3.0.
"""

from __future__ import annotations

from typing import Callable

from .html_countries import country_name_by_iso2, flag_emoji_by_iso2
from .html_text import PeersMap, WrapBase, format_text, serialize_string
from .model import peer_from_channel
from .model_format import format_date_time, number_to_string
from .model_media import GiveawayResults, GiveawayStart, Poll, TodoList

WrapMessageLink = Callable[[int, str], str]


def _votes(count: int) -> str:
    if count > 1:
        return number_to_string(count) + " votes"
    if count > 0:
        return number_to_string(count) + " vote"
    return "No votes"


def _participants(data: GiveawayStart, any_channel: bool, any_group: bool, count: int) -> str:
    single = count == 1
    many = count > 1
    if data.all:
        if not any_group and any_channel:
            if single:
                return "All subscribers of the channel:"
            if many:
                return "All subscribers of the channels:"
        if any_group and not any_channel:
            if single:
                return "All members of the group:"
            if many:
                return "All members of the groups:"
        if any_group and any_channel:
            if single:
                return "All members of the group:"
            if many:
                return "All members of the groups and channels:"
        return ""
    if not any_group and any_channel:
        if single:
            return "All users who joined the channel below after this date:"
        if many:
            return "All users who joined the channels below after this date:"
    if any_group and not any_channel:
        if single:
            return "All users who joined the group below after this date:"
        if many:
            return "All users who joined the groups below after this date:"
    if any_group and any_channel:
        if single:
            return "All users who joined the group below after this date:"
        if many:
            return "All users who joined the groups and channels below after this date:"
    return ""


class ExtraMediaMixin(WrapBase):
    def _push_section(self, class_name: str, content: str) -> str:
        return self.push_div(class_name) + content + self.pop_tag()

    def push_poll(self, data: Poll, internal_links_domain: str, relative_link_base: str) -> str:
        result = self.push_div("media_wrap clearfix") + self.push_div("media_poll")
        result += self._push_section(
            "question bold", format_text(data.question, internal_links_domain, relative_link_base)
        )
        result += self._push_section(
            "details", serialize_string("Final results" if data.closed else "Anonymous poll")
        )
        for answer in data.answers:
            details = ""
            if answer.votes:
                chosen = ", chosen vote" if answer.my else ""
                details = f' <span class="details">{_votes(answer.votes)}{chosen}</span>'
            text = format_text(answer.text, internal_links_domain, relative_link_base)
            result += self._push_section("answer", "- " + text + details)
        # tdesktop's class literal ends with a tab, which the attribute escaping keeps.
        result += self._push_section("total details\t", _votes(data.total_votes))
        return result + self.pop_tag() + self.pop_tag()

    def push_todo_list(
        self, data: TodoList, internal_links_domain: str, relative_link_base: str
    ) -> str:
        result = self.push_div("media_wrap clearfix") + self.push_div("media_poll")
        result += self._push_section(
            "question bold", format_text(data.title, internal_links_domain, relative_link_base)
        )
        result += self._push_section("details", serialize_string("To-do List"))
        for item in data.items:
            text = format_text(item.text, internal_links_domain, relative_link_base)
            result += self._push_section("answer", "- " + text)
        return result + self.pop_tag() + self.pop_tag()

    def push_giveaway_start(self, peers: PeersMap, data: GiveawayStart) -> str:
        result = self.push_div("media_wrap clearfix") + self.push_div("media_giveaway")
        title = "Giveaway Prizes" if data.quantity > 1 else "Giveaway Prize"
        result += self._push_section("section_title bold", serialize_string(title))
        quantity = number_to_string(data.quantity)
        result += self._push_section(
            "section_body",
            "<b>" + quantity + "</b> " + serialize_string(data.additional_prize),
        )
        result += self._push_section("section_title bold", serialize_string("with"))
        if data.credits > 0:
            stars = " Star" if data.credits == 1 else " Stars"
            body = (
                "<b>"
                + number_to_string(data.credits)
                + serialize_string(stars)
                + "</b> "
                + serialize_string("will be distributed ")
            )
            if data.quantity == 1:
                body += (
                    serialize_string("to ") + f"<b>{quantity}</b> " + serialize_string("winner.")
                )
            else:
                body += (
                    serialize_string("among ")
                    + f"<b>{quantity}</b> "
                    + serialize_string("winners.")
                )
        else:
            subscriptions = (
                "Telegram Premium Subscriptions"
                if data.quantity > 1
                else "Telegram Premium Subscription"
            )
            body = (
                f"<b>{quantity}</b> "
                + serialize_string(subscriptions)
                + " for <b>"
                + number_to_string(data.months)
                + "</b> "
                + ("months." if data.months > 1 else "month.")
            )
        result += self._push_section("section_body", body)
        result += self._push_section("section_title bold", serialize_string("Participants"))
        any_channel = any_group = False
        for channel in data.channels:
            chat = peers.peer(peer_from_channel(channel)).chat()
            if chat is not None:
                if chat.is_broadcast:
                    any_channel = True
                elif chat.is_supergroup:
                    any_group = True
        # tdesktop builds the "<b>name</b>" list here and then drops it (its `+` result is
        # discarded), so only the sentence is written.
        participants = _participants(data, any_channel, any_group, len(data.channels))
        result += self._push_section("section_body", serialize_string(participants))
        countries = [
            flag_emoji_by_iso2(country) + "\u00a0" + country_name_by_iso2(country)
            for country in data.countries
        ]
        if countries:
            united = countries[0]
            for index in range(1, len(countries)):
                separator = " and " if index + 1 == len(countries) else ", "
                united = united + separator + countries[index]
            result += self._push_section("section_body", serialize_string("from " + united))
        result += self._push_section(
            "section_title bold", serialize_string("Winners Selection Date")
        )
        result += self._push_section("section_body", format_date_time(data.until_date))
        return result + self.pop_tag() + self.pop_tag()

    def push_giveaway_results(
        self, peers: PeersMap, data: GiveawayResults, wrap_message_link: WrapMessageLink
    ) -> str:
        result = self.push_div("media_wrap clearfix") + self.push_div("media_giveaway")
        many = data.winners_count > 1
        title = "Winners Selected!" if many else "Winner Selected!"
        result += self._push_section("section_title bold", serialize_string(title))
        result += self._push_section(
            "section_body",
            "<b>"
            + number_to_string(data.winners_count)
            + "</b> "
            + serialize_string("winners" if many else "winner")
            + " of the "
            + wrap_message_link(data.launch_id, "Giveaway")
            + " was randomly selected by Telegram.",
        )
        result += self._push_section(
            "section_title bold", serialize_string("Winners" if many else "Winner")
        )
        winners = ", ".join(
            "<b>" + peers.wrap_peer_name(winner) + "</b>" for winner in data.winners
        )
        more = ""
        if data.winners_count > len(data.winners):
            more = (
                serialize_string(" and ")
                + number_to_string(data.winners_count - len(data.winners))
                + serialize_string(" more!")
            )
        result += self._push_section("section_body", winners + more)
        stars = " Star" if data.credits == 1 else " Stars"
        if data.credits and data.winners_count == 1:
            prize = (
                serialize_string("The winner received ")
                + "<b>"
                + number_to_string(data.credits)
                + "</b>"
                + serialize_string(stars + ".")
            )
        elif data.credits and data.winners_count > 1:
            prize = (
                serialize_string("All winners received ")
                + "<b>"
                + number_to_string(data.credits)
                + "</b>"
                + serialize_string(stars + " in total.")
            )
        elif data.unclaimed_count:
            prize = serialize_string("Some winners couldn't be selected.")
        elif data.winners_count == 1:
            prize = serialize_string("The winner received their gift link in a private message.")
        elif data.winners_count > 1:
            prize = serialize_string("All winners received gift links in private messages.")
        else:
            prize = ""
        result += self._push_section("section_body", prize)
        return result + self.pop_tag() + self.pop_tag()
