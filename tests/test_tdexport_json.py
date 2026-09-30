"""The JSON export writer (spec 030, stream B): result.json byte for byte as Telegram Desktop.

Every expectation is hand-derived from Telegram Desktop v7.2.10's export_output_json.cpp for the
same Data:: objects: its own string escaping, its nesting-driven indentation (quirks included)
and its key order. Synthetic data only.
"""

from datetime import datetime, timezone

import pytest
from telethon.tl import types as tl

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import SkipReason
from telegram_mcp.tdexport.html_and_json import HtmlAndJsonWriter
from telegram_mcp.tdexport.json_message import serialize_message
from telegram_mcp.tdexport.json_rich import serialize_rich_message
from telegram_mcp.tdexport.json_serialize import (
    K_ARRAY,
    K_OBJECT,
    JsonContext,
    serialize_string,
    serialize_text,
)
from telegram_mcp.tdexport.json_writer import JsonWriter
from telegram_mcp.tdexport.model import (
    CreditsAmount,
    Document,
    File,
    Image,
    Photo,
    TextPart,
    peer_from_channel,
    peer_from_chat,
    peer_from_user,
)
from telegram_mcp.tdexport.model_actions import (
    ActionChatCreate,
    ActionPhoneCall,
    ActionRequestedPeer,
    ActionSuggestedPostApproval,
    ActionTodoAppendTasks,
    ActionTopicEdit,
    ServiceAction,
)
from telegram_mcp.tdexport.model_dialogs import DialogInfo, DialogsInfo
from telegram_mcp.tdexport.model_media import (
    Game,
    GeoPoint,
    GiveawayStart,
    Invoice,
    Media,
    Poll,
    TodoList,
    TodoListItem,
    UnsupportedMedia,
)
from telegram_mcp.tdexport.model_message import (
    HistoryMessageMarkupButton,
    Message,
    MessagesSlice,
    Reaction,
)
from telegram_mcp.tdexport.model_peers import Chat, ContactInfo, Peer, User
from telegram_mcp.tdexport.model_rich import (
    InlineButtonAction,
    RichBlock,
    RichButtonPayload,
    RichButtonStyle,
    RichMessage,
    RichText,
)
from telegram_mcp.tdexport.output_file import File as OutputFile
from telegram_mcp.tdexport.settings import Environment, Settings

D = int(datetime(2024, 1, 12, 10, 0, 5, tzinfo=timezone.utc).timestamp())
ISO = '"2024-01-12T10:00:05"'
RAW = '"1705053605"'
ANN = peer_from_user(100)
T = TextPart.Type


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def lines(*parts: str) -> bytes:
    return "\n".join(parts).encode("utf-8")


def peers() -> dict:
    bot = User(bare_id=300, info=ContactInfo(user_id=300, first_name="Bot"))
    bot.username, bot.is_bot = "helper", True
    return {
        ANN: Peer(User(bare_id=100, info=ContactInfo(user_id=100, first_name="Ann"))),
        peer_from_user(300): Peer(bot),
        peer_from_channel(9): Peer(Chat(bare_id=9, title="News", is_broadcast=True)),
    }


def message_json(message: Message, domain: str = "") -> bytes:
    """A message as the single-chat writer serializes it: inside the dialog's "messages"."""
    return serialize_message(JsonContext([K_OBJECT, K_ARRAY]), message, peers(), domain)


HEAD = ("{", '   "id": 5,', '   "type": "message",', f'   "date": {ISO},')


def head(kind: str = "message", id_: int = 5) -> tuple[str, ...]:
    return ("{", f'   "id": {id_},', f'   "type": "{kind}",', f'   "date": {ISO},')


# SerializeString


def test_string_escapes_like_tdesktop_not_like_json():
    assert serialize_string('a"b\\c\n\r\t') == b'"a\\"b\\\\c\\n\\r\\t"'
    # Control bytes become \xNN (upper-case hex); DEL and non-ASCII pass through as UTF-8.
    assert serialize_string("\x01\x1f\x7f\u00e9") == b'"\\x01\\x1F\x7f\xc3\xa9"'
    assert serialize_string(b"\xff\x00") == b'"\xff\\x00"'


