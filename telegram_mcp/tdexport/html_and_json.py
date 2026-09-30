"""The "HTML and JSON" export format: every call goes to both writers.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/
export_output_html_and_json.cpp), GPL-3.0. The two writers are injected (HTML first, then JSON,
tdesktop's order) instead of created here.
"""

from __future__ import annotations

from typing import Any, Callable

from .model_dialogs import DialogInfo, DialogsInfo
from .model_message import MessagesSlice
from .settings import Environment, Format, Settings


class HtmlAndJsonWriter:
    """Output::HtmlAndJsonWriter."""

    def __init__(self, html_writer: Any, json_writer: Any) -> None:
        self._writers = [html_writer, json_writer]

    def format(self) -> Format:
        return Format.HtmlAndJson

    def start(self, settings: Settings, environment: Environment) -> None:
        self._invoke(lambda writer: writer.start(settings, environment))

    def write_dialogs_start(self, data: DialogsInfo) -> None:
        self._invoke(lambda writer: writer.write_dialogs_start(data))

    def write_dialog_start(self, data: DialogInfo) -> None:
        self._invoke(lambda writer: writer.write_dialog_start(data))

    def write_dialog_slice(self, data: MessagesSlice) -> None:
        self._invoke(lambda writer: writer.write_dialog_slice(data))

    def write_dialog_end(self) -> None:
        self._invoke(lambda writer: writer.write_dialog_end())

    def write_dialogs_end(self) -> None:
        self._invoke(lambda writer: writer.write_dialogs_end())

    def finish(self) -> None:
        self._invoke(lambda writer: writer.finish())

    def main_file_path(self) -> str:
        return str(self._writers[0].main_file_path())

    def _invoke(self, method: Callable[[Any], None]) -> None:
        """Every writer runs even when one fails; the last failure is the one reported."""
        failure: Exception | None = None
        for writer in self._writers:
            try:
                method(writer)
            except Exception as error:  # noqa: BLE001 - tdesktop keeps going, then reports.
                failure = error
        if failure is not None:
            raise failure
