"""Dialogs: DialogInfo/DialogsInfo and the single-peer dialog list rules.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.{h,cpp}
DialogInfo, DialogsInfo, DialogTypeFromChat/User, DialogInfoFromUser/Chat, ParseDialogsInfo for
a single peer, AddMigrateFromSlice, FinalizeDialogsInfo), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any

from telethon.tl import types as tl  # type: ignore[import-untyped]

from .model import peer_from_channel, peer_from_chat, peer_from_user
from .model_format import number_to_string
from .model_peers import Chat, Peer, User, parse_chat, parse_user
from .settings import Settings


@dataclass
class DialogInfo:
    class Type(Enum):
        Unknown = 0
        Self = 1
        Replies = 2
        VerifyCodes = 3
        Personal = 4
        Bot = 5
        PrivateGroup = 6
        PrivateSupergroup = 7
        PublicSupergroup = 8
        PrivateChannel = 9
        PublicChannel = 10

    type: DialogInfo.Type = Type.Unknown
    name: str = ""
    last_name: str = ""
    input: Any = field(default_factory=tl.InputPeerEmpty)
    top_message_id: int = 0
    top_message_date: int = 0
    peer_id: int = 0
    color_index: int = 0
    migrated_from_input: Any = field(default_factory=tl.InputPeerEmpty)
    migrated_to_channel_id: int = 0
    monoforum_broadcast_input: Any = field(default_factory=tl.InputPeerEmpty)
    # User messages splits which contained that dialog.
    splits: list[int] = field(default_factory=list)
    # Filled after the whole dialogs list is accumulated.
    only_my_messages: bool = False
    is_left_channel: bool = False
    is_monoforum: bool = False
    relative_path: str = ""
    # Filled when requesting dialog messages.
    messages_count_per_split: list[int] = field(default_factory=list)


@dataclass
class DialogsInfo:
    chats: list[DialogInfo] = field(default_factory=list)
    left: list[DialogInfo] = field(default_factory=list)

    def item(self, index: int) -> DialogInfo | None:
        if index < 0:
            return None
        if index < len(self.chats):
            return self.chats[index]
        if index - len(self.chats) < len(self.left):
            return self.left[index - len(self.chats)]
        return None


def dialog_type_from_chat(chat: Chat) -> DialogInfo.Type:
    kind = DialogInfo.Type
    if chat.is_monoforum and not chat.is_monoforum_admin:
        return kind.Personal
    if chat.is_monoforum_admin and chat.is_monoforum_of_public_broadcast:
        return kind.PublicSupergroup
    if chat.is_monoforum_admin:
        return kind.PrivateSupergroup
    if not chat.username:
        if chat.is_broadcast:
            return kind.PrivateChannel
        return kind.PrivateSupergroup if chat.is_supergroup else kind.PrivateGroup
    return kind.PublicChannel if chat.is_broadcast else kind.PublicSupergroup


def dialog_type_from_user(user: User) -> DialogInfo.Type:
    kind = DialogInfo.Type
    if user.is_self:
        return kind.Self
    if user.is_replies:
        return kind.Replies
    if user.is_verify_codes:
        return kind.VerifyCodes
    return kind.Bot if user.is_bot else kind.Personal


def dialog_info_from_user(data: User) -> DialogInfo:
    return DialogInfo(
        input=Peer(data).input(),
        name=data.info.first_name,
        last_name=data.info.last_name,
        peer_id=data.id(),
        type=dialog_type_from_user(data),
    )


def dialog_info_from_chat(data: Chat) -> DialogInfo:
    result = DialogInfo(
        input=data.input,
        name=data.title,
        peer_id=data.id(),
        type=dialog_type_from_chat(data),
        migrated_to_channel_id=data.migrated_to_channel_id,
        is_monoforum=data.is_monoforum,
    )
    if data.is_monoforum_admin:
        result.monoforum_broadcast_input = data.monoforum_broadcast_input
    return result


def parse_dialogs_info_users(single_peer: Any, users: list[Any]) -> DialogsInfo:
    """ParseDialogsInfo(singlePeer, users): the answer of users.getUsers for one peer."""
    if isinstance(single_peer, tl.InputPeerUser):
        single_id = peer_from_user(single_peer.user_id)
    elif isinstance(single_peer, tl.InputPeerSelf):
        single_id = 0
    else:
        raise ValueError("Single peer type in ParseDialogsInfo(users).")
    result = DialogsInfo()
    for single in users:
        user_id = peer_from_user(single.id)
        is_self = isinstance(single, tl.User) and bool(single.is_self)
        if user_id != single_id and (single_id != 0 or not is_self):
            continue
        result.chats.append(dialog_info_from_user(parse_user(single)))
    return result


def parse_dialogs_info_chats(single_peer: Any, chats: list[Any]) -> DialogsInfo:
    """ParseDialogsInfo(singlePeer, chats): the answer of getChats/getChannels for one peer."""
    if isinstance(single_peer, tl.InputPeerChat):
        single_id = peer_from_chat(single_peer.chat_id)
    elif isinstance(single_peer, tl.InputPeerChannel):
        single_id = peer_from_channel(single_peer.channel_id)
    else:
        raise ValueError("Single peer type in ParseDialogsInfo(chats).")
    channel_types = (tl.Channel, tl.ChannelForbidden, tl.Community, tl.CommunityForbidden)
    result = DialogsInfo()
    for single in chats:
        is_channel = isinstance(single, channel_types)
        peer_id = peer_from_channel(single.id) if is_channel else peer_from_chat(single.id)
        if peer_id != single_id:
            continue
        info = dialog_info_from_chat(parse_chat(single))
        info.is_left_channel = False
        result.chats.append(info)
    return result


def add_migrate_from_slice(
    to: DialogInfo, source: DialogInfo, split_index: int, splits_count: int
) -> bool:
    if not (
        isinstance(to.migrated_from_input, tl.InputPeerEmpty)
        or (
            isinstance(to.migrated_from_input, tl.InputPeerChat)
            and peer_from_chat(to.migrated_from_input.chat_id) == source.peer_id
        )
    ):
        return False
    for split, count in zip(source.splits, source.messages_count_per_split):
        to.splits.append(split - splits_count)
        to.messages_count_per_split.append(count)
    to.migrated_from_input = source.input
    to.splits.append(split_index - splits_count)
    to.messages_count_per_split.append(0)
    return True


_SETTING_FOR_DIALOG = {
    DialogInfo.Type.Self: Settings.Type.PersonalChats,
    DialogInfo.Type.Personal: Settings.Type.PersonalChats,
    DialogInfo.Type.Bot: Settings.Type.BotChats,
    DialogInfo.Type.PrivateGroup: Settings.Type.PrivateGroups,
    DialogInfo.Type.PrivateSupergroup: Settings.Type.PrivateGroups,
    DialogInfo.Type.PrivateChannel: Settings.Type.PrivateChannels,
    DialogInfo.Type.PublicSupergroup: Settings.Type.PublicGroups,
    DialogInfo.Type.PublicChannel: Settings.Type.PublicChannels,
}


def settings_from_dialogs_type(dialog_type: DialogInfo.Type) -> Settings.Type:
    """ApiWrap SettingsFromDialogsType (0 for Unknown, Replies and VerifyCodes)."""
    return _SETTING_FOR_DIALOG.get(dialog_type, Settings.Type(0))


def finalize_dialogs_info(info: DialogsInfo, settings: Settings) -> None:
    full_count = len(info.chats) + len(info.left)
    digits = len(number_to_string(full_count - 1))
    index = 0
    for dialog in info.chats:
        index += 1
        number = number_to_string(index, digits, "0")
        dialog.relative_path = "" if settings.only_single_peer() else f"chats/chat_{number}/"
        if dialog.type not in _SETTING_FOR_DIALOG:
            raise ValueError("Type in ApiWrap::onlyMyMessages.")
        setting = _SETTING_FOR_DIALOG[dialog.type]
        dialog.only_my_messages = dialog.type != DialogInfo.Type.Personal and (
            (settings.full_chats & setting) != setting
        )
        dialog.splits.sort()
    for dialog in info.left:
        index += 1
        number = number_to_string(index, digits, "0")
        dialog.relative_path = f"chats/chat_{number}/"
        dialog.only_my_messages = True
