"""Users, chats and peers.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.{h,cpp}
ContactInfo/User/Chat/Peer, ParseUser/ParseChat/ParsePeersLists/EmptyPeer), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Union

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .model import (
    REPLIES_USER_ID,
    VERIFY_CODES_USER_ID,
    peer_color_index,
    peer_from_channel,
    peer_from_chat,
    peer_from_user,
    peer_is_user,
    peer_to_chat,
    peer_to_user,
)


@dataclass
class ContactInfo:
    user_id: int = 0
    first_name: str = ""
    last_name: str = ""
    phone_number: str = ""
    date: int = 0
    color_index: int = 0

    def name(self) -> str:
        if not self.first_name:
            return self.last_name
        return f"{self.first_name} {self.last_name}" if self.last_name else self.first_name


def _custom_color_index(color: Any, fallback: int) -> int:
    if isinstance(color, tl.PeerColor) and color.color is not None:
        return int(color.color)
    return fallback


def parse_contact_info(data: Any) -> ContactInfo:
    result = ContactInfo()
    if isinstance(data, tl.User):
        result.user_id = data.id
        result.color_index = _custom_color_index(data.color, peer_color_index(data.id))
        result.first_name = data.first_name or ""
        result.last_name = data.last_name or ""
        result.phone_number = data.phone or ""
    elif isinstance(data, tl.UserEmpty):
        result.user_id = data.id
        result.color_index = peer_color_index(data.id)
    return result


def contact_color_index(data: ContactInfo) -> int:
    return data.color_index


@dataclass
class User:
    bare_id: int = 0
    info: ContactInfo = field(default_factory=ContactInfo)
    username: str = ""
    color_index: int = 0
    is_bot: bool = False
    is_self: bool = False
    is_replies: bool = False
    is_verify_codes: bool = False
    input: Any = field(default_factory=tl.InputUserEmpty)

    def id(self) -> int:
        return peer_from_user(self.bare_id)

    def name(self) -> str:
        return self.info.name()


def parse_user(data: Any) -> User:
    result = User(info=parse_contact_info(data))
    if isinstance(data, tl.User):
        result.bare_id = data.id
        result.color_index = _custom_color_index(data.color, peer_color_index(data.id))
        result.username = data.username or ""
        result.is_bot = data.bot_info_version is not None
        if data.is_self:
            result.is_self = True
        elif data.id == REPLIES_USER_ID:
            result.is_replies = True
        elif data.id == VERIFY_CODES_USER_ID:
            result.is_verify_codes = True
        result.input = tl.InputUser(user_id=data.id, access_hash=data.access_hash or 0)
    elif isinstance(data, tl.UserEmpty):
        result.input = tl.InputUser(user_id=data.id, access_hash=0)
    return result


def parse_users_list(data: list[Any]) -> dict[int, User]:
    result: dict[int, User] = {}
    for user in data:
        parsed = parse_user(user)
        result.setdefault(parsed.info.user_id, parsed)
    return dict(sorted(result.items()))


@dataclass
class Chat:
    bare_id: int = 0
    migrated_to_channel_id: int = 0
    title: str = ""
    username: str = ""
    color_index: int = 0
    is_monoforum: bool = False
    is_broadcast: bool = False
    is_supergroup: bool = False
    is_monoforum_admin: bool = False
    has_monoforum_admin_rights: bool = False
    is_monoforum_of_public_broadcast: bool = False
    monoforum_link_id: int = 0
    input: Any = field(default_factory=tl.InputPeerEmpty)
    monoforum_broadcast_input: Any = field(default_factory=tl.InputPeerEmpty)

    def id(self) -> int:
        if self.is_broadcast or self.is_supergroup:
            return peer_from_channel(self.bare_id)
        return peer_from_chat(self.bare_id)


def parse_chat(data: Any) -> Chat:
    result = Chat()
    if isinstance(data, tl.Chat):
        result.bare_id = data.id
        result.title = data.title or ""
        result.input = tl.InputPeerChat(chat_id=result.bare_id)
        if isinstance(data.migrated_to, tl.InputChannel):
            result.migrated_to_channel_id = data.migrated_to.channel_id
    elif isinstance(data, tl.ChatEmpty):
        result.bare_id = data.id
        result.input = tl.InputPeerChat(chat_id=result.bare_id)
    elif isinstance(data, tl.ChatForbidden):
        result.bare_id = data.id
        result.title = data.title or ""
        result.input = tl.InputPeerChat(chat_id=result.bare_id)
    elif isinstance(data, tl.Channel):
        result.bare_id = data.id
        result.color_index = _custom_color_index(data.color, peer_color_index(data.id))
        result.is_monoforum = bool(data.monoforum)
        result.is_broadcast = bool(data.broadcast)
        result.is_supergroup = bool(data.megagroup)
        result.has_monoforum_admin_rights = bool(data.broadcast) and bool(
            data.creator
            or (data.admin_rights is not None and data.admin_rights.manage_direct_messages)
        )
        result.monoforum_link_id = data.linked_monoforum_id or 0
        result.title = data.title or ""
        result.username = data.username or ""
        result.input = tl.InputPeerChannel(
            channel_id=result.bare_id, access_hash=data.access_hash or 0
        )
    elif isinstance(data, tl.ChannelForbidden):
        result.bare_id = data.id
        result.is_broadcast = bool(data.broadcast)
        result.is_supergroup = bool(data.megagroup)
        result.is_monoforum = bool(data.monoforum)
        result.title = data.title or ""
        result.input = tl.InputPeerChannel(channel_id=result.bare_id, access_hash=data.access_hash)
    elif isinstance(data, (tl.Community, tl.CommunityForbidden)):
        result.bare_id = data.id
        result.title = data.title or ""
        result.input = tl.InputPeerChannel(
            channel_id=result.bare_id, access_hash=data.access_hash or 0
        )
    return result


@dataclass
class Peer:
    data: Union[User, Chat] = field(default_factory=User)

    def user(self) -> User | None:
        return self.data if isinstance(self.data, User) else None

    def chat(self) -> Chat | None:
        return self.data if isinstance(self.data, Chat) else None

    def id(self) -> int:
        if isinstance(self.data, User):
            return peer_from_user(self.data.info.user_id)
        return self.data.id()

    def name(self) -> str:
        if isinstance(self.data, User):
            return self.data.name()
        return self.data.title

    def input(self) -> Any:
        if isinstance(self.data, User):
            source = self.data.input
            if isinstance(source, tl.InputUser):
                return tl.InputPeerUser(user_id=source.user_id, access_hash=source.access_hash)
            return tl.InputPeerEmpty()
        return self.data.input

    def color_index(self) -> int:
        return self.data.color_index


def parse_peers_lists(users: list[Any], chats: list[Any]) -> dict[int, Peer]:
    result: dict[int, Peer] = {}
    for user in users:
        parsed_user = parse_user(user)
        result.setdefault(peer_from_user(parsed_user.info.user_id), Peer(parsed_user))
    for chat in chats:
        parsed_chat = parse_chat(chat)
        result.setdefault(parsed_chat.id(), Peer(parsed_chat))
    for parsed in result.values():
        chat_data = parsed.chat()
        if chat_data is None or not chat_data.is_monoforum:
            continue
        linked = result.get(peer_from_channel(chat_data.monoforum_link_id))
        linked_chat = linked.chat() if linked else None
        if linked_chat is not None:
            chat_data.is_monoforum_admin = linked_chat.has_monoforum_admin_rights
            chat_data.is_monoforum_of_public_broadcast = bool(linked_chat.username)
    return dict(sorted(result.items()))


def empty_user(user_id: int) -> User:
    return parse_user(tl.UserEmpty(id=user_id))


def empty_chat(chat_id: int) -> Chat:
    return parse_chat(tl.ChatEmpty(id=chat_id))


def empty_peer(peer_id: int) -> Peer:
    if peer_is_user(peer_id):
        return Peer(empty_user(peer_to_user(peer_id)))
    # EmptyPeer maps a channel through peerToChat, which yields 0 for it.
    return Peer(empty_chat(peer_to_chat(peer_id)))
