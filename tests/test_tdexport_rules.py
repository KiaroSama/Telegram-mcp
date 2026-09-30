"""tdesktop export rules outside message parsing: settings, file names, folders, formatting,
peers, dialogs and rich messages (spec 030, stream A). Synthetic data only.
"""

from datetime import date, timezone

import pytest
from telethon.tl import types as tl

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.files import (
    SkipReason,
    apply_file_policy,
    document_media_type,
    file_name_from_user_string,
    mime_first_glob,
    normalize_path,
    prepare_relative_path,
)
from telegram_mcp.tdexport.model import (
    Document,
    File,
    FileLocation,
    ParseMediaContext,
    peer_from_channel,
    peer_from_chat,
    peer_to_bare_id,
    refresh_file_reference,
)
from telegram_mcp.tdexport.model_dialogs import (
    DialogInfo,
    add_migrate_from_slice,
    dialog_type_from_chat,
    finalize_dialogs_info,
    parse_dialogs_info_chats,
    parse_dialogs_info_users,
)
from telegram_mcp.tdexport.model_format import (
    CallingCodeInfo,
    CountryInfo,
    fill_amount_and_currency,
    format_date_time,
    format_duration,
    format_file_size,
    format_phone_number,
    number_to_string,
    set_countries_list,
)
from telegram_mcp.tdexport.model_peers import empty_peer, parse_chat, parse_peers_lists, parse_user
from telegram_mcp.tdexport.model_rich import RichBlock, RichButtonStyle, RichText
from telegram_mcp.tdexport.model_rich_parse import (
    RichVisitor,
    extract_full_rich_message,
    parse_rich_message,
    visit_rich_message,
)
from telegram_mcp.tdexport.settings import (
    Format,
    MediaSettings,
    Settings,
    normalize_settings,
)


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def test_settings_defaults_match_tdesktop():
    settings = Settings()
    assert settings.media.types == MediaSettings.Type.Photo
    assert settings.media.size_limit == 8 * 1024 * 1024
    assert settings.format == Format.Html and not settings.only_single_peer()
    assert settings.validate()
    single = normalize_settings(Settings(single_peer=tl.InputPeerSelf()))
    assert single.types == single.full_chats == Settings.Type.AnyChatsMask
    assert not Settings(single_peer_from=10, single_peer_till=5).validate()
    assert not MediaSettings(size_limit=4001 * 1024 * 1024).validate()


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("report.pdf", "report.pdf"),
        ('a<b>c:"d|e?f*g\\h/i', "a_b_c__d_e_f_g_h_i"),
        ("x\u202egnp.exe", "x_gnp.exe"),
        ("trailing.", "trailing._"),
        ("", "_"),
        ("con.txt", "_con.txt"),
        ("CONSOLE.txt", "CONSOLE.txt"),
        ("link.lnk", "link.lnk.download"),
    ],
)
def test_file_name_from_user_string(name, expected):
    assert file_name_from_user_string(name) == expected


def test_mime_first_glob_uses_tdesktop_known_types_and_qt_database():
    assert mime_first_glob("image/webp") == "*.webp"
    assert mime_first_glob("application/x-tgsticker") == "*.tgs"
    assert mime_first_glob("video/mp4") == "*.mp4"
    assert mime_first_glob("image/jpeg") == "*.jpg"
    # audio/mpeg3 goes through the audio/mp3 alias of audio/mpeg.
    assert mime_first_glob("audio/mpeg3") == mime_first_glob("audio/mpeg") != ""
    assert mime_first_glob("application/unknown-thing") == ""


def test_prepare_relative_path_adds_counter_before_first_dot(tmp_path):
    folder = str(tmp_path).replace("\\", "/") + "/"
    assert prepare_relative_path(folder, "files/a.tar.gz") == "files/a.tar.gz"
    (tmp_path / "files").mkdir()
    (tmp_path / "files" / "a.tar.gz").write_bytes(b"")
    assert prepare_relative_path(folder, "files/a.tar.gz") == "files/a (1).tar.gz"
    (tmp_path / "files" / "a (1).tar.gz").write_bytes(b"")
    assert prepare_relative_path(folder, "files/a.tar.gz") == "files/a (2).tar.gz"