def test_line_separators_keep_their_continuation_bytes():
    # SerializeString escapes the 0xE2 lead byte and then copies 0x80 0xA8 unchanged.
    assert serialize_string("x\u2028y\u2029") == b'"x\\u2028\x80\xa8y\\u2029\x80\xa9"'


# SerializeText


def test_text_empty_single_plain_and_objects():
    context = JsonContext()
    assert serialize_text(context, []) == b'""'
    assert serialize_text(context, [], True) == b"[]"
    assert serialize_text(context, [TextPart(text="a")]) == b'"a"'
    assert serialize_text(context, [TextPart(text="a")], True) == lines(
        "[", " {", '  "type": "plain",', '  "text": "a"', " }", "]"
    )


def test_text_parts_and_additional_values():
    parts = [
        TextPart(text="Hi "),
        TextPart(T.Bold, "you"),
        TextPart(T.MentionName, "Bob", "42"),
        TextPart(T.Blockquote, "q", ""),
        TextPart(T.Pre, "x", ""),
        TextPart(T.TextUrl, "l", "https://e.x"),
        TextPart(T.CustomEmoji, "e", "(unavailable)"),
    ]
    assert serialize_text(JsonContext(), parts) == lines(
        "[",
        ' "Hi ",',
        " {",
        '  "type": "bold",',
        '  "text": "you"',
        " },",
        " {",
        '  "type": "mention_name",',
        '  "text": "Bob",',
        '  "user_id": 42',
        " },",
        " {",
        '  "type": "blockquote",',
        '  "text": "q",',
        '  "collapsed": false',
        " },",
        " {",
        '  "type": "pre",',
        '  "text": "x",',
        '  "language": ""',
        " },",
        " {",
        '  "type": "text_link",',
        '  "text": "l",',
        '  "href": "https://e.x"',
        " },",
        " {",
        '  "type": "custom_emoji",',
        '  "text": "e",',
        '  "document_id": "(unavailable)"',
        " }",
        "]",
    )


# JsonWriter: the whole result.json


def single_chat_settings(tmp_path, **kwargs) -> Settings:
    return Settings(
        path=str(tmp_path).replace("\\", "/") + "/",
        single_peer=tl.InputPeerUser(user_id=100, access_hash=0),
        **kwargs,
    )


def write_export(writer, settings: Settings, info: DialogInfo, slices: list) -> None:
    writer.start(settings, Environment())
    writer.write_dialogs_start(DialogsInfo(chats=[info]))
    writer.write_dialog_start(info)
    for slice_ in slices:
        writer.write_dialog_slice(slice_)
    writer.write_dialog_end()
    writer.write_dialogs_end()
    writer.finish()


def test_single_chat_result_json_and_date_skip(tmp_path):
    settings = single_chat_settings(tmp_path, single_peer_from=D)
    info = DialogInfo(type=DialogInfo.Type.Personal, name="Ann", peer_id=ANN)
    early = Message(id=4, date=D - 1, from_id=ANN, text=[TextPart(text="old")])
    kept = Message(id=5, date=D, from_id=ANN, text=[TextPart(text="hi")])
    writer = JsonWriter()
    write_export(writer, settings, info, [MessagesSlice([early, kept], peers()), MessagesSlice()])
    assert writer.main_file_path() == settings.path + "result.json"
    assert (tmp_path / "result.json").read_bytes() == lines(
        "{",
        ' "name": "Ann",',
        ' "type": "personal_chat",',
        ' "id": 100,',
        ' "messages": [',
        "  {",
        '   "id": 5,',
        '   "type": "message",',
        f'   "date": {ISO},',
        f'   "date_unixtime": {RAW},',
        '   "from": "Ann",',
        '   "from_id": "user100",',
        '   "text": "hi",',
        '   "text_entities": [',
        "    {",
        '     "type": "plain",',
        '     "text": "hi"',
        "    }",
        "   ]",
        "  }",
        " ]",
        "}",
    )


def test_saved_messages_has_no_name_and_empty_list(tmp_path):
    info = DialogInfo(type=DialogInfo.Type.Self, name="Me", peer_id=ANN)
    write_export(JsonWriter(), single_chat_settings(tmp_path), info, [])
    assert (tmp_path / "result.json").read_bytes() == lines(
        "{", ' "type": "saved_messages",', ' "id": 100,', ' "messages": [', " ]", "}"
    )


