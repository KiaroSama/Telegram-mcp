"""Parsing of Telethon TL objects into the tdesktop export model (spec 030, stream A).

Every expectation below is what Telegram Desktop v7.2.10's export_data_types.cpp produces for
the same MTProto object. Synthetic data only.
"""

from datetime import datetime, timezone

import pytest
from telethon.tl import types as tl

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.model import (
    Document,
    ParseMediaContext,
    Photo,
    TextPart,
    parse_document,
    parse_text,
    peer_from_channel,
    peer_from_chat,
    peer_from_user,
)
from telegram_mcp.tdexport.model_actions import (
    ActionChatEditPhoto,
    ActionGiftCredits,
    ActionGiftPremium,
    ActionPhoneCall,
    ActionPollAppendAnswer,
    ActionPollDeleteAnswer,
    ActionStarGift,
    ActionTopicEdit,
    parse_service_action,
)
from telegram_mcp.tdexport.model_media import (
    GiveawayResults,
    PaidMedia,
    Poll,
    SharedContact,
    TodoList,
    parse_media,
)
from telegram_mcp.tdexport.model_message import (
    HistoryMessageMarkupButton,
    Reaction,
    adjust_migrate_message_ids,
    parse_message,
    parse_messages_slice,
)

UTC_DATE = int(datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc).timestamp())
STAMP = "@12-01-2024_10-00-05"
USER = tl.PeerUser(user_id=100)


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def photo(photo_id=5, sizes=None):
    return tl.Photo(
        id=photo_id,
        access_hash=7,
        file_reference=b"ref",
        date=UTC_DATE,
        sizes=sizes or [tl.PhotoSize(type="y", w=1280, h=720, size=3000)],
        dc_id=2,
    )


def document(doc_id=9, mime="video/mp4", attributes=(), size=1000, thumbs=None):
    return tl.Document(
        id=doc_id,
        access_hash=3,
        file_reference=b"r",
        date=UTC_DATE,
        mime_type=mime,
        size=size,
        dc_id=4,
        attributes=list(attributes),
        thumbs=thumbs,
    )


def message(**kwargs):
    kwargs.setdefault("id", 1)
    kwargs.setdefault("peer_id", USER)
    kwargs.setdefault("date", UTC_DATE)
    return tl.Message(**kwargs)


def test_parse_text_splits_entities_on_utf16_offsets():
    # The emoji is two UTF-16 units: the bold entity starts after it.
    text = "\U0001f600 bold end"
    entities = [
        tl.MessageEntityBold(offset=3, length=4),
        tl.MessageEntityBold(offset=2, length=1),  # overlaps the previous one: skipped
        tl.MessageEntityCustomEmoji(offset=0, length=2, document_id=77),
    ]
    parts = parse_text(text, entities)
    # The custom emoji comes after the bold one in the list, before the offset: skipped too.
    assert [(p.type, p.text) for p in parts] == [
        (TextPart.Type.Text, "\U0001f600 "),
        (TextPart.Type.Bold, "bold"),
        (TextPart.Type.Text, " end"),
    ]


def test_parse_text_additional_values():
    text = "code link @x quote"
    entities = [
        tl.MessageEntityPre(offset=0, length=4, language="py"),
        tl.MessageEntityTextUrl(offset=5, length=4, url="https://t.me"),
        tl.MessageEntityMentionName(offset=10, length=2, user_id=42),
        tl.MessageEntityBlockquote(offset=13, length=5, collapsed=True),
    ]
    parts = parse_text(text, entities)
    assert [(p.type, p.additional) for p in parts if p.type != TextPart.Type.Text] == [
        (TextPart.Type.Pre, "py"),
        (TextPart.Type.TextUrl, "https://t.me"),
        (TextPart.Type.MentionName, "42"),
        (TextPart.Type.Blockquote, "1"),
    ]


def test_parse_text_formatted_date_entity_is_unknown():
    parts = parse_text("today", [tl.MessageEntityFormattedDate(offset=0, length=5, date=1)])
    assert parts[0].type == TextPart.Type.Unknown


