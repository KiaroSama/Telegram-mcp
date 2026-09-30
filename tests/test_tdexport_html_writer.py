"""tdesktop HTML export: the writer end to end into tmp_path - page skeleton, assets, paging,
empty chats and date limits (spec 030, stream C). Synthetic data only.
"""

from datetime import timezone
from pathlib import Path

import pytest
from telethon.tl import types as tl

from telegram_mcp.tdexport import model_format
from telegram_mcp.tdexport.html_writer import HtmlWriter
from telegram_mcp.tdexport.model import TextPart, peer_from_user
from telegram_mcp.tdexport.model_dialogs import DialogInfo, DialogsInfo
from telegram_mcp.tdexport.model_message import Message, MessagesSlice
from telegram_mcp.tdexport.model_peers import ContactInfo, Peer, User
from telegram_mcp.tdexport.settings import Environment, Settings

DATE = 86400 * 365
ANN = peer_from_user(5)
ASSETS = Path(__file__).resolve().parent.parent / "telegram_mcp" / "tdexport" / "assets"

COMPOSE = (
    "<!DOCTYPE html>"
    "\n<html>\n"
    "\n <head>\n"
    '\n  <meta charset="utf-8"/>\n'
    "<title>Exported Data</title>"
    '\n  <meta content="width=device-width, initial-scale=1.0" name="viewport"/>\n'
    '\n  <link href="css/style.css" rel="stylesheet"/>\n'
    '\n  <script src="js/script.js" type="text/javascript">\n'
    "\n  </script>\n"
    "\n </head>\n"
    '\n <body onload="CheckLocation();">\n'
    '\n  <div class="page_wrap">\n'
)
CLOSE = "\n    </div>\n\n   </div>\n\n  </div>\n\n </body>\n\n</html>\n"


def header(name):
    return (
        '\n   <div class="page_header">\n'
        '\n    <div class="content">\n'
        '\n     <div class="text bold">\n' + name + "\n     </div>\n"
        "\n    </div>\n"
        "\n   </div>\n"
        '\n   <div class="page_body chat_page">\n'
        '\n    <div class="history">\n'
    )


def date_line(message_id, text):
    return (
        f'\n     <div class="message service" id="message{message_id}">\n'
        '\n      <div class="body details">\n' + text + "\n      </div>\n"
        "\n     </div>\n"
    )


@pytest.fixture(autouse=True)
def utc_zone(monkeypatch):
    monkeypatch.setattr(model_format, "LOCAL_TIMEZONE", timezone.utc)


def settings_for(tmp_path, **extra):
    path = str(tmp_path).replace("\\", "/") + "/"
    return Settings(path=path, single_peer=tl.InputPeerUser(user_id=5, access_hash=0), **extra)


def peers():
    return {ANN: Peer(User(bare_id=5, info=ContactInfo(5, "Ann", "Lee")))}


def message(message_id, date=DATE, text="Hi", **fields):
    return Message(id=message_id, date=date, from_id=ANN, text=[TextPart(text=text)], **fields)


def export(tmp_path, slices, settings=None, dialog=None):
    writer = HtmlWriter()
    writer.start(settings or settings_for(tmp_path), Environment(internal_links_domain="t/"))
    writer.write_dialogs_start(DialogsInfo())
    writer.write_dialog_start(dialog or DialogInfo(name="Ann", peer_id=ANN))
    for slice_ in slices:
        writer.write_dialog_slice(MessagesSlice(list=slice_, peers=peers()))
    writer.write_dialog_end()
    writer.write_dialogs_end()
    writer.finish()
    return writer


def test_single_message_page_matches_tdesktop(tmp_path):
    writer = export(tmp_path, [[message(1, text="Hi & bye")]])
    assert writer.main_file_path().endswith("/messages.html")
    page = (tmp_path / "messages.html").read_bytes().decode("utf-8")
    assert page == (
        COMPOSE
        + header("Ann ")
        + date_line(-1, "1 January 1971")
        + '\n     <div class="message default clearfix" id="message1">\n'
        '\n      <div class="pull_left userpic_wrap">\n'
        '\n       <div class="userpic userpic4" style="width: 42px; height: 42px">\n'
        '\n        <div class="initials" style="line-height: 42px">\n'
        "AL"
        "\n        </div>\n"
        "\n       </div>\n"
        "\n      </div>\n"
        '\n      <div class="body">\n'
        '\n       <div class="pull_right date details"'
        ' title="01.01.1971 00:00:00 UTC+00:00">\n'
        "00:00"
        "\n       </div>\n"
        '\n       <div class="from_name">\nAnn Lee\n       </div>\n'
        '\n       <div class="text">\nHi &amp; bye\n       </div>\n'
        "\n      </div>\n"
        "\n     </div>\n" + CLOSE
    )