def test_account_wide_framing_when_not_single_peer(tmp_path):
    settings = Settings(path=str(tmp_path).replace("\\", "/") + "/")
    environment = Environment(about_telegram="About", about_chats="Chats")
    info = DialogInfo(type=DialogInfo.Type.Personal, name="Ann", peer_id=ANN)
    writer = JsonWriter()
    writer.start(settings, environment)
    writer.write_dialogs_start(DialogsInfo(chats=[info]))
    writer.write_dialog_start(info)
    writer.write_dialog_slice(MessagesSlice())
    writer.write_dialog_end()
    writer.write_dialogs_end()
    writer.finish()
    assert (tmp_path / "result.json").read_bytes() == lines(
        "{",
        ' "about": "About",',
        ' "chats": {',
        '  "about": "Chats",',
        '  "list": [',
        "   {",
        '    "name": "Ann",',
        '    "type": "personal_chat",',
        '    "id": 100,',
        '    "messages": [',
        "    ]",
        "   }",
        "  ]",
        " }",
        "}",
    )


# SerializeMessage: header fields


def test_forward_reply_edit_author_and_via_bot():
    message = Message(
        id=5,
        date=D,
        edited=D,
        from_id=ANN,
        signature="Sig",
        forwarded_from_id=peer_from_channel(9),
        reply_to_msg_id=2,
        reply_to_peer_id=peer_from_chat(4),
        via_bot_id=300,
    )
    assert message_json(message) == lines(
        *HEAD,
        f'   "date_unixtime": {RAW},',
        f'   "edited": {ISO},',
        f'   "edited_unixtime": {RAW},',
        '   "from": "Ann",',
        '   "from_id": "user100",',
        '   "author": "Sig",',
        '   "forwarded_from": "News",',
        '   "forwarded_from_id": "channel9",',
        '   "reply_to_message_id": 2,',
        '   "reply_to_peer_id": "chat4",',
        '   "via_bot": "@helper",',
        '   "text": "",',
        '   "text_entities": []',
        "  }",
    )


def test_forwarded_name_and_unknown_sender_is_null():
    message = Message(id=5, date=D, from_id=peer_from_user(7), forwarded_from_name="Anon")
    body = message_json(message)
    assert b'\n   "from": null,\n   "from_id": "user7",' in body
    assert b'\n   "forwarded_from": "Anon",' in body


def test_unsupported_media_without_rich_message():
    message = Message(id=9, date=D, media=Media(content=UnsupportedMedia()))
    assert message_json(message) == lines("{", '   "id": 9,', '   "type": "unsupported"', "  }")


# Service actions


def service(action, **kwargs) -> Message:
    return Message(id=7, date=D, action=ServiceAction(action), **kwargs)


def test_create_group_members_array():
    message = service(ActionChatCreate("Club", [100, 200]), from_id=ANN)
    assert message_json(message) == lines(
        *head("service", 7),
        f'   "date_unixtime": {RAW},',
        '   "actor": "Ann",',
        '   "actor_id": "user100",',
        '   "action": "create_group",',
        '   "title": "Club",',
        '   "members": [',
        '    "Ann",',
        "    null",
        "   ],",
        '   "text": "",',
        '   "text_entities": []',
        "  }",
    )


def test_requested_peers_are_written_as_a_string():
    # push("peers", SerializeArray(...)) serializes the array text again as a string.
    action = ActionRequestedPeer([peer_from_user(5), peer_from_channel(9)], 3)
    body = message_json(service(action))
    assert b'\n   "action": "requested_peer",\n   "button_id": 3,' in body
    assert b'\n   "peers": "[\\n    5,\\n    562949953421321\\n   ]",' in body


def test_phone_and_conference_calls():
    missed = service(ActionPhoneCall(state=ActionPhoneCall.State.Missed), from_id=ANN)
    assert b'"action": "phone_call",\n   "discard_reason": "missed",' in message_json(missed)
    active = ActionPhoneCall(conference_id=1, state=ActionPhoneCall.State.Active, duration=30)
    assert (
        b'"action": "conference_call",\n   "duration_seconds": 30,\n'
        b'   "is_active": true,\n   "is_missed": false,'
    ) in message_json(service(active))