def test_message_basic_fields_and_forward():
    data = message(
        id=11,
        message="hi",
        out=True,
        from_id=tl.PeerUser(user_id=200),
        edit_date=datetime.fromtimestamp(UTC_DATE + 60, timezone.utc),
        post_author="Author",
        via_bot_id=300,
        fwd_from=tl.MessageFwdHeader(
            date=UTC_DATE - 5,
            from_id=tl.PeerChannel(channel_id=55),
            saved_from_peer=tl.PeerChat(chat_id=66),
        ),
        reply_to=tl.MessageReplyHeader(reply_to_msg_id=9, reply_to_peer_id=USER),
    )
    parsed = parse_message(ParseMediaContext(self_peer_id=peer_from_user(1)), data, "")
    assert (parsed.id, parsed.date, parsed.edited) == (11, UTC_DATE, UTC_DATE + 60)
    assert parsed.out and parsed.self_id == 1
    assert parsed.peer_id == 100 and parsed.from_id == 200
    assert parsed.forwarded_from_id == peer_from_channel(55)
    assert parsed.saved_from_chat_id == peer_from_chat(66)
    assert parsed.forwarded and parsed.show_forwarded_as_original
    assert parsed.forwarded_date == UTC_DATE - 5
    assert (parsed.signature, parsed.via_bot_id) == ("Author", 300)
    # An ordinary message keeps a reply peer equal to its own peer (tdesktop re-parses it).
    assert (parsed.reply_to_msg_id, parsed.reply_to_peer_id) == (9, 100)
    assert parsed.text == [TextPart(text="hi")]


def test_message_without_from_uses_peer():
    parsed = parse_message(ParseMediaContext(), message(peer_id=tl.PeerChannel(channel_id=8)), "")
    assert parsed.from_id == parsed.peer_id == peer_from_channel(8)


def test_service_message_drops_same_peer_reply_and_parses_action():
    data = tl.MessageService(
        id=3,
        peer_id=USER,
        date=UTC_DATE,
        reply_to=tl.MessageReplyHeader(reply_to_msg_id=2, reply_to_peer_id=USER),
        action=tl.MessageActionChatEditPhoto(photo=photo()),
    )
    context = ParseMediaContext()
    parsed = parse_message(context, data, "chats/chat_1/")
    assert parsed.reply_to_peer_id == 0
    assert isinstance(parsed.action.content, ActionChatEditPhoto)
    assert parsed.file().suggested_path == f"chats/chat_1/photos/photo_1{STAMP}.jpg"
    assert context.photos == 1


def test_photo_media_takes_largest_real_size():
    sizes = [
        tl.PhotoStrippedSize(type="i", bytes=b"xx"),
        tl.PhotoCachedSize(type="s", w=90, h=90, bytes=b"abc"),
        tl.PhotoSize(type="m", w=320, h=320, size=500),
        tl.PhotoSizeProgressive(type="y", w=800, h=800, sizes=[100, 900]),
    ]
    context = ParseMediaContext()
    media = parse_media(context, tl.MessageMediaPhoto(photo=photo(sizes=sizes)), "", UTC_DATE)
    assert isinstance(media.content, Photo)
    image = media.content.image
    assert (image.width, image.height, image.file.size) == (800, 800, 900)
    assert image.file.location.dc_id == 2
    assert image.file.location.data.thumb_size == "y"
    assert image.file.suggested_path == f"photos/photo_1{STAMP}.jpg"


def test_photo_cached_size_keeps_bytes():
    sizes = [tl.PhotoCachedSize(type="s", w=90, h=90, bytes=b"abc")]
    media = parse_media(
        ParseMediaContext(), tl.MessageMediaPhoto(photo=photo(sizes=sizes)), "", UTC_DATE
    )
    assert media.content.image.file.content == b"abc"
    assert media.content.image.file.size == 3


