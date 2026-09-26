"""A chat's notifications: sound on or off, the tone it plays, and the saved sounds.

Every change reads the chat's current settings and writes them back with ONE field
changed. Telegram does not promise to keep a field that a partial update leaves out,
so turning the sound off could otherwise unmute a chat, and choosing a tone could
reset a mute. `mute_chat` and `unmute_chat` (chat_state.py) write through the same
helper.
"""

from pathlib import PurePath

from telegram_mcp.runtime import *

# The formats Telegram accepts for a notification sound (core.telegram.org/api/ringtones).
_SOUND_TYPES = {".mp3": "audio/mpeg", ".ogg": "audio/ogg", ".opus": "audio/ogg"}


async def _update_notify(cl, entity, **changes):
    """Write ``changes`` over the chat's current notification settings."""
    notify_peer = types.InputNotifyPeer(peer=utils.get_input_peer(entity))
    current = await cl(functions.account.GetNotifySettingsRequest(peer=notify_peer))
    fields = {
        "show_previews": current.show_previews,
        "silent": current.silent,
        "mute_until": current.mute_until,
        "sound": current.other_sound,
        "stories_muted": current.stories_muted,
        "stories_hide_sender": current.stories_hide_sender,
        "stories_sound": current.stories_other_sound,
    }
    fields.update(changes)
    return await cl(
        functions.account.UpdateNotifySettingsRequest(
            peer=notify_peer, settings=types.InputPeerNotifySettings(**fields)
        )
    )


def _sound_title(doc) -> str:
    for attr in doc.attributes:
        if isinstance(attr, types.DocumentAttributeFilename):
            return attr.file_name
    return str(doc.id)


def _sound_seconds(doc):
    for attr in doc.attributes:
        if isinstance(attr, types.DocumentAttributeAudio):
            return attr.duration
    return None


async def _saved_sounds(cl) -> list:
    result = await cl(functions.account.GetSavedRingtonesRequest(hash=0))
    return list(getattr(result, "ringtones", []))


def _find_sound(sounds, wanted: str):
    """``(document, None)`` by id, title, or title without its extension; else a refusal."""
    key = wanted.strip().casefold()
    for doc in sounds:
        title = _sound_title(doc).casefold()
        if key in (str(doc.id), title, PurePath(title).stem):
            return doc, None
    known = ", ".join(f"{_sound_title(d)} ({d.id})" for d in sounds) or "none saved"
    return None, f"No saved sound {wanted!r}. Saved sounds: {known}."


def _input_document(doc):
    return types.InputDocument(
        id=doc.id, access_hash=doc.access_hash, file_reference=doc.file_reference
    )


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Chat Sound Disabled",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat")
async def set_chat_sound_disabled(
    chat: Union[int, str], disabled: bool, account: str = None
) -> str:
    """
    Let a chat's notifications arrive silently (disabled=True), or with sound again.

    This is not a mute: notifications still arrive. Other settings stay as they are.
    """
    try:
        cl = get_client(account)
        ok = await _update_notify(cl, await resolve_entity(chat, cl), silent=bool(disabled))
        if ok is False:
            return "Telegram did not change the sound; nothing changed."
        return (
            "Notifications now arrive without sound." if disabled else "Notification sound is on."
        )
    except Exception as e:
        return log_and_format_error("set_chat_sound_disabled", e, chat=chat)


async def _upload_sound(cl, file_path, ctx):
    """``(sound document, None)`` after uploading and saving the file, or ``(None, refusal)``."""
    suffix = PurePath(file_path).suffix.lower()
    mime = _SOUND_TYPES.get(suffix)
    if mime is None:
        return None, "A notification sound must be MP3 or OGG OPUS (.mp3, .ogg, .opus)."
    async with _open_verified_source(raw_path=file_path, ctx=ctx, tool_name="set_chat_tone") as (
        source,
        path_error,
    ):
        if path_error:
            return None, path_error
        uploaded = await cl.upload_file(source.handle)
        doc = await cl(
            functions.account.UploadRingtoneRequest(
                file=uploaded, file_name=PurePath(str(source.path)).name, mime_type=mime
            )
        )
    saved = await cl(functions.account.SaveRingtoneRequest(id=_input_document(doc), unsave=False))
    # Telegram may convert the upload and answer with the document to use instead.
    return getattr(saved, "document", None) or doc, None