def test_suggested_post_price_is_quoted_and_topic_icon_zero_is_written():
    approval = ActionSuggestedPostApproval(schedule_date=D, price=CreditsAmount.make(5, 0))
    assert (
        b'"price_amount_whole": "5",\n   "price_amount_nano": "0",\n'
        b'   "price_currency": "Stars",\n   "scheduled_date": 1705053605,'
    ) in message_json(service(approval))
    assert b'"new_icon_emoji_id": 0,' in message_json(service(ActionTopicEdit()))
    assert b"new_icon_emoji_id" not in message_json(service(ActionTopicEdit("t", None)))


def test_todo_append_tasks_keeps_tdesktop_indentation():
    item = TodoListItem(text=[TextPart(T.Bold, "b")], id=2)
    body = message_json(service(ActionTodoAppendTasks([item])))
    # The item text is serialized before its object is opened, one level shallower.
    assert (
        lines(
            '   "items": [',
            "    {",
            '     "text": [',
            "     {",
            '      "type": "bold",',
            '      "text": "b"',
            "     }",
            "    ],",
            '     "id": 2',
            "    }",
            "   ],",
        )
        in body
    )


# Media


def media(content, ttl: int = 0) -> Message:
    return Message(id=5, date=D, media=Media(content=content, ttl=ttl))


@pytest.mark.parametrize(
    ("reason", "text"),
    [
        (SkipReason.FileSize, "(File exceeds maximum size. Change data exporting settings to "),
        (SkipReason.FileType, "(File not included. Change data exporting settings to "),
        (SkipReason.Unavailable, "(File unavailable, please try again later)"),
    ],
)
def test_document_placeholders(reason, text):
    document = Document(file=File(size=10, skip_reason=reason), name="a.bin", mime="app/x")
    body = message_json(media(document))
    assert f'\n   "file": "{text}'.encode() in body
    assert b'"file_name": "a.bin",\n   "file_size": 10,\n   "mime_type": "app/x",' in body


def test_voice_message_and_sticker_documents():
    voice = Document(file=File(relative_path="voice_messages/a.ogg", size=3), duration=4)
    voice.is_voice_message, voice.mime = True, "audio/ogg"
    assert (
        b'"file": "voice_messages/a.ogg",\n   "file_size": 3,\n'
        b'   "media_type": "voice_message",\n   "mime_type": "audio/ogg",\n'
        b'   "duration_seconds": 4,'
    ) in message_json(media(voice))
    sticker = Document(file=File(relative_path="s.webp"), width=512, height=512)
    sticker.is_sticker, sticker.sticker_emoji = True, "x"
    assert (
        b'"media_type": "sticker",\n   "sticker_emoji": "x",\n'
        b'   "width": 512,\n   "height": 512,'
    ) in message_json(media(sticker))


def test_photo_path_and_self_destructing_photo():
    photo = Photo(image=Image(2, 1, File(size=3, relative_path="photos/p.jpg")))
    photo.spoilered = True
    assert (
        b'"photo": "photos/p.jpg",\n   "photo_file_size": 3,\n   "width": 2,\n'
        b'   "height": 1,\n   "media_spoiler": true,'
    ) in message_json(media(photo))
    body = message_json(media(Photo(), ttl=10))
    assert b'"photo_file_size": 0,\n   "self_destruct_period_seconds": 10,' in body
    assert b'"photo":' not in body


def test_locations():
    point = GeoPoint(latitude=55.7558, longitude=37.6173, valid=True)
    assert (
        b'"location_information": {\n    "latitude": 55.755800,\n'
        b'    "longitude": 37.617300\n   },'
    ) in message_json(media(point))
    live = message_json(media(GeoPoint(), ttl=60))
    assert b'"location_information": null,\n   "live_location_period_seconds": 60,' in live


def test_invoice_is_written_as_a_string_and_game_link():
    invoice = Invoice(title="T", description="D", currency="USD", amount=150)
    assert (
        b'"invoice_information": "{\\n    \\"title\\": \\"T\\",\\n    \\"description\\": '
        b'\\"D\\",\\n    \\"amount\\": 150,\\n    \\"currency\\": \\"USD\\"\\n   }",'
    ) in message_json(media(invoice))
    game = Game(title="G", short_name="g1", bot_id=300)
    body = message_json(media(game), "https://t.me/")
    assert b'"game_title": "G",\n   "game_link": "https://t.me/helper?game=g1",' in body


