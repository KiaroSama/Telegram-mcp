"""Read-only discovery tools never join or mutate a Telegram chat."""

import importlib
import json
from types import SimpleNamespace

import pytest
from telethon import functions, types


@pytest.mark.asyncio
async def test_folder_preview_and_topic_icons_use_read_requests(wire_client):
    folders = importlib.import_module("telegram_mcp.tools.folder_invites")
    icons = importlib.import_module("telegram_mcp.tools.topic_icons")

    class Client:
        def __init__(self):
            self.requests = []

        async def __call__(self, request):
            self.requests.append(request)
            if isinstance(request, functions.chatlists.CheckChatlistInviteRequest):
                assert request.slug == "Fixture_slug"
                return types.chatlists.ChatlistInvite(
                    title=types.TextWithEntities("Work", []),
                    peers=[types.PeerChat(7)],
                    chats=[
                        types.Chat(
                            id=7,
                            title="Group",
                            photo=types.ChatPhotoEmpty(),
                            participants_count=2,
                            date=None,
                            version=1,
                        )
                    ],
                    users=[],
                )
            assert isinstance(request, functions.messages.GetStickerSetRequest)
            assert isinstance(request.stickerset, types.InputStickerSetEmojiDefaultTopicIcons)
            return SimpleNamespace(
                documents=[
                    SimpleNamespace(
                        id=123,
                        attributes=[
                            types.DocumentAttributeCustomEmoji(
                                alt="🎉", stickerset=types.InputStickerSetEmpty()
                            )
                        ],
                    )
                ]
            )

    client = wire_client(folders, Client())
    preview = json.loads(
        await folders.get_chatlist_invite("https://t.me/addlist/Fixture_slug", account="default")
    )
    assert preview["results"][0]["chat_id"] == -7
    assert preview["joined"] is False
    before = len(client.requests)
    refused = await folders.get_chatlist_invite(
        "https://evil.example/addlist/Fixture_slug", account="default"
    )
    assert "invalid" in refused.lower()
    assert len(client.requests) == before
    wire_client(icons, client)
    result = json.loads(await icons.list_forum_topic_icons(account="default"))
    assert result["results"] == [{"icon_emoji_id": "123", "emoji": "🎉"}]