def test_assets_are_copied_like_tdesktop(tmp_path):
    export(tmp_path, [[message(1)]])
    copied = sorted(
        str(path.relative_to(tmp_path)).replace("\\", "/")
        for path in tmp_path.rglob("*")
        if path.is_file() and path.name != "messages.html"
    )
    names = [
        "back",
        "media_call",
        "media_contact",
        "media_file",
        "media_game",
        "media_location",
        "media_music",
        "media_photo",
        "media_shop",
        "media_video",
        "media_voice",
        "section_calls",
        "section_chats",
        "section_contacts",
        "section_frequent",
        "section_music",
        "section_other",
        "section_photos",
        "section_sessions",
        "section_stories",
        "section_web",
    ]
    expected = ["css/style.css", "js/script.js"]
    for name in names:
        expected += [f"images/{name}.png", f"images/{name}@2x.png"]
    assert copied == sorted(expected)
    for relative in expected:
        assert (tmp_path / relative).read_bytes() == (ASSETS / relative).read_bytes()


def test_paging_links_and_cross_page_replies(tmp_path):
    first = [message(i) for i in range(1, 1001)]
    tail = [message(1001, reply_to_msg_id=1), message(1002, reply_to_msg_id=1001)]
    export(tmp_path, [first[:600], first[600:] + tail])
    page1 = (tmp_path / "messages.html").read_text(encoding="utf-8")
    page2 = (tmp_path / "messages2.html").read_text(encoding="utf-8")
    assert page1.count('class="message default clearfix') == 1000
    assert page1.endswith(
        '\n     <a class="pagination block_link" href="messages2.html">\n'
        "Next messages"
        "\n     </a>\n" + CLOSE
    )
    assert page2.startswith(
        COMPOSE
        + header("Ann ")
        + '\n     <a class="pagination block_link" href="messages.html">\n'
        "Previous messages"
        "\n     </a>\n" + date_line(-2, "1 January 1971")
    )
    assert 'In reply to <a href="messages.html#go_to_message1">this message</a>' in page2
    assert (
        'In reply to <a href="#go_to_message1001" onclick="return GoToMessage(1001)">'
        "this message</a>"
    ) in page2
    assert page2.count('class="message default clearfix"') == 1
    assert not (tmp_path / "messages3.html").exists()


def test_empty_and_date_limited_exports(tmp_path):
    export(tmp_path, [], dialog=DialogInfo(peer_id=ANN))
    page = (tmp_path / "messages.html").read_text(encoding="utf-8")
    assert page == (
        COMPOSE + header("Deleted Account") + date_line(-1, "No exported messages") + CLOSE
    )
    limited = tmp_path / "limited"
    settings = settings_for(limited, single_peer_from=DATE + 10, single_peer_till=DATE + 20)
    export(
        limited, [[message(1), message(2, date=DATE + 15), message(3, date=DATE + 20)]], settings
    )
    page = (limited / "messages.html").read_text(encoding="utf-8")
    assert 'id="message2"' in page and 'id="message1"' not in page
    assert 'id="message3"' not in page


def test_existing_page_is_truncated_and_bytes_are_kept(tmp_path):
    (tmp_path / "messages.html").write_bytes(b"x" * 100000)
    mention = [TextPart(TextPart.Type.Mention, "\u00e9t")]
    export(tmp_path, [[Message(id=1, date=DATE, from_id=ANN, text=mention)]])
    data = (tmp_path / "messages.html").read_bytes()
    assert data.startswith(b"<!DOCTYPE html>") and b"xxx" not in data
    # tdesktop cuts the first byte of the UTF-8 mention, leaving a loose continuation byte.
    assert b'<a href="t/\xa9t">\xc3\xa9t</a>' in data


def test_account_wide_export_is_not_ported(tmp_path):
    with pytest.raises(NotImplementedError):
        HtmlWriter().start(Settings(path=str(tmp_path) + "/"), Environment())