def test_normalize_path_uses_chat_export_folder(tmp_path):
    single = Settings(single_peer=tl.InputPeerSelf(), path=str(tmp_path))
    base = str(tmp_path).replace("\\", "/")
    # An empty or missing folder is used as it is.
    assert normalize_path(single, date(2026, 9, 30)) == base + "/"
    (tmp_path / "other.txt").write_text("x", encoding="utf-8")
    assert normalize_path(single, date(2026, 9, 30)) == base + "/ChatExport_2026-09-30/"
    (tmp_path / "ChatExport_2026-09-30").mkdir()
    assert normalize_path(single, date(2026, 9, 30)) == base + "/ChatExport_2026-09-30 (1)/"
    forced = Settings(path=str(tmp_path / "empty"), force_sub_path=True)
    assert normalize_path(forced, date(2026, 1, 2)).endswith("/empty/DataExport_2026-01-02/")


def test_apply_file_policy_order():
    media = MediaSettings(types=MediaSettings.Type.Photo, size_limit=100)
    located = FileLocation(
        1, tl.InputPhotoFileLocation(id=1, access_hash=1, file_reference=b"", thumb_size="y")
    )

    def decide(file, **kwargs):
        options = dict(skip_by_date=False, media_type=MediaSettings.Type.Photo, controlling_size=1)
        options.update(kwargs)
        done = apply_file_policy(file, media=media, **options)
        return done, file.skip_reason

    assert decide(File(location=located), skip_by_date=True) == (True, SkipReason.DateLimits)
    assert decide(File()) == (True, SkipReason.Unavailable)
    assert decide(File(content=b"x"), media_type=MediaSettings.Type.Video) == (
        True,
        SkipReason.FileType,
    )
    assert decide(File(location=located), controlling_size=101) == (True, SkipReason.FileSize)
    assert decide(File(location=located)) == (False, SkipReason.None_)
    assert decide(File(relative_path="done")) == (True, SkipReason.None_)
    sticker = Document(is_sticker=True, is_animated=True)
    assert document_media_type(sticker) == MediaSettings.Type.Sticker
    assert document_media_type(Document(is_animated=True)) == MediaSettings.Type.GIF


def test_refresh_file_reference_only_for_same_file():
    old = FileLocation(
        2, tl.InputDocumentFileLocation(id=5, access_hash=1, file_reference=b"a", thumb_size="")
    )
    new = FileLocation(
        2, tl.InputDocumentFileLocation(id=5, access_hash=1, file_reference=b"b", thumb_size="")
    )
    other = FileLocation(
        2, tl.InputDocumentFileLocation(id=6, access_hash=1, file_reference=b"c", thumb_size="")
    )
    assert not refresh_file_reference(old, other)
    assert refresh_file_reference(old, new) and old.data.file_reference == b"b"


def test_formatting_helpers():
    stamp = 1_705_053_605  # 2024-01-12 10:00:05 UTC
    assert format_date_time(stamp) == "12.01.2024 10:00:05"
    assert format_date_time(stamp, True) == "12.01.2024 10:00:05 UTC+00:00"
    assert format_date_time(stamp, False, "-", "-", "_") == "12-01-2024_10-00-05"
    assert format_date_time(0) == ""
    assert [format_file_size(v) for v in (512, 1536, 5 * 1024 * 1024 + 1)] == [
        "512 B",
        "1.5 KB",
        "5.0 MB",
    ]
    assert [format_duration(v) for v in (5, 65, 3725)] == ["00:05", "01:05", "1:02:05"]
    assert number_to_string(7, 3) == "007"
    assert fill_amount_and_currency(123456, "USD") == "$1,234.56"
    assert fill_amount_and_currency(-500, "JPY") == "\u2212\u00a5500"
    assert fill_amount_and_currency(100000, "DKK") == "1000,00 DKK"
    assert fill_amount_and_currency(1500, "XTR") == "\u2b501,500"
    assert fill_amount_and_currency(700, "ZZZ") == "ZZZ7.00"