def test_poll_and_todo_list():
    answer = Poll.Answer(text=[TextPart(text="Yes")], votes=2, my=True)
    poll = Poll(question=[TextPart(text="Q?")], answers=[answer], total_votes=2)
    assert lines(
        '   "poll": {',
        '    "question": "Q?",',
        '    "closed": false,',
        '    "total_voters": 2,',
        '    "answers": [',
        "     {",
        '      "text": "Yes",',
        '      "voters": 2,',
        '      "chosen": true',
        "     }",
        "    ]",
        "   },",
    ) in message_json(media(poll))
    todo = TodoList(True, False, [TextPart(text="Do")], [TodoListItem([TextPart(text="a")], 1)])
    assert lines(
        '   "todo_list": {',
        '    "title": "Do",',
        '    "others_can_append": true,',
        '    "others_can_complete": false,',
        '    "answers": [',
        "     {",
        '      "text": "a",',
        '      "id": 1',
        "     }",
        "    ]",
        "   },",
    ) in message_json(media(todo))


def test_giveaway_start():
    start = GiveawayStart(["DE"], [9], "", D, 0, 3, 6, True)
    assert lines(
        '   "giveaway_information": {',
        '    "quantity": 3,',
        '    "months": 6,',
        f'    "until_date": {ISO},',
        '    "channels": [',
        "     9",
        "    ],",
        '    "countries": [',
        '     "DE"',
        "    ],",
        '    "additional_prize": "",',
        '    "stars": 0,',
        '    "is_only_new_subscribers": false',
        "   },",
    ) in message_json(media(start))


# Inline buttons and reactions


def test_inline_buttons():
    kind = HistoryMessageMarkupButton.Type
    row = [
        HistoryMessageMarkupButton(kind.Callback, "Go", b"\xfb\xff\x01"),
        HistoryMessageMarkupButton(kind.Url, "Site", b"https://e.x"),
    ]
    message = Message(id=5, date=D, inline_button_rows=[row])
    assert message_json(message).endswith(
        lines(
            '   "inline_bot_buttons": [',
            "    [",
            "     {",
            '      "type": "callback",',
            '      "text": "Go",',
            '      "dataBase64": "-_8B",',
            '      "data": ""',
            "     },",
            "     {",
            '      "type": "url",',
            '      "text": "Site",',
            '      "data": "https://e.x"',
            "     }",
            "    ]",
            "   ]",
            "  }",
        )
    )


def test_reactions_are_indented_one_level_deeper():
    recent = Reaction.Recent(peer_id=ANN, date=D)
    reaction = Reaction(Reaction.Type.Emoji, "\u2764", count=2, recent=[recent])
    message = Message(id=5, date=D, reactions=[reaction])
    assert message_json(message).endswith(
        lines(
            '   "reactions": [',
            "     {",
            '      "type": "emoji",',
            '      "count": 2,',
            '      "emoji": "\u2764",',
            '      "recent": [',
            "       {",
            '        "from": "Ann",',
            '        "from_id": "user100",',
            f'        "date": {ISO}',
            "       }",
            "      ]",
            "     }",
            "    ]",
            "  }",
        )
    )


# Rich messages


def test_rich_message_replaces_text():
    paragraph = RichBlock(kind=RichBlock.Kind.Paragraph)
    paragraph.text = RichText(type=RichText.Type.Plain, text="Hi")
    photo = RichBlock(kind=RichBlock.Kind.Photo, photo_id=77)
    message = Message(id=5, date=D, rich_message=RichMessage(blocks=[paragraph, photo]))
    assert message_json(message) == lines(
        *HEAD,
        f'   "date_unixtime": {RAW},',
        '   "rich_message": {',
        '    "rtl": false,',
        '    "part": false,',
        '    "blocks": [',
        "     {",
        '      "type": "paragraph",',
        '      "text": {',
        '       "type": "plain",',
        '       "text": "Hi"',
        "      }",
        "     },",
        "     {",
        '      "type": "photo",',
        '      "photo_id": "77",',
        '      "photo_skip_reason": "unavailable",',
        '      "spoiler": false,',
        '      "caption": {',
        '       "text": {',
        '        "type": "empty"',
        "       },",
        '       "credit": {',
        '        "type": "empty"',
        "       }",
        "      }",
        "     }",
        "    ]",
        "   }",
        "  }",
    )


