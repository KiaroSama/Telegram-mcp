"""The JSON export writer: result.json exactly as Telegram Desktop writes it.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_json.{h,cpp}
JsonWriter), GPL-3.0. The account-wide sections (personal information, profile pictures,
stories, profile music, contacts, sessions, other data) are not ported: a single-chat export
never writes them.
"""

from __future__ import annotations

from dataclasses import replace
from enum import Enum

from .json_message import serialize_message
from .json_serialize import (
    K_ARRAY,
    K_OBJECT,
    JsonContext,
    indentation,
    number,
    serialize_string,
    string_allow_null,
)
from .model import peer_to_bare_id
from .model_dialogs import DialogInfo, DialogsInfo
from .model_message import MessagesSlice, skip_message_by_date
from .output_file import File
from .settings import Environment, Format, Settings

_DIALOG_TYPE_NAMES = {
    DialogInfo.Type.Unknown: "",
    DialogInfo.Type.Self: "saved_messages",
    DialogInfo.Type.Replies: "replies",
    DialogInfo.Type.VerifyCodes: "verification_codes",
    DialogInfo.Type.Personal: "personal_chat",
    DialogInfo.Type.Bot: "bot_chat",
    DialogInfo.Type.PrivateGroup: "private_group",
    DialogInfo.Type.PrivateSupergroup: "private_supergroup",
    DialogInfo.Type.PublicSupergroup: "public_supergroup",
    DialogInfo.Type.PrivateChannel: "private_channel",
    DialogInfo.Type.PublicChannel: "public_channel",
}


class _DialogsMode(Enum):
    None_ = 0
    Chats = 1
    Left = 2


class JsonWriter:
    """Output::JsonWriter, driven through the ExportWriter protocol of fetch.py."""

    def __init__(self) -> None:
        self._settings = Settings()
        self._environment = Environment()
        self._context = JsonContext()
        self._current_nesting_had_item = False
        self._dialogs_mode = _DialogsMode.None_
        self._output: File | None = None

    def format(self) -> Format:
        return Format.Json

    def start(self, settings: Settings, environment: Environment) -> None:
        assert self._output is None
        assert settings.path.endswith("/")
        self._settings = replace(settings)
        self._environment = environment
        self._output = File(self.main_file_path())
        if self._settings.only_single_peer():
            return
        block = self._push_nesting(K_OBJECT)
        block += self._prepare_object_item_start("about")
        block += serialize_string(self._environment.about_telegram)
        self._write(block)

    def write_dialogs_start(self, data: DialogsInfo) -> None:
        pass

    def write_dialog_start(self, data: DialogInfo) -> None:
        if not self._settings.only_single_peer():
            self._validate_dialogs_mode(data.is_left_channel)
        kind = DialogInfo.Type
        block = b"" if self._settings.only_single_peer() else self._prepare_array_item_start()
        block += self._push_nesting(K_OBJECT)
        if data.type not in (kind.Self, kind.Replies, kind.VerifyCodes):
            block += self._prepare_object_item_start("name") + string_allow_null(data.name)
        block += self._prepare_object_item_start("type") + string_allow_null(
            _DIALOG_TYPE_NAMES[data.type]
        )
        block += self._prepare_object_item_start("id") + number(peer_to_bare_id(data.peer_id))
        block += self._prepare_object_item_start("messages")
        block += self._push_nesting(K_ARRAY)
        self._write(block)

    def write_dialog_slice(self, data: MessagesSlice) -> None:
        block = b""
        for message in data.list:
            if skip_message_by_date(message, self._settings):
                continue
            block += self._prepare_array_item_start() + serialize_message(
                self._context, message, data.peers, self._environment.internal_links_domain
            )
        if block:
            self._write(block)

    def write_dialog_end(self) -> None:
        block = self._pop_nesting()
        self._write(block + self._pop_nesting())

    def write_dialogs_end(self) -> None:
        if not self._settings.only_single_peer():
            self._write_chats_end()

    def finish(self) -> None:
        if self._settings.only_single_peer():
            assert not self._context.nesting
        else:
            block = self._pop_nesting()
            assert not self._context.nesting
            self._write(block)
        assert self._output is not None
        self._output.close()

    def main_file_path(self) -> str:
        return self._settings.path + "result.json"

    def _write(self, block: bytes) -> None:
        assert self._output is not None
        self._output.write_block(block)

    def _push_nesting(self, kind: bool) -> bytes:
        self._context.nesting.append(kind)
        self._current_nesting_had_item = False
        return b"{" if kind == K_OBJECT else b"["

    def _prepare_object_item_start(self, key: str) -> bytes:
        had = self._current_nesting_had_item
        self._current_nesting_had_item = True
        return (
            (b",\n" if had else b"\n") + indentation(self._context) + serialize_string(key) + b": "
        )

    def _prepare_array_item_start(self) -> bytes:
        had = self._current_nesting_had_item
        self._current_nesting_had_item = True
        return (b",\n" if had else b"\n") + indentation(self._context)

    def _pop_nesting(self) -> bytes:
        kind = self._context.nesting.pop()
        self._current_nesting_had_item = True
        return b"\n" + indentation(self._context) + (b"}" if kind == K_OBJECT else b"]")

    def _validate_dialogs_mode(self, is_left_channel: bool) -> None:
        mode = _DialogsMode.Left if is_left_channel else _DialogsMode.Chats
        if self._dialogs_mode == mode:
            return
        if self._dialogs_mode != _DialogsMode.None_:
            self._write_chats_end()
        self._dialogs_mode = mode
        self._write_chats_start(
            "left_chats" if is_left_channel else "chats",
            (
                self._environment.about_left_chats
                if is_left_channel
                else self._environment.about_chats
            ),
        )

    def _write_chats_start(self, list_name: str, about: str) -> None:
        block = self._prepare_object_item_start(list_name)
        block += self._push_nesting(K_OBJECT)
        block += self._prepare_object_item_start("about")
        block += serialize_string(about)
        block += self._prepare_object_item_start("list")
        self._write(block + self._push_nesting(K_ARRAY))

    def _write_chats_end(self) -> None:
        block = self._pop_nesting()
        self._write(block + self._pop_nesting())
