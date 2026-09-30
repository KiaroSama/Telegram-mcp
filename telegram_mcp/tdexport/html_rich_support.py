"""Stateless helpers of the rich-message renderer: attributes, output checks, plain targets.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/output/export_output_html.cpp
NoFileDescription, RichFilePresentation, RichPhotoPresentation, RichDocumentPresentation,
RichFileDescription, RichBlockAttributes, RichHeadingTag, RichBoolAttribute, RichButtonMetadata,
RichButtonPeerTypes, RichButtonAttributes, RichPercentValue, RichAspectRatioStyle,
RichTextHasOutput, RichCaptionHasOutput, RichChannelSourceName, RichChannelStatus,
RichMapPointSourceName, RichMapCoordinates, AppendPlainTarget, PlainTarget), GPL-3.0.
"""

from __future__ import annotations

import base64
import math
from copy import deepcopy
from dataclasses import dataclass
from urllib.parse import quote

from .files import SkipReason
from .html_safe import RICH_TARGET_MAX_BYTES, utf8_size
from .model import Document, File, Photo
from .model_format import number_to_string
from .model_rich import (
    InlineButtonAction,
    InlineButtonPeerType,
    RichButtonStyle,
    RichCaption,
    RichChannel,
    RichMapPoint,
    RichText,
    inline_button_peer_type_to_string,
    rich_button_style_to_string,
)

RICH_TABLE_SPAN_MAX = 1000


@dataclass
class MediaData:
    """details::MediaData: what pushGenericMedia draws."""

    title: str = ""
    description: str = ""
    status: str = ""
    classes: str = ""
    thumb: str = ""
    link: str = ""


def no_file_description(reason: SkipReason) -> str:
    if reason == SkipReason.Unavailable:
        return "Unavailable, please try again later."
    if reason == SkipReason.FileSize:
        return "Exceeds maximum size, change data exporting settings to download."
    if reason == SkipReason.FileType:
        return "Not included, change data exporting settings to download."
    if reason == SkipReason.None_:
        return ""
    raise ValueError("Skip reason in NoFileDescription.")


def rich_file_presentation(file: File | None) -> File:
    result = deepcopy(file) if file is not None else File()
    if result.relative_path:
        result.skip_reason = SkipReason.None_
    elif result.skip_reason not in (SkipReason.FileType, SkipReason.FileSize):
        result.skip_reason = SkipReason.Unavailable
    return result


def rich_photo_presentation(photo: Photo | None) -> Photo:
    result = deepcopy(photo) if photo is not None else Photo()
    result.image.file = rich_file_presentation(photo.image.file if photo is not None else None)
    return result


def rich_document_presentation(document: Document | None) -> Document:
    result = deepcopy(document) if document is not None else Document()
    result.file = rich_file_presentation(document.file if document is not None else None)
    result.thumb.file = rich_file_presentation(
        document.thumb.file if document is not None else None
    )
    return result


def rich_file_description(file: File | None) -> str:
    return no_file_description(rich_file_presentation(file).skip_reason)


def rich_block_attributes(classes: str, kind: str) -> dict[str, str]:
    return {"class": "rich_block " + classes, "data-rich-kind": kind}


def rich_heading_tag(level: int) -> str:
    return f"h{level}" if 1 <= level <= 6 else "h6"


def rich_bool(value: bool) -> str:
    return "true" if value else "false"


def rich_button_metadata(value: str) -> str:
    """QUrl::toPercentEncoding: everything but ALPHA / DIGIT / "-" / "." / "_" / "~"."""
    return quote(value.encode("utf-8", "surrogateescape"), safe="")


def rich_button_peer_types(types: list[InlineButtonPeerType]) -> str:
    listed = ",".join(f'"{inline_button_peer_type_to_string(t)}"' for t in types)
    return rich_button_metadata("[" + listed + "]")


def rich_button_attributes(
    action: InlineButtonAction, style: RichButtonStyle | None, inline_button: bool
) -> dict[str, str]:
    kind = InlineButtonAction.Type
    style_name = rich_button_style_to_string(style or RichButtonStyle.Default)
    type_name = InlineButtonAction.type_to_string(action)
    result = {
        "class": "rich_button "
        + ("rich_button_inline" if inline_button else "rich_button_block")
        + " rich_button_style_"
        + style_name
        + " rich_button_action_"
        + type_name,
        "data-button-type": type_name,
        "inline": "",
    }
    if style is not None:
        result.setdefault("data-button-style", style_name)
    if action.type in (kind.Url, kind.Auth, kind.WebView):
        result.setdefault("data-button-url", rich_button_metadata(action.url))
        if action.type == kind.Auth:
            if action.forward_text is not None:
                result.setdefault(
                    "data-button-forward-text", rich_button_metadata(action.forward_text)
                )
            result.setdefault("data-button-id", number_to_string(action.button_id))
    elif action.type in (kind.Callback, kind.CallbackWithPassword):
        encoded = base64.urlsafe_b64encode(action.callback_data).decode("ascii").rstrip("=")
        result.setdefault("data-callback-data-base64", encoded)
        result.setdefault("data-button-requires-password", rich_bool(action.requires_password))
    elif action.type in (kind.SwitchInline, kind.SwitchInlineSame):
        result.setdefault("data-button-query", rich_button_metadata(action.query))
        result.setdefault("data-button-same-peer", rich_bool(action.same_peer))
        if action.peer_types is not None:
            result.setdefault("data-button-peer-types", rich_button_peer_types(action.peer_types))
    elif action.type == kind.UserProfile:
        result.setdefault("data-button-user-id", number_to_string(action.user_id))
    elif action.type == kind.CopyText:
        result.setdefault("data-copy-text", rich_button_metadata(action.copy_text))
    if action.type == kind.Disabled:
        result.setdefault("aria-disabled", "true")
    return result