def test_phone_formatting_uses_server_country_list():
    set_countries_list([])
    try:
        assert format_phone_number("79991234567") == "79991234567"
        set_countries_list(
            [CountryInfo("Russia", "RU", [CallingCodeInfo("7", [""], ["XXX XXX XXXX"])])]
        )
        assert format_phone_number("+7 9991234567") == "+7 999 123 4567"
        assert format_phone_number("0123") == "0123"
        assert format_phone_number("") == ""
    finally:
        set_countries_list([])


def test_users_chats_and_peers():
    bot = parse_user(tl.User(id=10, first_name="B", bot=True, bot_info_version=1, access_hash=5))
    assert bot.is_bot and bot.input == tl.InputUser(user_id=10, access_hash=5)
    replies = parse_user(tl.User(id=1271266957, color=tl.PeerColor(color=3)))
    assert replies.is_replies and replies.color_index == 3
    channel = parse_chat(
        tl.Channel(
            id=7, title="C", photo=None, date=None, broadcast=True, creator=True, username="c"
        )
    )
    assert channel.id() == peer_from_channel(7) and channel.has_monoforum_admin_rights
    group = parse_chat(
        tl.Chat(id=8, title="G", photo=None, participants_count=1, date=None, version=1)
    )
    assert group.id() == peer_from_chat(8) and group.color_index == 0
    monoforum = tl.Channel(
        id=9,
        title="M",
        photo=None,
        date=None,
        megagroup=True,
        monoforum=True,
        linked_monoforum_id=7,
    )
    peers = parse_peers_lists(
        [tl.User(id=10)],
        [
            monoforum,
            tl.Channel(
                id=7, title="C", photo=None, date=None, broadcast=True, creator=True, username="c"
            ),
        ],
    )
    linked = peers[peer_from_channel(9)].chat()
    assert linked.is_monoforum_admin and linked.is_monoforum_of_public_broadcast
    assert list(peers) == sorted(peers)
    assert peer_to_bare_id(peer_from_channel(9)) == 9
    assert empty_peer(peer_from_channel(9)).id() == peer_from_chat(0)


def test_dialog_info_rules():
    users = [tl.User(id=5, first_name="A", last_name="B"), tl.User(id=6, is_self=True)]
    info = parse_dialogs_info_users(tl.InputPeerUser(user_id=5, access_hash=1), users)
    assert [(d.peer_id, d.name, d.last_name, d.type) for d in info.chats] == [
        (5, "A", "B", DialogInfo.Type.Personal)
    ]
    self_info = parse_dialogs_info_users(tl.InputPeerSelf(), users)
    assert [d.type for d in self_info.chats] == [DialogInfo.Type.Self]
    chats = [tl.Channel(id=3, title="S", photo=None, date=None, megagroup=True, access_hash=2)]
    group = parse_dialogs_info_chats(tl.InputPeerChannel(channel_id=3, access_hash=2), chats)
    assert group.chats[0].type == DialogInfo.Type.PrivateSupergroup
    assert dialog_type_from_chat(parse_chat(chats[0])) == DialogInfo.Type.PrivateSupergroup

    to = group.chats[0]
    to.splits, to.messages_count_per_split = [1], [0]
    source = DialogInfo(
        peer_id=peer_from_chat(4),
        input=tl.InputPeerChat(chat_id=4),
        splits=[0],
        messages_count_per_split=[3],
    )
    assert add_migrate_from_slice(to, source, 1, 2)
    assert to.splits == [1, -2, -1] and to.messages_count_per_split == [0, 3, 0]
    assert to.migrated_from_input == tl.InputPeerChat(chat_id=4)

    settings = normalize_settings(Settings(single_peer=tl.InputPeerSelf()))
    finalize_dialogs_info(group, settings)
    assert to.relative_path == "" and not to.only_my_messages and to.splits == [-2, -1, 1]


