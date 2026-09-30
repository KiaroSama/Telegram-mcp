"""Rich message types (page blocks, rich text) and inline button actions.

Ported from Telegram Desktop v7.2.10 (Telegram/SourceFiles/export/data/export_data_types.h
RichText/RichBlock/RichMessage/InlineButtonAction and their *ToString helpers), GPL-3.0.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from .model import Document, Photo


class InlineButtonPeerType(Enum):
    SameBotPM = 0
    PM = 1
    Chat = 2
    Megagroup = 3
    Broadcast = 4
    BotPM = 5


_PEER_TYPE_NAMES = {
    InlineButtonPeerType.SameBotPM: "same_bot_pm",
    InlineButtonPeerType.PM: "pm",
    InlineButtonPeerType.Chat: "chat",
    InlineButtonPeerType.Megagroup: "megagroup",
    InlineButtonPeerType.Broadcast: "broadcast",
    InlineButtonPeerType.BotPM: "bot_pm",
}


def inline_button_peer_type_to_string(value: InlineButtonPeerType) -> str:
    return _PEER_TYPE_NAMES[value]


@dataclass
class InlineButtonAction:
    class Type(Enum):
        Url = 0
        Auth = 1
        WebView = 2
        Callback = 3
        CallbackWithPassword = 4
        Game = 5
        Buy = 6
        SwitchInline = 7
        SwitchInlineSame = 8
        UserProfile = 9
        CopyText = 10
        Disabled = 11

    url: str = ""
    forward_text: str | None = None
    callback_data: bytes = b""
    query: str = ""
    peer_types: list[InlineButtonPeerType] | None = None
    copy_text: str = ""
    user_id: int = 0
    button_id: int = 0
    type: InlineButtonAction.Type = Type.Url
    requires_password: bool = False
    same_peer: bool = False

    @staticmethod
    def type_to_string(action: InlineButtonAction) -> str:
        return _ACTION_TYPE_NAMES[action.type]


_A = InlineButtonAction.Type
_ACTION_TYPE_NAMES = {
    _A.Url: "url",
    _A.Auth: "auth",
    _A.WebView: "web_view",
    _A.Callback: "callback",
    _A.CallbackWithPassword: "callback_with_password",
    _A.Game: "game",
    _A.Buy: "buy",
    _A.SwitchInline: "switch_inline",
    _A.SwitchInlineSame: "switch_inline_same",
    _A.UserProfile: "user_profile",
    _A.CopyText: "copy_text",
    _A.Disabled: "disabled",
}


class RichButtonStyle(Enum):
    Default = 0
    Primary = 1
    Success = 2
    Danger = 3
    Link = 4


class RichButtonAlignment(Enum):
    Stretch = 0
    Left = 1
    Center = 2
    Right = 3


def rich_button_style_to_string(style: RichButtonStyle) -> str:
    return style.name.lower()


def rich_button_alignment_to_string(alignment: RichButtonAlignment) -> str:
    return alignment.name.lower()


@dataclass
class RichButtonPayload:
    action: InlineButtonAction = field(default_factory=InlineButtonAction)
    style: RichButtonStyle | None = None


@dataclass
class RichText:
    class Type(Enum):
        Empty = 0
        Plain = 1
        Concat = 2
        Bold = 3
        Italic = 4
        Underline = 5
        Strike = 6
        Fixed = 7
        Url = 8
        Email = 9
        Phone = 10
        Subscript = 11
        Superscript = 12
        Marked = 13
        Anchor = 14
        Math = 15
        CustomEmoji = 16
        Spoiler = 17
        Mention = 18
        Hashtag = 19
        BotCommand = 20
        Cashtag = 21
        AutoUrl = 22
        AutoEmail = 23
        AutoPhone = 24
        BankCard = 25
        MentionName = 26
        FormattedDate = 27
        InlineImage = 28
        Diff = 29
        Button = 30

    text: str = ""
    data: str = ""
    custom_emoji_data: str = ""
    children: list[RichText] = field(default_factory=list)
    old_children: list[RichText] = field(default_factory=list)
    button: RichButtonPayload | None = None
    id: int = 0
    date: int = 0
    width: int = 0
    height: int = 0
    type: RichText.Type = Type.Empty
    relative: bool = False
    short_time: bool = False
    long_time: bool = False
    short_date: bool = False
    long_date: bool = False
    day_of_week: bool = False
    unsupported: bool = False


@dataclass
class RichCaption:
    text: RichText = field(default_factory=RichText)
    credit: RichText = field(default_factory=RichText)


class RichTaskState(Enum):
    None_ = 0
    Unchecked = 1
    Checked = 2


class RichListKind(Enum):
    Bullet = 0
    Ordered = 1


class RichListItemContent(Enum):
    Text = 0
    Blocks = 1


class RichQuoteContent(Enum):
    Text = 0
    Blocks = 1


@dataclass
class RichOrderedList:
    type: str | None = None
    start: int | None = None
    reversed: bool = False


@dataclass
class RichListItem:
    text: RichText | None = None
    blocks: list[RichBlock] = field(default_factory=list)
    num: str | None = None
    type: str | None = None
    value: int | None = None
    task_state: RichTaskState = RichTaskState.None_
    content: RichListItemContent = RichListItemContent.Text


class RichTableAlignment(Enum):
    Left = 0
    Center = 1
    Right = 2


class RichTableVerticalAlignment(Enum):
    Top = 0
    Middle = 1
    Bottom = 2


@dataclass
class RichTableCell:
    text: RichText | None = None
    colspan: int | None = None
    rowspan: int | None = None
    alignment: RichTableAlignment = RichTableAlignment.Left
    vertical_alignment: RichTableVerticalAlignment = RichTableVerticalAlignment.Top
    header: bool = False


@dataclass
class RichTableRow:
    cells: list[RichTableCell] = field(default_factory=list)


@dataclass
class RichRelatedArticle:
    url: str = ""
    title: str | None = None
    description: str | None = None
    author: str | None = None
    photo_id: int | None = None
    published_date: int | None = None
    webpage_id: int = 0


@dataclass
class RichChannel:
    class Source(Enum):
        ChatEmpty = 0
        Chat = 1
        ChatForbidden = 2
        Channel = 3
        ChannelForbidden = 4
        Community = 5
        CommunityForbidden = 6

    title: str | None = None
    username: str | None = None
    access_hash: int | None = None
    id: int = 0
    source: RichChannel.Source = Source.ChatEmpty
    broadcast: bool = False
    megagroup: bool = False
    monoforum: bool = False


@dataclass
class RichMapPoint:
    class Source(Enum):
        GeoPointEmpty = 0
        GeoPoint = 1
        InputGeoPointEmpty = 2
        InputGeoPoint = 3

    access_hash: int | None = None
    accuracy_radius: int | None = None
    latitude: float = 0.0
    longitude: float = 0.0
    source: RichMapPoint.Source = Source.GeoPointEmpty


@dataclass
class RichBlock:
    class Kind(Enum):
        Unsupported = 0
        Heading = 1
        Paragraph = 2
        Footer = 3
        Thinking = 4
        AuthorDate = 5
        Code = 6
        Divider = 7
        Anchor = 8
        List = 9
        Quote = 10
        Photo = 11
        Video = 12
        Cover = 13
        Embed = 14
        EmbedPost = 15
        Collage = 16
        Slideshow = 17
        Channel = 18
        Audio = 19
        File = 20
        Math = 21
        Table = 22
        Details = 23
        RelatedArticles = 24
        Map = 25
        InputMap = 26
        ButtonRow = 27
        Unknown = 28

    text: RichText = field(default_factory=RichText)
    quote_caption: RichText = field(default_factory=RichText)
    caption: RichCaption = field(default_factory=RichCaption)
    ordered_list: RichOrderedList = field(default_factory=RichOrderedList)
    channel: RichChannel = field(default_factory=RichChannel)
    map_point: RichMapPoint = field(default_factory=RichMapPoint)
    language: str = ""
    formula: str = ""
    name: str = ""
    url: str = ""
    author: str = ""
    optional_url: str | None = None
    html: str | None = None
    blocks: list[RichBlock] = field(default_factory=list)
    list_items: list[RichListItem] = field(default_factory=list)
    table_rows: list[RichTableRow] = field(default_factory=list)
    related_articles: list[RichRelatedArticle] = field(default_factory=list)
    buttons: list[RichText] = field(default_factory=list)
    optional_webpage_id: int | None = None
    poster_photo_id: int | None = None
    width: int | None = None
    height: int | None = None
    photo_id: int = 0
    document_id: int = 0
    webpage_id: int = 0
    author_photo_id: int = 0
    date: int = 0
    heading_level: int = 0
    zoom: int = 0
    map_width: int = 0
    map_height: int = 0
    kind: RichBlock.Kind = Kind.Unknown
    list_kind: RichListKind = RichListKind.Bullet
    quote_content: RichQuoteContent = RichQuoteContent.Text
    button_alignment: RichButtonAlignment = RichButtonAlignment.Stretch
    unsupported: bool = False
    full_width: bool = False
    allow_scrolling: bool = False
    autoplay: bool = False
    loop: bool = False
    spoiler: bool = False
    open: bool = False
    bordered: bool = False
    striped: bool = False
    compact: bool = False
    pullquote: bool = False


@dataclass
class RichMessage:
    blocks: list[RichBlock] = field(default_factory=list)
    photos: dict[int, Photo] = field(default_factory=dict)
    documents: dict[int, Document] = field(default_factory=dict)
    rtl: bool = False
    part: bool = False