@pytest.mark.parametrize(
    ("attributes", "mime", "path"),
    [
        (
            [tl.DocumentAttributeVideo(duration=3.7, w=10, h=20)],
            "video/mp4",
            "video_files/video_1",
        ),
        (
            [tl.DocumentAttributeAudio(duration=5, voice=True)],
            "audio/ogg",
            "voice_messages/audio_1",
        ),
        ([tl.DocumentAttributeVideo(duration=5, w=1, h=1, round_message=True)], "video/mp4", ""),
        (
            [tl.DocumentAttributeSticker(alt="x", stickerset=tl.InputStickerSetEmpty())],
            "image/webp",
            "stickers/file_1",
        ),
        ([tl.DocumentAttributeAnimated()], "video/mp4", "animations/file_1"),
        ([], "application/x-nothing", "files/file_1"),
    ],
)
def test_document_folders_and_generated_names(attributes, mime, path):
    context = ParseMediaContext()
    parsed = parse_document(context, document(mime=mime, attributes=attributes), "", UTC_DATE)
    expected = {
        "video_files/video_1": f"video_files/video_1{STAMP}.mp4",
        "voice_messages/audio_1": f"voice_messages/audio_1{STAMP}.ogg",
        "": f"round_video_messages/file_1{STAMP}.mp4",
        "stickers/file_1": f"stickers/file_1{STAMP}.webp",
        "animations/file_1": f"animations/file_1{STAMP}.mp4",
        "files/file_1": f"files/file_1{STAMP}.unknown",
    }[path]
    assert parsed.file.suggested_path == expected


def test_document_file_name_attribute_is_sanitised_and_thumb_named():
    thumbs = [
        tl.PhotoStrippedSize(type="i", bytes=b"z"),
        tl.PhotoSize(type="m", w=320, h=180, size=77),
    ]
    attributes = [
        tl.DocumentAttributeFilename(file_name="a:b?.txt"),
        tl.DocumentAttributeAudio(duration=61, performer="P", title="T"),
    ]
    parsed = parse_document(
        ParseMediaContext(),
        document(mime="audio/mpeg", attributes=attributes, thumbs=thumbs),
        "chats/chat_1/",
        UTC_DATE,
    )
    assert parsed.file.suggested_path == "chats/chat_1/files/a_b_.txt"
    assert (parsed.song_performer, parsed.song_title, parsed.duration) == ("P", "T", 61)
    assert parsed.is_audio_file
    assert parsed.thumb.file.suggested_path == "chats/chat_1/files/a_b_.txt_thumb.jpg"
    assert (parsed.thumb.width, parsed.thumb.file.size) == (320, 77)
    assert parsed.thumb.file.location.data.thumb_size == "m"


def test_voice_mp3_and_counters_are_shared():
    context = ParseMediaContext()
    voice = [tl.DocumentAttributeAudio(duration=1, voice=True)]
    first = parse_document(context, document(mime="audio/mp3", attributes=voice), "", 0)
    second = parse_document(context, document(mime="audio/ogg", attributes=voice), "", 0)
    assert first.file.suggested_path == "voice_messages/audio_1.mp3"
    assert second.file.suggested_path == "voice_messages/audio_2.ogg"
    assert context.audios == 2


def test_ttl_media_loses_its_file_unless_outgoing():
    media = tl.MessageMediaDocument(
        document=document(attributes=[tl.DocumentAttributeVideo(duration=1, w=1, h=1)]),
        ttl_seconds=10,
    )
    incoming = parse_message(ParseMediaContext(), message(media=media), "")
    assert incoming.media.ttl == 10
    assert incoming.media.content.file.suggested_path == ""
    photo_media = tl.MessageMediaPhoto(photo=photo(), ttl_seconds=5)
    outgoing = parse_message(ParseMediaContext(), message(media=photo_media, out=True), "")
    # ParseMedia itself already clears a TTL photo's file, outgoing or not.
    assert outgoing.media.content.image.file.suggested_path == ""


