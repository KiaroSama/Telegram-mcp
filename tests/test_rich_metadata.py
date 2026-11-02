"""Rich post metadata exposes destinations without flattening them away."""

from telethon import types
from telegram_mcp.tools.messages_view import message_to_dict


def test_compact_rich_post_reports_nested_link_and_media_reference():
    rich = types.RichMessage(
        blocks=[
            types.PageBlockParagraph(
                text=types.TextBold(
                    types.TextUrl(types.TextPlain("Visit"), "https://example.org/report", 0)
                )
            ),
            types.PageBlockPhoto(
                photo_id=123, caption=types.PageCaption(types.TextEmpty(), types.TextEmpty())
            ),
        ],
        photos=[],
        documents=[],
    )
    message = types.Message(id=7, peer_id=types.PeerUser(8), message="", rich_message=rich)
    result = message_to_dict(message)
    assert "https://example.org/report" in result["link_urls"]
    assert result["rich_media"] == [{"kind": "photo", "file_id": "123", "available": False}]


def test_rich_contacts_embeds_and_cycles_are_bounded():
    rich = types.RichMessage(
        blocks=[
            types.PageBlockParagraph(types.TextEmail(types.TextPlain("Mail"), "demo@example.org")),
            types.PageBlockParagraph(types.TextPhone(types.TextPlain("Call"), "+12025550123")),
            types.PageBlockEmbed(
                caption=types.PageCaption(types.TextEmpty(), types.TextEmpty()),
                url="https://example.org/embed",
            ),
        ],
        photos=[],
        documents=[],
    )
    # Malformed in-memory graphs cannot keep the renderer alive forever.
    rich.blocks.append(rich)
    message = types.Message(id=8, peer_id=types.PeerUser(8), message="", rich_message=rich)
    result = message_to_dict(message)
    assert set(result["link_urls"]) == {
        "mailto:demo@example.org",
        "tel:+12025550123",
        "https://example.org/embed",
    }