def test_rich_button_row():
    button = RichText(type=RichText.Type.Button)
    button.children = [RichText(type=RichText.Type.Plain, text="Go")]
    action = InlineButtonAction(type=InlineButtonAction.Type.Callback, callback_data=b"\x01")
    button.button = RichButtonPayload(action, RichButtonStyle.Primary)
    row = RichBlock(kind=RichBlock.Kind.ButtonRow, buttons=[button])
    assert serialize_rich_message(JsonContext(), RichMessage(blocks=[row])) == lines(
        "{",
        ' "rtl": false,',
        ' "part": false,',
        ' "blocks": [',
        "  {",
        '   "type": "button_row",',
        '   "alignment": "stretch",',
        '   "buttons": [',
        "    {",
        '     "text": {',
        '      "type": "plain",',
        '      "text": "Go"',
        "     },",
        '     "button": {',
        '      "type": "callback",',
        '      "requires_password": false,',
        '      "dataBase64": "AQ",',
        '      "data": ""',
        "     },",
        '     "style": "primary"',
        "    }",
        "   ]",
        "  }",
        " ]",
        "}",
    )


def test_rich_video_document_and_unsigned_ids():
    video = Document(file=File(relative_path="video_files/v.mp4", size=9), mime="video/mp4")
    video.duration, video.width, video.height, video.is_video_file = 3, 4, 2, True
    link = RichText(type=RichText.Type.Url, data="https://e.x", id=-1)
    link.children = [RichText(type=RichText.Type.Plain, text="l")]
    blocks = [
        RichBlock(kind=RichBlock.Kind.Video, document_id=12),
        RichBlock(kind=RichBlock.Kind.Paragraph, text=link),
    ]
    body = serialize_rich_message(JsonContext(), RichMessage(blocks, documents={12: video}))
    assert (
        lines(
            '   "document_id": "12",',
            '   "file": "video_files/v.mp4",',
            '   "file_size": 9,',
            '   "media_type": "video_file",',
            '   "mime_type": "video/mp4",',
            '   "duration_seconds": 3,',
            '   "width": 4,',
            '   "height": 2,',
            '   "autoplay": false,',
        )
        in body
    )
    # uint64 fields print unsigned, as tdesktop's NumberToString(uint64) does.
    assert b'"href": "https://e.x",\n    "webpage_id": "18446744073709551615",' in body


# Output file and the combined writer


def test_output_file_creates_folder_and_truncates(tmp_path):
    path = tmp_path / "sub dir" / "result.json"
    path.parent.mkdir()
    path.write_bytes(b"stale content")
    output = OutputFile(str(path))
    output.write_block(b"")
    assert path.read_bytes() == b""
    output.write_block(b"ab")
    output.write_block(b"c")
    output.close()
    assert path.read_bytes() == b"abc" and output.size() == 3
    fresh = OutputFile(str(tmp_path / "new" / "x.json"))
    fresh.write_block(b"1")
    fresh.close()
    assert (tmp_path / "new" / "x.json").read_bytes() == b"1"


class _Recorder:
    def __init__(self, name: str, calls: list, fail: bool = False) -> None:
        self.name, self.calls, self.fail = name, calls, fail

    def main_file_path(self) -> str:
        return self.name + ".path"

    def __getattr__(self, method: str):
        def call(*args):
            self.calls.append((self.name, method))
            if self.fail:
                raise OSError(self.name)

        return call


def test_html_and_json_calls_both_and_reports_the_last_failure():
    calls: list = []
    writer = HtmlAndJsonWriter(_Recorder("html", calls, fail=True), _Recorder("json", calls))
    with pytest.raises(OSError, match="html"):
        writer.write_dialog_end()
    assert calls == [("html", "write_dialog_end"), ("json", "write_dialog_end")]
    assert writer.main_file_path() == "html.path"