def test_shared_contact_vcard_file():
    context = ParseMediaContext()
    media = tl.MessageMediaContact(
        phone_number="123", first_name="A", last_name="B", vcard="BEGIN:VCARD", user_id=5
    )
    parsed = parse_media(context, media, "", 0)
    assert isinstance(parsed.content, SharedContact)
    assert parsed.content.info.name() == "A B"
    assert parsed.content.vcard.content == b"BEGIN:VCARD"
    assert parsed.content.vcard.suggested_path == "contacts/contact_1.vcard"


def test_poll_answers_and_results():
    poll = tl.Poll(
        id=1,
        question=tl.TextWithEntities(text="Q?", entities=[]),
        answers=[
            tl.PollAnswer(text=tl.TextWithEntities(text="yes", entities=[]), option=b"0"),
            tl.PollAnswer(text=tl.TextWithEntities(text="no", entities=[]), option=b"1"),
        ],
        hash=0,
        closed=True,
    )
    results = tl.PollResults(
        total_voters=3,
        results=[
            tl.PollAnswerVoters(option=b"1", voters=2, chosen=True),
            tl.PollAnswerVoters(option=b"9", voters=1),
        ],
    )
    parsed = parse_media(
        ParseMediaContext(), tl.MessageMediaPoll(poll=poll, results=results), "", 0
    )
    content = parsed.content
    assert isinstance(content, Poll) and content.closed and content.total_votes == 3
    assert [(a.text[0].text, a.votes, a.my) for a in content.answers] == [
        ("yes", 0, False),
        ("no", 2, True),
    ]


def test_todo_list_and_giveaway_results_and_paid_media():
    todo = tl.MessageMediaToDo(
        todo=tl.TodoList(
            title=tl.TextWithEntities(text="T", entities=[]),
            list=[tl.TodoItem(id=4, title=tl.TextWithEntities(text="a", entities=[]))],
            others_can_append=True,
        )
    )
    parsed = parse_media(ParseMediaContext(), todo, "", 0).content
    assert isinstance(parsed, TodoList) and parsed.others_can_append
    assert [(i.id, i.text[0].text) for i in parsed.items] == [(4, "a")]

    results = tl.MessageMediaGiveawayResults(
        channel_id=7,
        launch_msg_id=8,
        winners_count=2,
        unclaimed_count=0,
        winners=[10, 11],
        until_date=UTC_DATE,
        only_new_subscribers=True,
        stars=50,
    )
    giveaway = parse_media(ParseMediaContext(), results, "", 0).content
    assert isinstance(giveaway, GiveawayResults)
    assert (giveaway.channel, giveaway.winners, giveaway.credits, giveaway.all) == (
        7,
        [10, 11],
        50,
        False,
    )

    paid = tl.MessageMediaPaidMedia(
        stars_amount=25,
        extended_media=[
            tl.MessageExtendedMediaPreview(),
            tl.MessageExtendedMedia(media=tl.MessageMediaPhoto(photo=photo())),
        ],
    )
    context = ParseMediaContext()
    content = parse_media(context, paid, "", UTC_DATE).content
    assert isinstance(content, PaidMedia) and content.stars == 25
    assert content.extended[0] is None
    assert isinstance(content.extended[1].content, Photo) and context.photos == 1


def test_web_page_and_dice_stay_null():
    assert (
        parse_media(ParseMediaContext(), tl.MessageMediaDice(value=1, emoticon="x"), "", 0).content
        is None
    )


def test_reactions_keep_order_and_merge_recent():
    reactions = tl.MessageReactions(
        results=[
            tl.ReactionCount(reaction=tl.ReactionEmoji(emoticon="\U0001f44d"), count=2),
            tl.ReactionCount(reaction=tl.ReactionCustomEmoji(document_id=99), count=1),
        ],
        recent_reactions=[
            tl.MessagePeerReaction(
                peer_id=tl.PeerUser(user_id=5),
                date=UTC_DATE,
                reaction=tl.ReactionEmoji(emoticon="\U0001f44d"),
            ),
            tl.MessagePeerReaction(peer_id=USER, date=UTC_DATE, reaction=tl.ReactionPaid()),
        ],
    )
    parsed = parse_message(ParseMediaContext(), message(reactions=reactions), "").reactions
    assert [Reaction.id(r) for r in parsed] == ["emoji\U0001f44d", "custom_emoji99", "paid"]
    assert parsed[0].count == 2 and parsed[0].recent == [Reaction.Recent(5, UTC_DATE)]
    assert parsed[2].count == 0 and len(parsed[2].recent) == 1


