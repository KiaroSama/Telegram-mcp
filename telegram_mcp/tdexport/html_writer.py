"""The HTML export writer: pages on disk, paging, and the single-chat writer protocol.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
HtmlWriter::Wrap (file handling, composeStart, pushHeader, close) and HtmlWriter start/
writeDialog*/finish/wrapMessageLink/switchToNextChatFile/writeDialogOpening/
writeEmptySinglePeer/copyFile/mainFilePath/messagesFile, output/export_output_file.cpp
File::Copy), GPL-3.0. Output::File itself is output_file.File.

Only the single-chat export is ported: tdesktop's summary page, contacts, sessions, userpics,
stories, profile music and chats list are written only for an account-wide export.
"""

from __future__ import annotations

from pathlib import Path

from .html_message import MessageInfo, MessageMixin
from .html_text import PeersMap, display_date, format_date_text, serialize_string
from .model_dialogs import DialogInfo, DialogsInfo
from .model_message import MessagesSlice, skip_message_by_date
from .output_file import File
from .settings import Environment, Settings

MESSAGES_IN_FILE = 1000

_ASSETS = Path(__file__).parent / "assets"
# HtmlWriter::start's `files` list; every .png is followed by its @2x twin.
_ASSET_FILES = (
    "css/style.css",
    "images/back.png",
    "images/media_call.png",
    "images/media_contact.png",
    "images/media_file.png",
    "images/media_game.png",
    "images/media_location.png",
    "images/media_music.png",
    "images/media_photo.png",
    "images/media_shop.png",
    "images/media_video.png",
    "images/media_voice.png",
    "images/section_calls.png",
    "images/section_chats.png",
    "images/section_contacts.png",
    "images/section_frequent.png",
    "images/section_music.png",
    "images/section_other.png",
    "images/section_photos.png",
    "images/section_sessions.png",
    "images/section_stories.png",
    "images/section_web.png",
    "js/script.js",
)


def _encode(text: str) -> bytes:
    return text.encode("utf-8", "surrogateescape")


class Wrap(MessageMixin):
    """HtmlWriter::Wrap: one HTML page."""

    def __init__(self, path: str, base: str) -> None:
        if not base.endswith("/") or not path.startswith(base):
            raise ValueError("Wrap path must lie inside the export folder.")
        super().__init__("../" * path[len(base) :].count("/"))
        self._file = File(path)
        self._closed = False
        self._composed_start = self._compose_start()

    def empty(self) -> bool:
        return self._file.empty()

    def write_block(self, block: str) -> None:
        if self._closed:
            raise ValueError("Write to a closed HTML page.")
        if block and self._file.empty():
            block = self._composed_start + block
        try:
            self._file.write_block(_encode(block))
        except OSError:
            self._closed = True
            raise

    def close(self) -> None:
        if not self._closed:
            self._closed = True
            if not self._file.empty():
                block = ""
                while not self._context.empty():
                    block += self.pop_tag()
                self._file.write_block(_encode(block))
        self._file.close()

    def push_header(self, header: str, path: str = "") -> str:
        result = self.push_div("page_header")
        if not path:
            result += self.push_div("content")
        else:
            result += self.push_tag(
                "a",
                {
                    "class": "content block_link",
                    "href": self.relative_path(path),
                    "onclick": "return GoBack(this)",
                },
            )
        result += self.push_div("text bold") + serialize_string(header)
        return result + self.pop_tag() + self.pop_tag() + self.pop_tag()

    def _compose_start(self) -> str:
        result = "<!DOCTYPE html>" + self.push_tag("html")
        result += self.push_tag("head")
        result += self.push_tag("meta", {"charset": "utf-8", "empty": ""})
        result += self.push_tag("title", {"inline": ""}) + "Exported Data" + self.pop_tag()
        result += self.push_tag(
            "meta",
            {
                "name": "viewport",
                "content": "width=device-width, initial-scale=1.0",
                "empty": "",
            },
        )
        result += self.push_tag(
            "link", {"href": self._base + "css/style.css", "rel": "stylesheet", "empty": ""}
        )
        result += self.push_tag(
            "script", {"src": self._base + "js/script.js", "type": "text/javascript"}
        )
        result += self.pop_tag() + self.pop_tag()
        result += self.push_tag("body", {"onload": "CheckLocation();"})
        return result + self.push_div("page_wrap")


def messages_file(index: int) -> str:
    return "messages" + (str(index + 1) if index > 0 else "") + ".html"


