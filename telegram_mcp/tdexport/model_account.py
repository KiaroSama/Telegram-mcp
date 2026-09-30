"""Account-wide export sections (profile, userpics, stories, contacts, sessions).

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.h),
GPL-3.0. Only the data shapes: a single-chat export never requests these sections, so their
Parse* functions (ParseUserpicsSlice, ParseStoriesSlice, ParsePersonalInfo, ParseContactsList,
AppendTopPeers, ParseSessionsList, ParseWebSessionsList) are not ported.
"""

from __future__ import annotations

import builtins
from dataclasses import dataclass, field
from typing import List

from .model import File, Image, Photo, TextPart
from .model_media import Media
from .model_message import Message
from .model_peers import ContactInfo, Peer, User


@dataclass
class UserpicsInfo:
    count: int = 0


@dataclass
class StoriesInfo:
    count: int = 0


@dataclass
class ProfileMusicInfo:
    count: int = 0


@dataclass
class UserpicsSlice:
    list: List[Photo] = field(default_factory=list)


@dataclass
class PersonalInfo:
    user: User = field(default_factory=User)
    bio: str = ""


@dataclass
class TopPeer:
    peer: Peer = field(default_factory=Peer)
    rating: float = 0.0


@dataclass
class ContactsList:
    list: List[ContactInfo] = field(default_factory=list)
    correspondents: List[TopPeer] = field(default_factory=builtins.list)
    inline_bots: List[TopPeer] = field(default_factory=builtins.list)
    phone_calls: List[TopPeer] = field(default_factory=builtins.list)


@dataclass
class Session:
    application_id: int = 0
    platform: str = ""
    device_model: str = ""
    system_version: str = ""
    application_name: str = ""
    application_version: str = ""
    created: int = 0
    last_active: int = 0
    ip: str = ""
    country: str = ""
    region: str = ""


@dataclass
class WebSession:
    bot_username: str = ""
    domain: str = ""
    browser: str = ""
    platform: str = ""
    created: int = 0
    last_active: int = 0
    ip: str = ""
    region: str = ""


@dataclass
class SessionsList:
    list: List[Session] = field(default_factory=list)
    web_list: List[WebSession] = field(default_factory=builtins.list)


@dataclass
class Story:
    id: int = 0
    date: int = 0
    expires: int = 0
    media: Media = field(default_factory=Media)
    pinned: bool = False
    caption: list[TextPart] = field(default_factory=list)

    def file(self) -> File:
        return self.media.file()

    def thumb(self) -> Image:
        return self.media.thumb()


@dataclass
class StoriesSlice:
    list: List[Story] = field(default_factory=list)
    last_id: int = 0
    skipped: int = 0


@dataclass
class ProfileMusicSlice:
    list: List[Message] = field(default_factory=list)
    skipped: int = 0