def rich_percent_value(value: int, total: int) -> str:
    return f"{value * 100.0 / total:.3f}%"


def rich_aspect_ratio_style(width: int, height: int) -> str:
    return "aspect-ratio: " + number_to_string(width) + " / " + number_to_string(height)


def rich_text_has_output(text: RichText) -> bool:
    kind = RichText.Type
    if text.type == kind.Empty:
        return False
    if text.type == kind.Plain:
        return bool(text.text)
    if text.type == kind.Concat:
        return any(rich_text_has_output(child) for child in text.children)
    return True


def rich_caption_has_output(caption: RichCaption) -> bool:
    return rich_text_has_output(caption.text) or rich_text_has_output(caption.credit)


_CHANNEL_SOURCES = {
    RichChannel.Source.ChatEmpty: ("chat_empty", "Chat"),
    RichChannel.Source.Chat: ("chat", "Chat"),
    RichChannel.Source.ChatForbidden: ("chat_forbidden", "Chat unavailable"),
    RichChannel.Source.Channel: ("channel", "Channel"),
    RichChannel.Source.ChannelForbidden: ("channel_forbidden", "Channel unavailable"),
    RichChannel.Source.Community: ("community", "Community"),
    RichChannel.Source.CommunityForbidden: ("community_forbidden", "Community unavailable"),
}


def rich_channel_source_name(source: RichChannel.Source) -> str:
    return _CHANNEL_SOURCES[source][0]


def rich_channel_status(channel: RichChannel) -> str:
    label = _CHANNEL_SOURCES[channel.source][1]
    if channel.username:
        return "@" + channel.username + ", " + label
    return label


_MAP_SOURCES = {
    RichMapPoint.Source.GeoPointEmpty: "geo_point_empty",
    RichMapPoint.Source.GeoPoint: "geo_point",
    RichMapPoint.Source.InputGeoPointEmpty: "input_geo_point_empty",
    RichMapPoint.Source.InputGeoPoint: "input_geo_point",
}


def rich_map_point_source_name(source: RichMapPoint.Source) -> str:
    return _MAP_SOURCES[source]


def rich_map_coordinates(point: RichMapPoint) -> tuple[str, str] | None:
    if point.source in (
        RichMapPoint.Source.GeoPointEmpty,
        RichMapPoint.Source.InputGeoPointEmpty,
    ):
        return None
    latitude, longitude = point.latitude, point.longitude
    if not math.isfinite(latitude) or not math.isfinite(longitude):
        return None
    if not (-90.0 <= latitude <= 90.0) or not (-180.0 <= longitude <= 180.0):
        return None
    return number_to_string(float(latitude)), number_to_string(float(longitude))


_CHILDREN_TARGETS = {
    RichText.Type.Concat,
    RichText.Type.Bold,
    RichText.Type.Italic,
    RichText.Type.Underline,
    RichText.Type.Strike,
    RichText.Type.Fixed,
    RichText.Type.Url,
    RichText.Type.Email,
    RichText.Type.Phone,
    RichText.Type.Subscript,
    RichText.Type.Superscript,
    RichText.Type.Marked,
    RichText.Type.Anchor,
    RichText.Type.Spoiler,
    RichText.Type.Mention,
    RichText.Type.Hashtag,
    RichText.Type.BotCommand,
    RichText.Type.Cashtag,
    RichText.Type.AutoUrl,
    RichText.Type.AutoEmail,
    RichText.Type.AutoPhone,
    RichText.Type.BankCard,
    RichText.Type.MentionName,
    RichText.Type.FormattedDate,
    RichText.Type.Diff,
}


def _append_plain(result: list[str], size: list[int], value: str) -> bool:
    value_size = utf8_size(value)
    if size[0] > RICH_TARGET_MAX_BYTES or value_size > RICH_TARGET_MAX_BYTES - size[0]:
        return False
    result.append(value)
    size[0] += value_size
    return True


def _append_plain_text(result: list[str], size: list[int], text: RichText) -> bool:
    kind = RichText.Type
    if text.type == kind.Empty:
        return True
    if text.type in (kind.Plain, kind.CustomEmoji):
        return _append_plain(result, size, text.text)
    if text.type in _CHILDREN_TARGETS:
        return all(_append_plain_text(result, size, child) for child in text.children)
    return False


def plain_target(text: RichText) -> str | None:
    result: list[str] = []
    size = [0]
    return "".join(result) if _append_plain_text(result, size, text) else None