def test_inline_keyboard_rows():
    markup = tl.ReplyInlineMarkup(
        rows=[
            tl.KeyboardInlineButtonRow(
                buttons=[
                    tl.KeyboardInlineButton(
                        text="u", type=tl.InlineButtonTypeUrl(url="https://a")
                    ),
                    tl.KeyboardInlineButton(
                        text="c",
                        type=tl.InlineButtonTypeCallback(data=b"\x00\x01", requires_password=True),
                    ),
                ]
            ),
            tl.KeyboardInlineButtonRow(
                buttons=[
                    tl.KeyboardInlineButton(
                        text="skip",
                        type=tl.InputInlineButtonTypeUserProfile(user_id=tl.InputUserSelf()),
                    )
                ]
            ),
            tl.KeyboardInlineButtonRow(
                buttons=[
                    tl.KeyboardInlineButton(
                        text="p", type=tl.InlineButtonTypeUserProfile(user_id=7)
                    ),
                    tl.KeyboardInlineButton(text="d", type=tl.InlineButtonTypeDisabled()),
                ]
            ),
        ]
    )
    rows = parse_message(ParseMediaContext(), message(reply_markup=markup), "").inline_button_rows
    kinds = HistoryMessageMarkupButton.Type
    assert [[(b.type, b.data) for b in row] for row in rows] == [
        [(kinds.Url, b"https://a"), (kinds.CallbackWithPassword, b"\x00\x01")],
        [(kinds.UserProfile, b"7"), (kinds.Disabled, b"")],
    ]
    assert HistoryMessageMarkupButton.type_to_string(rows[0][1]) == "callback_with_password"


def test_messages_slice_is_oldest_first_and_migrated_ids_shift():
    data = [message(id=5, reply_to=tl.MessageReplyHeader(reply_to_msg_id=4)), message(id=4)]
    users = [tl.User(id=100, first_name="A")]
    slice_ = parse_messages_slice(ParseMediaContext(), data, users, [], "")
    assert [m.id for m in slice_.list] == [4, 5]
    assert slice_.peers[100].name() == "A"
    shifted = adjust_migrate_message_ids(slice_)
    assert [m.id for m in shifted.list] == [4 - 1_000_000_000, 5 - 1_000_000_000]
    assert shifted.list[1].reply_to_msg_id == 4 - 1_000_000_000


def action(value, folder="", context=None):
    return parse_service_action(context or ParseMediaContext(), value, folder, UTC_DATE).content


def test_service_actions_money_and_calls():
    premium = action(tl.MessageActionGiftPremium(currency="USD", amount=199, days=30))
    assert premium == ActionGiftPremium(cost="$1.99", days=30)
    stars = action(tl.MessageActionGiftStars(currency="EUR", amount=500, stars=100))
    assert isinstance(stars, ActionGiftCredits) and stars.cost == "5,00 \u20ac"
    assert stars.amount.whole() == 100 and stars.amount.stars()
    ton = action(
        tl.MessageActionGiftTon(
            currency="USD", amount=2_500_000_000, crypto_currency="TON", crypto_amount=1
        )
    )
    assert (ton.amount.whole(), ton.amount.nano(), ton.amount.ton()) == (2, 500_000_000, True)
    call = action(
        tl.MessageActionPhoneCall(call_id=1, reason=tl.PhoneCallDiscardReasonMissed(), duration=4)
    )
    assert (call.state, call.duration) == (ActionPhoneCall.State.Missed, 4)
    for kwargs, state in (
        ({"missed": True}, ActionPhoneCall.State.Missed),
        ({"active": True}, ActionPhoneCall.State.Active),
        ({"duration": 9}, ActionPhoneCall.State.Hangup),
        ({}, ActionPhoneCall.State.Invitation),
    ):
        conference = action(tl.MessageActionConferenceCall(call_id=77, **kwargs))
        assert (conference.state, conference.conference_id) == (state, 77)


