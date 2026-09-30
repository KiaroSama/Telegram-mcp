"""Export settings.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/export_settings.{h,cpp},
export_controller.cpp NormalizeSettings), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from enum import Enum, IntFlag

from telethon.tl import types as tl  # type: ignore[import-untyped]

# export_settings.cpp kMaxFileSize.
MAX_FILE_SIZE = 4000 * 1024 * 1024


class Format(Enum):
    """Output::Format from output/export_output_abstract.h."""

    Html = 0
    Json = 1
    HtmlAndJson = 2


@dataclass
class MediaSettings:
    class Type(IntFlag):
        Photo = 0x01
        Video = 0x02
        VoiceMessage = 0x04
        VideoMessage = 0x08
        Sticker = 0x10
        GIF = 0x20
        File = 0x40

        MediaMask = Photo | Video | VoiceMessage | VideoMessage
        AllMask = MediaMask | Sticker | GIF | File

    types: MediaSettings.Type = field(default_factory=lambda: MediaSettings.default_types())
    size_limit: int = 8 * 1024 * 1024

    @staticmethod
    def default_types() -> MediaSettings.Type:
        return MediaSettings.Type.Photo

    def validate(self) -> bool:
        all_mask = MediaSettings.Type.AllMask
        if (self.types | all_mask) != all_mask:
            return False
        return 0 <= self.size_limit <= MAX_FILE_SIZE


@dataclass
class Settings:
    class Type(IntFlag):
        PersonalInfo = 0x001
        Userpics = 0x002
        Contacts = 0x004
        Sessions = 0x008
        OtherData = 0x010
        PersonalChats = 0x020
        BotChats = 0x040
        PrivateGroups = 0x080
        PublicGroups = 0x100
        PrivateChannels = 0x200
        PublicChannels = 0x400
        Stories = 0x800
        ProfileMusic = 0x1000

        GroupsMask = PrivateGroups | PublicGroups
        ChannelsMask = PrivateChannels | PublicChannels
        GroupsChannelsMask = GroupsMask | ChannelsMask
        NonChannelChatsMask = PersonalChats | BotChats | PrivateGroups
        AnyChatsMask = PersonalChats | BotChats | GroupsChannelsMask
        NonChatsMask = PersonalInfo | Userpics | Contacts | Stories | ProfileMusic | Sessions
        AllMask = NonChatsMask | OtherData | AnyChatsMask

    path: str = ""
    force_sub_path: bool = False
    format: Format = Format.Html

    types: Settings.Type = field(default_factory=lambda: Settings.default_types())
    full_chats: Settings.Type = field(default_factory=lambda: Settings.default_full_chats())
    media: MediaSettings = field(default_factory=MediaSettings)

    single_peer: tl.TypeInputPeer = field(default_factory=tl.InputPeerEmpty)
    single_peer_from: int = 0
    single_peer_till: int = 0

    single_topic_root_id: int = 0
    single_topic_peer_id: int = 0
    single_topic_title: str = ""

    available_at: int = 0

    def only_single_peer(self) -> bool:
        return not isinstance(self.single_peer, tl.InputPeerEmpty)

    def only_single_topic(self) -> bool:
        return self.only_single_peer() and self.single_topic_root_id != 0

    @staticmethod
    def default_types() -> Settings.Type:
        t = Settings.Type
        return (
            t.PersonalInfo
            | t.Userpics
            | t.Contacts
            | t.Stories
            | t.ProfileMusic
            | t.PersonalChats
            | t.PrivateGroups
        )

    @staticmethod
    def default_full_chats() -> Settings.Type:
        return Settings.Type.PersonalChats | Settings.Type.BotChats

    def validate(self) -> bool:
        t = Settings.Type
        must_be_full = t.PersonalChats | t.BotChats
        must_not_be_full = t.PublicGroups | t.PublicChannels
        if (self.types | t.AllMask) != t.AllMask:
            return False
        if (self.full_chats | t.AllMask) != t.AllMask:
            return False
        if (self.full_chats & must_be_full) != must_be_full:
            return False
        if self.full_chats & must_not_be_full:
            return False
        if self.format not in (Format.Html, Format.Json):
            return False
        if not self.media.validate():
            return False
        return not (0 < self.single_peer_till <= self.single_peer_from)


@dataclass
class Environment:
    internal_links_domain: str = ""
    about_telegram: str = ""
    about_contacts: str = ""
    about_frequent: str = ""
    about_sessions: str = ""
    about_web_sessions: str = ""
    about_chats: str = ""
    about_left_chats: str = ""


def normalize_settings(settings: Settings) -> Settings:
    """export_controller.cpp NormalizeSettings: a single-peer export covers every chat type."""

    if not settings.only_single_peer():
        return replace(settings)
    return replace(
        settings,
        types=Settings.Type.AnyChatsMask,
        full_chats=Settings.Type.AnyChatsMask,
    )
