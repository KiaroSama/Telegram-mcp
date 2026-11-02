"""Fetch story media on the existing client, never send a read/view request."""

from telethon import functions, types


async def fetch_story_media(client, peer, story_id):
    if not isinstance(story_id, int) or isinstance(story_id, bool) or story_id <= 0:
        raise ValueError("story_id must be a positive integer.")
    result = await client(functions.stories.GetStoriesByIDRequest(peer=peer, id=[story_id]))
    story = next(
        (
            item
            for item in getattr(result, "stories", []) or []
            if getattr(item, "id", None) == story_id
        ),
        None,
    )
    if story is None or not isinstance(story, types.StoryItem):
        raise ValueError("Story unavailable: expired, deleted or inaccessible to this account.")
    media = getattr(story, "media", None)
    if not isinstance(media, (types.MessageMediaPhoto, types.MessageMediaDocument)):
        raise ValueError("Story has no downloadable photo/video (live stories are unsupported).")
    message = types.Message(id=story_id, peer_id=None, message="", media=media)
    if message.photo is None and message.video is None:
        raise ValueError("Story has no downloadable photo/video.")
    return message