@mcp.tool(
    annotations=ToolAnnotations(
        title="Set Chat Tone",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
@validate_id("chat")
async def set_chat_tone(
    chat: Union[int, str],
    tone: Optional[str] = None,
    file_path: Optional[str] = None,
    ctx: Optional[Context] = None,
    account: str = None,
) -> str:
    """
    Choose the sound a chat's notifications play. Give exactly one of:

    Args:
        tone: "default", "none", or a saved sound's id or title (list_saved_sounds).
        file_path: An MP3 or OGG OPUS file; it is added to the saved sounds, then used.
            Telegram limits a sound's length, size and the number saved.
    """
    if (tone is None) == (file_path is None):
        return "Give exactly one of tone (default, none, or a saved sound) and file_path."
    try:
        cl = get_client(account)
        if file_path is not None:
            doc, refusal = await _upload_sound(cl, file_path, ctx)
            if refusal:
                return refusal
            sound, label = types.NotificationSoundRingtone(id=doc.id), _sound_title(doc)
        elif tone.strip().casefold() == "default":
            sound, label = types.NotificationSoundDefault(), "the default sound"
        elif tone.strip().casefold() == "none":
            sound, label = types.NotificationSoundNone(), "no sound"
        else:
            doc, refusal = _find_sound(await _saved_sounds(cl), tone)
            if refusal:
                return refusal
            sound, label = types.NotificationSoundRingtone(id=doc.id), _sound_title(doc)
        ok = await _update_notify(cl, await resolve_entity(chat, cl), sound=sound)
        if ok is False:
            return "Telegram did not change the tone; nothing changed."
        return f"The chat's notifications now play {label}."
    except Exception as e:
        return log_and_format_error("set_chat_tone", e, chat=chat, tone=tone)


@mcp.tool(
    annotations=ToolAnnotations(
        title="List Saved Sounds",
        openWorldHint=True,
        readOnlyHint=True,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=True)
async def list_saved_sounds(account: str = None) -> str:
    """List the notification sounds saved in the account: id, title, length."""
    try:
        sounds = await _saved_sounds(get_client(account))
        if not sounds:
            return "No saved notification sounds."
        rows = []
        for doc in sounds:
            seconds = _sound_seconds(doc)
            length = f"{seconds:g} s" if seconds is not None else "length unknown"
            rows.append(f"ID: {doc.id} | {sanitize_name(_sound_title(doc))} | {length}")
        return "\n".join(rows)
    except Exception as e:
        return log_and_format_error("list_saved_sounds", e)


@mcp.tool(
    annotations=ToolAnnotations(
        title="Remove Saved Sound",
        openWorldHint=True,
        readOnlyHint=False,
        destructiveHint=True,
        idempotentHint=True,
    )
)
@with_account(readonly=False)
async def remove_saved_sound(sound: str, account: str = None) -> str:
    """Remove a notification sound from the saved sounds, by id or title."""
    try:
        cl = get_client(account)
        doc, refusal = _find_sound(await _saved_sounds(cl), str(sound))
        if refusal:
            return refusal
        await cl(functions.account.SaveRingtoneRequest(id=_input_document(doc), unsave=True))
        return f"Removed {_sound_title(doc)} from the saved sounds."
    except Exception as e:
        return log_and_format_error("remove_saved_sound", e, sound=sound)


__all__ = ["set_chat_sound_disabled", "set_chat_tone", "list_saved_sounds", "remove_saved_sound"]