def test_service_actions_quirks_ported_as_is():
    answer = tl.PollAnswer(text=tl.TextWithEntities(text="opt", entities=[]), option=b"1")
    # tdesktop's first PollAppendAnswer handler wins and leaves the option empty.
    assert action(tl.MessageActionPollAppendAnswer(answer=answer)) == ActionPollAppendAnswer()
    assert action(tl.MessageActionPollDeleteAnswer(answer=answer)) == ActionPollDeleteAnswer("opt")
    # std::optional<uint64> iconEmojiId = 0 stays engaged with 0.
    assert action(tl.MessageActionTopicEdit(title="t")) == ActionTopicEdit("t", 0)
    assert action(tl.MessageActionTopicEdit(icon_emoji_id=5)).icon_emoji_id == 5
    assert (
        action(
            tl.MessageActionPaymentSentMe(currency="X", total_amount=1, payload=b"", charge=None)
        )
        is None
    )


def test_service_actions_gifts_and_misc():
    gift = tl.StarGift(id=4, sticker=document(), stars=50, convert_stars=10, limited=True)
    parsed = action(
        tl.MessageActionStarGift(
            gift=gift,
            name_hidden=True,
            message=tl.TextWithEntities(text="hey", entities=[]),
        )
    )
    assert isinstance(parsed, ActionStarGift)
    assert (parsed.gift_id, parsed.stars, parsed.limited, parsed.anonymous) == (4, 50, True, True)
    assert parsed.text == [TextPart(text="hey")]
    offer = action(
        tl.MessageActionStarGiftPurchaseOffer(
            gift=gift, price=tl.StarsTonAmount(amount=-1_500_000_000), expires_at=UTC_DATE
        )
    )
    assert offer.offer and offer.offer_expire_at == UTC_DATE
    assert (offer.offer_price.whole(), offer.offer_price.nano()) == (-2, 500_000_000)
    refunded = action(
        tl.MessageActionPaymentRefunded(
            peer=USER,
            currency="XTR",
            total_amount=5,
            charge=tl.PaymentCharge(id="tx", provider_charge_id="p"),
        )
    )
    assert (refunded.peer_id, refunded.transaction_id) == (100, "tx")
    birthday = action(
        tl.MessageActionSuggestBirthday(birthday=tl.Birthday(day=29, month=2, year=2023))
    )
    assert not birthday.birthday.valid()
    proximity = action(
        tl.MessageActionGeoProximityReached(
            from_id=USER, to_id=tl.PeerUser(user_id=1), distance=50
        ),
        context=ParseMediaContext(self_peer_id=100),
    )
    assert proximity.from_self and not proximity.to_self
    secure = action(tl.MessageActionSecureValuesSent(types=[tl.SecureValueTypeEmail()]))
    assert secure.types[0].name == "Email"
    approval = action(
        tl.MessageActionSuggestedPostApproval(
            price=tl.StarsAmount(amount=3, nanos=5), rejected=True
        )
    )
    assert approval.rejected and approval.price.value() == pytest.approx(3.000000005)
    assert action(tl.MessageActionNoForwardsToggle(prev_value=False, new_value=True)).new_value


def test_suggest_profile_photo_counts_photos():
    context = ParseMediaContext(photos=2)
    parsed = action(tl.MessageActionSuggestProfilePhoto(photo=photo()), "f/", context)
    assert parsed.photo.image.file.suggested_path == f"f/photos/photo_3{STAMP}.jpg"


def test_empty_document_media_is_default_document():
    media = parse_media(ParseMediaContext(), tl.MessageMediaDocument(spoiler=True), "", 0)
    assert isinstance(media.content, Document) and media.content.spoilered
    assert media.content.file.skip_reason == SkipReason.None_
