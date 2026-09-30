"""The JSON export writer: message media and extras as SerializeMessage writes them.

Every expectation is hand-derived from the media v::match, the inline-button lambda and the
header pushes of Telegram Desktop v7.2.10's export_output_json.cpp SerializeMessage: key order,
which values are bare or quoted, and the nesting-driven indentation. Synthetic data only.
"""

from datetime import datetime, timezone

import pytest

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.json_message import serialize_message
from telegram_mcp.tdexport.json_serialize import K_ARRAY, K_OBJECT, JsonContext
from telegram_mcp.tdexport.model import (
    Document,
    File,
    Image,
    peer_from_channel,
    peer_from_user,
)
from telegram_mcp.tdexport.model_media import (
    GeoPoint,
    GiveawayResults,
    Invoice,
    Media,
    PaidMedia,
    SharedContact,
    Venue,
)
from telegram_mcp.tdexport.model_message import HistoryMessageMarkupButton, Message, Reaction
from telegram_mcp.tdexport.model_peers import Chat, ContactInfo, Peer, User

D = int(datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc).timestamp())
ISO = '"2024-01-12T10:00:05"'
ANN = peer_from_user(100)
NEWS = peer_from_channel(9)
HEAD = (
    "{",
    '   "id": 5,',
    '   "type": "message",',
    f'   "date": {ISO},',
    '   "date_unixtime": "1705053605",',
)
TAIL = ('   "text": "",', '   "text_entities": []', "  }")


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def lines(*parts: str) -> bytes:
    return "\n".join(parts).encode("utf-8")


def peers() -> dict:
    return {
        ANN: Peer(User(bare_id=100, info=ContactInfo(user_id=100, first_name="Ann"))),
        NEWS: Peer(Chat(bare_id=9, title="News", is_broadcast=True)),
    }


def message_json(message: Message) -> bytes:
    return serialize_message(JsonContext([K_OBJECT, K_ARRAY]), message, peers(), "")


def media_json(content, ttl=0) -> bytes:
    return message_json(Message(id=5, date=D, media=Media(content, ttl)))


def test_animation_with_thumbnail_dimensions_spoiler_and_ttl():
    gif = Document(
        name="a.mp4",
        mime="video/mp4",
        is_animated=True,
        width=320,
        height=240,
        duration=4,
        spoilered=True,
        file=File(relative_path="video_files/a.mp4", size=100),
        thumb=Image(90, 90, File(relative_path="video_files/a.mp4_thumb.jpg", size=10)),
    )
    assert media_json(gif, ttl=7) == lines(
        *HEAD,
        '   "file": "video_files/a.mp4",',
        '   "file_name": "a.mp4",',
        '   "file_size": 100,',
        '   "thumbnail": "video_files/a.mp4_thumb.jpg",',
        '   "thumbnail_file_size": 10,',
        '   "media_type": "animation",',
        '   "mime_type": "video/mp4",',
        '   "duration_seconds": 4,',
        '   "width": 320,',
        '   "height": 240,',
        '   "media_spoiler": true,',
        '   "self_destruct_period_seconds": 7,',
        *TAIL,
    )


@pytest.mark.parametrize(
    ("flags", "typed"),
    [
        ({"is_video_message": True}, ['   "media_type": "video_message",']),
        ({"is_video_file": True}, ['   "media_type": "video_file",']),
        (
            {"is_audio_file": True, "song_performer": "P", "song_title": "T"},
            ['   "media_type": "audio_file",', '   "performer": "P",', '   "title": "T",'],
        ),
        ({"is_audio_file": True}, ['   "media_type": "audio_file",']),
        ({}, []),
    ],
)
def test_document_media_types(flags, typed):
    document = Document(file=File(relative_path="files/f", size=1), **flags)
    assert media_json(document) == lines(
        *HEAD, '   "file": "files/f",', '   "file_size": 1,', *typed, *TAIL
    )