class HtmlWriter:
    """Output::HtmlWriter for a single-chat export (settings.only_single_peer())."""

    def __init__(self) -> None:
        self._settings = Settings()
        self._environment = Environment()
        self._dialog = DialogInfo()
        self._messages_count = 0
        self._last_message_info: MessageInfo | None = None
        self._date_message_id = 0
        self._chat: Wrap | None = None
        self._last_message_ids_per_file: list[int] = []
        self._chat_file_empty = False

    def start(self, settings: Settings, environment: Environment) -> None:
        if not settings.path.endswith("/"):
            raise ValueError("Export path must end with '/'.")
        if not settings.only_single_peer():
            raise NotImplementedError("Only the single-chat HTML export is ported.")
        self._settings = settings
        self._environment = environment
        for name in _ASSET_FILES:
            self._copy_file(name)
            png = name.find(".png")
            if png > 0:
                self._copy_file(name[:png] + "@2x.png")

    def _copy_file(self, relative_path: str) -> None:
        data = (_ASSETS / relative_path).read_bytes()
        target = File(self._path_with_relative_path(relative_path))
        try:
            target.write_block(data)
        finally:
            target.close()

    def write_dialogs_start(self, data: DialogsInfo) -> None:
        return None

    def write_dialog_start(self, data: DialogInfo) -> None:
        if self._chat is not None:
            raise ValueError("A dialog is already open.")
        self._chat = self._file_with_relative_path(data.relative_path + messages_file(0))
        self._chat_file_empty = True
        self._messages_count = 0
        self._date_message_id = 0
        self._last_message_info = None
        self._last_message_ids_per_file = []
        self._dialog = data

    def write_dialog_slice(self, data: MessagesSlice) -> None:
        chat = self._require_chat()
        peers = PeersMap(data.peers)
        old_index = (self._messages_count - 1) // MESSAGES_IN_FILE if self._messages_count else 0
        previous = self._last_message_info
        saved: MessageInfo | None = None
        block = ""
        for message in data.list:
            if skip_message_by_date(message, self._settings):
                continue
            new_index = self._messages_count // MESSAGES_IN_FILE
            if old_index != new_index:
                chat.write_block(block)
                chat = self._switch_to_next_chat_file(new_index)
                last = saved if saved is not None else self._last_message_info
                if last is None:
                    raise ValueError("No message before a page switch.")
                self._last_message_ids_per_file.append(last.id)
                block = ""
                self._last_message_info = previous = saved = None
                old_index = new_index
            if self._chat_file_empty:
                self._write_dialog_opening(old_index)
                self._chat_file_empty = False
            if display_date(message.date, previous.date if previous is not None else 0):
                self._date_message_id -= 1
                block += chat.push_service_message(
                    self._date_message_id,
                    self._dialog,
                    self._settings.path,
                    format_date_text(message.date),
                )
            info, content = chat.push_message(
                message,
                previous,
                self._dialog,
                self._settings.path,
                peers,
                self._environment.internal_links_domain,
                self.wrap_message_link,
            )
            block += content
            self._messages_count += 1
            saved = previous = info
        if saved is not None:
            self._last_message_info = saved
        if block:
            chat.write_block(block)

    def _write_empty_single_peer(self) -> None:
        chat = self._require_chat()
        if self._messages_count != 0:
            return
        self._write_dialog_opening(0)
        self._date_message_id -= 1
        chat.write_block(
            chat.push_service_message(
                self._date_message_id, self._dialog, self._settings.path, "No exported messages"
            )
        )

    def write_dialog_end(self) -> None:
        self._write_empty_single_peer()
        chat, self._chat = self._require_chat(), None
        chat.close()

    def write_dialogs_end(self) -> None:
        return None

    def finish(self) -> None:
        return None

    def main_file_path(self) -> str:
        return self._path_with_relative_path(messages_file(0))

    def wrap_message_link(self, message_id: int, text: str) -> str:
        for index, max_message_id in enumerate(self._last_message_ids_per_file):
            if message_id <= max_message_id:
                return f'<a href="{messages_file(index)}#go_to_message{message_id}">{text}</a>'
        return (
            f'<a href="#go_to_message{message_id}" '
            f'onclick="return GoToMessage({message_id})">{text}</a>'
        )

    def _require_chat(self) -> Wrap:
        if self._chat is None:
            raise ValueError("No dialog is open.")
        return self._chat

    def _write_dialog_opening(self, index: int) -> None:
        chat = self._require_chat()
        dialog = self._dialog
        if not dialog.name and not dialog.last_name:
            name = "Deleted Account"
        else:
            name = dialog.name + " " + dialog.last_name
        block = chat.push_header(name, "")
        block += chat.push_div("page_body chat_page")
        block += chat.push_div("history")
        if index > 0:
            block += chat.push_tag(
                "a", {"class": "pagination block_link", "href": messages_file(index - 1)}
            )
            block += "Previous messages" + chat.pop_tag()
        chat.write_block(block)

    def _switch_to_next_chat_file(self, index: int) -> Wrap:
        chat = self._require_chat()
        next_path = messages_file(index)
        link = chat.push_tag("a", {"class": "pagination block_link", "href": next_path})
        chat.write_block(link + "Next messages" + chat.pop_tag())
        chat.close()
        self._chat = self._file_with_relative_path(self._dialog.relative_path + next_path)
        self._chat_file_empty = True
        return self._chat

    def _path_with_relative_path(self, path: str) -> str:
        return self._settings.path + path

    def _file_with_relative_path(self, path: str) -> Wrap:
        return Wrap(self._path_with_relative_path(path), self._settings.path)