def test_rich_message_parsing_and_visiting():
    text = tl.TextConcat(
        texts=[
            tl.TextBold(text=tl.TextPlain(text="Hi ")),
            tl.TextUrl(text=tl.TextPlain(text="link"), url="https://x", webpage_id=4),
            tl.TextCustomEmoji(document_id=77, alt="*"),
        ]
    )
    button_row = tl.PageBlockButtonRow(
        buttons=[
            tl.PageButton(
                text=tl.TextPlain(text="go"),
                type=tl.InlineButtonTypeCallback(data=b"1"),
                style=tl.RichButtonStyle(link=True, bg_danger=True),
            )
        ],
        align_center=True,
    )
    inline_button = tl.TextButton(
        text=tl.TextPlain(text="b"),
        type=tl.InlineButtonTypeCallback(data=b"2"),
        style=tl.RichButtonStyle(link=True),
    )
    blocks = [
        tl.PageBlockHeading2(text=tl.TextPlain(text="H")),
        tl.PageBlockParagraph(text=text),
        tl.PageBlockParagraph(text=inline_button),
        tl.PageBlockPhoto(
            photo_id=50, caption=tl.PageCaption(text=tl.TextEmpty(), credit=tl.TextEmpty())
        ),
        tl.PageBlockOrderedList(
            items=[
                tl.PageListOrderedItemText(
                    text=tl.TextPlain(text="i"), num="1", checkbox=True, checked=True
                )
            ],
            start=3,
        ),
        tl.PageBlockTable(
            title=tl.TextEmpty(),
            rows=[
                tl.PageTableRow(
                    cells=[
                        tl.PageTableCell(
                            header=True, align_right=True, text=tl.TextPlain(text="c"), colspan=2
                        )
                    ]
                )
            ],
        ),
        tl.PageBlockDetails(
            blocks=[tl.PageBlockDivider()], title=tl.TextPlain(text="d"), open=True
        ),
        button_row,
    ]
    photo = tl.Photo(
        id=50,
        access_hash=1,
        file_reference=b"",
        date=0,
        sizes=[tl.PhotoSize(type="x", w=10, h=10, size=5)],
        dc_id=1,
    )
    rich = tl.RichMessage(blocks=blocks, photos=[photo], documents=[])
    context = ParseMediaContext()
    parsed = parse_rich_message(context, rich, "", 0)
    assert (
        context.photos == 1 and parsed.photos[50].image.file.suggested_path == "photos/photo_1.jpg"
    )
    kinds = [b.kind for b in parsed.blocks]
    assert kinds == [
        RichBlock.Kind.Heading,
        RichBlock.Kind.Paragraph,
        RichBlock.Kind.Paragraph,
        RichBlock.Kind.Photo,
        RichBlock.Kind.List,
        RichBlock.Kind.Table,
        RichBlock.Kind.Details,
        RichBlock.Kind.ButtonRow,
    ]
    assert parsed.blocks[0].heading_level == 2
    emoji = parsed.blocks[1].text.children[2]
    assert (emoji.type, emoji.custom_emoji_data, emoji.text) == (
        RichText.Type.CustomEmoji,
        "77",
        "*",
    )
    # The link style applies to inline buttons only; a page button falls back to its colour.
    assert parsed.blocks[2].text.button.style == RichButtonStyle.Link
    assert parsed.blocks[7].buttons[0].button.style == RichButtonStyle.Danger
    item = parsed.blocks[4].list_items[0]
    assert (item.num, item.task_state.name, parsed.blocks[4].ordered_list.start) == (
        "1",
        "Checked",
        3,
    )
    cell = parsed.blocks[5].table_rows[0].cells[0]
    assert cell.header and cell.alignment.name == "Right" and cell.colspan == 2

    seen = []
    visit_rich_message(
        parsed, RichVisitor(text=lambda t: seen.append(t.type), photo=lambda p: seen.append(p.id))
    )
    assert 50 in seen and RichText.Type.CustomEmoji in seen

    full = tl.Message(id=3, peer_id=tl.PeerUser(user_id=1), rich_message=rich)
    assert extract_full_rich_message([full], 3) is rich
    partial = tl.Message(
        id=3,
        peer_id=tl.PeerUser(user_id=1),
        rich_message=tl.RichMessage(blocks=[], photos=[], documents=[], part=True),
    )
    assert extract_full_rich_message([partial], 3) is None
    assert extract_full_rich_message([full, full], 3) is None