def test_contact_with_vcard():
    contact = SharedContact(
        info=ContactInfo(first_name="A", last_name="B", phone_number="0123"),
        vcard=File(content=b"vcard", relative_path="contacts/contact_1.vcard", size=5),
    )
    assert media_json(contact) == lines(
        *HEAD,
        '   "contact_information": {',
        '    "first_name": "A",',
        '    "last_name": "B",',
        '    "phone_number": "0123"',
        "   },",
        '   "contact_vcard": "contacts/contact_1.vcard",',
        '   "contact_vcard_file_size": 5,',
        *TAIL,
    )
    # Without vCard content the file is never mentioned.
    assert b"contact_vcard" not in media_json(SharedContact(vcard=File(size=5)))


def test_invalid_live_location_and_venues():
    assert media_json(GeoPoint(), ttl=60) == lines(
        *HEAD,
        '   "location_information": null,',
        '   "live_location_period_seconds": 60,',
        *TAIL,
    )
    venue = Venue(point=GeoPoint(1.5, 2.5, True), title="V", address="A")
    assert media_json(venue) == lines(
        *HEAD,
        '   "place_name": "V",',
        '   "address": "A",',
        '   "location_information": {',
        '    "latitude": 1.500000,',
        '    "longitude": 2.500000',
        "   },",
        *TAIL,
    )
    assert media_json(Venue()) == lines(*HEAD, *TAIL)


def test_invoice_receipt_and_paid_media():
    invoice = Invoice(title="T", description="D", currency="USD", amount=5, receipt_msg_id=9)
    assert b'\\n    \\"receipt_message_id\\": 9\\n   }",' in media_json(invoice)
    assert media_json(PaidMedia(stars=5)) == lines(*HEAD, '   "paid_stars_amount": 5,', *TAIL)


def test_giveaway_results():
    results = GiveawayResults(
        channel=9,
        winners=[ANN],
        additional_prize="P",
        until_date=D,
        launch_id=3,
        additional_peers_count=1,
        winners_count=2,
        months=3,
        refunded=True,
    )
    assert media_json(results) == lines(
        *HEAD,
        '   "giveaway_results": {',
        '    "channel": 9,',
        '    "winners": [',
        "     100",
        "    ],",
        '    "additional_prize": "P",',
        f'    "until_date": {ISO},',
        '    "launch_message_id": 3,',
        '    "additional_peers_count": 1,',
        '    "winners_count": 2,',
        '    "unclaimed_count": 0,',
        '    "months": 3,',
        '    "stars": 0,',
        '    "is_refunded": true,',
        '    "is_only_new_subscribers": true',
        "   },",
        *TAIL,
    )


def test_button_forward_text_id_and_empty_text():
    kind = HistoryMessageMarkupButton.Type
    auth = HistoryMessageMarkupButton(kind.Auth, "In", b"https://a", "fwd", 3)
    row = [auth, HistoryMessageMarkupButton(kind.RequestPhone)]
    body = message_json(Message(id=5, date=D, inline_button_rows=[row]))
    assert body.endswith(
        lines(
            '   "inline_bot_buttons": [',
            "    [",
            "     {",
            '      "type": "auth",',
            '      "text": "In",',
            '      "data": "https://a",',
            '      "forward_text": "fwd",',
            '      "button_id": 3',
            "     },",
            "     {",
            '      "type": "request_phone"',
            "     }",
            "    ]",
            "   ]",
            "  }",
        )
    )


def test_custom_emoji_and_paid_reactions():
    reactions = [
        Reaction(Reaction.Type.CustomEmoji, document_id="stickers/e.webp", count=2),
        Reaction(Reaction.Type.Paid, count=1),
    ]
    body = message_json(Message(id=5, date=D, reactions=reactions))
    assert body.endswith(
        lines(
            '   "reactions": [',
            "     {",
            '      "type": "custom_emoji",',
            '      "count": 2,',
            '      "document_id": "stickers/e.webp"',
            "     },",
            "     {",
            '      "type": "paid",',
            '      "count": 1',
            "     }",
            "    ]",
            "  }",
        )
    )


def test_saved_from_and_forwarded_peer():
    message = Message(
        id=5, date=D, forwarded_from_id=NEWS, saved_from_chat_id=ANN, signature="Sig"
    )
    assert message_json(message) == lines(
        *HEAD,
        '   "author": "Sig",',
        '   "forwarded_from": "News",',
        '   "forwarded_from_id": "channel9",',
        '   "saved_from": "Ann",',
        *TAIL,
    )
