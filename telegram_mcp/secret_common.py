"""The translation layer between the published secret-chat surface and the package.

Every secret-chat tool answers in a shape that was fixed while the previous backend
was still installed, and `specs/003-tdlib-removal/evidence/baseline-tdlib.json` holds
that shape field for field. The package underneath speaks a different vocabulary, so
one module absorbs the difference and nothing above it has to know there was one.

Three translations live here, and each is a real difference rather than a rename:

**Two ids for one chat.** The published surface has both `chat_id` (large, negative)
and `secret_chat_id` (small, positive), and most tools take the first. The package has
only the second. They are not two names for one number, but they ARE a fixed offset
apart - measured on this account's own chats, both pairs agreeing:
`-1999372142509 <-> 627857491` and `-1999794063129 <-> 205936871`, i.e. exactly
`chat_id = secret_chat_id - 2_000_000_000_000`. So both published fields survive, and
a caller holding an id from before the migration still reaches its chat.

**The title.** The previous backend kept its own chat list and put a display name on
it. The package holds the conversation, not the address book, so the name is resolved
through the Telethon client that is already connected - the same place every other
tool in this server gets a name.

**The refusals.** A refusal used to quote the previous backend ("TDLib code 400: ...").
It now quotes the package, which raises a typed exception per reason. The WORDS
therefore change; what does not change is that Telegram's or the protocol's own verdict
reaches the caller instead of an error code pointing at a log.
"""

from typing import Optional

from telegram_mcp.sanitize import sanitize_name

# Through the seam, never from the package: `secret_backend` is the one module
# allowed to import it, so an upstream rename is a one-file edit.
from telegram_mcp.secret_backend import (
    ChatClosed,
    ChatNotReady,
    LayerUnsupported,
    MessageRejected,
    ParameterRejected,
    ResendUnsatisfiable,
    SecretChatError,
    SecretChatUnavailable,
    StorageRequired,
)

__all__ = [
    "REFUSED",
    "account_label",
    "SecretChatUnavailable",
    "chat_record",
    "describe_refusal",
    "peer_title",
    "to_chat_id",
    "to_secret_id",
]

# The measured distance between the two published ids. Not a guess and not a
# convention: it is what the previous backend's own chat list carried for every
# secret chat on this account, and it is what keeps `chat_id` a live field.
_CHAT_ID_OFFSET = 2_000_000_000_000

# Below this, an id is already a secret chat id. Telegram's secret chat ids are
# small (nine digits here); a published `chat_id` is always past a trillion, so
# the two ranges cannot collide and a caller may pass either.
_ID_BOUNDARY = 1_000_000_000_000

REFUSED = "Telegram refused this: "


def to_secret_id(value: int) -> int:
    """The package's chat id, from whichever of the two published ids arrived.

    Accepting both is not politeness. `chat_id` is what `create_secret_chat` and
    `list_secret_chats` published for the whole life of the previous backend, so an
    agent's notes, a saved plan and this project's own documentation are full of them.
    """
    value = int(value)
    if abs(value) >= _ID_BOUNDARY:
        return value + _CHAT_ID_OFFSET if value < 0 else value - _CHAT_ID_OFFSET
    return value


def to_chat_id(secret_chat_id: int) -> int:
    """The published `chat_id` for a package chat id."""
    return int(secret_chat_id) - _CHAT_ID_OFFSET


async def peer_title(client, peer_user_id: int) -> str:
    """The other side's display name, or an empty string when it cannot be read.

    A name is a convenience field on every one of these answers and never the thing
    the caller acts on, so a peer this account cannot resolve costs a blank rather
    than an error: the chat is still listed, still sendable, still closable.
    """
    try:
        entity = await client.get_entity(int(peer_user_id))
    except Exception:
        return ""
    name = " ".join(
        part
        for part in (getattr(entity, "first_name", None), getattr(entity, "last_name", None))
        if part
    )
    return sanitize_name(name or getattr(entity, "username", None) or "")


def chat_record(chat, title: str = "") -> dict:
    """One chat in the published shape, field for field.

    `self_destruct_timer` appears ONLY when the timer is off, which is how the
    previous backend published it: a caller switching on the key's presence is
    reading "no timer", and adding it unconditionally would change what that means.
    """
    record = {
        "chat_id": to_chat_id(chat.id),
        "secret_chat_id": chat.id,
        "peer_user_id": chat.peer_user_id,
        "title": title,
    }
    ttl = int(getattr(chat, "ttl", 0) or 0)
    record["self_destruct_timer_seconds"] = ttl
    if not ttl:
        record["self_destruct_timer"] = "off - messages stay until deleted"
    return record


def describe_refusal(error: Exception) -> Optional[str]:
    """The sentence to show for a refusal, or ``None`` when this is not one.

    A refusal is the protocol's or Telegram's verdict and belongs in front of the
    caller; a defect belongs in the log with a code. Returning ``None`` is how a
    caller's ``except`` block tells the two apart without listing the package's
    exception hierarchy at nineteen call sites.
    """
    if isinstance(error, ChatNotReady):
        # The one refusal worth expanding: "not ready" reads like a delay, and a
        # chat waiting for the far side to accept is not going to become ready on
        # its own. Naming the act that clears it saves a caller a retry loop.
        return (
            f"{REFUSED}{error}. A secret chat carries nothing until the other side "
            "accepts it; `list_secret_chats` shows the state."
        )
    if isinstance(error, ChatClosed):
        return (
            f"{REFUSED}{error}. A closed chat cannot be reopened - the key is gone "
            "on both sides. Use `create_secret_chat` for a new one."
        )
    if isinstance(error, StorageRequired):
        return (
            f"{REFUSED}{error}. This account's secret-chat key store could not be "
            "opened, so nothing can be sent without losing the ability to read it back."
        )
    if isinstance(
        error, (LayerUnsupported, MessageRejected, ParameterRejected, ResendUnsatisfiable)
    ):
        return f"{REFUSED}{error}"
    if isinstance(error, SecretChatUnavailable):
        return str(error)
    if isinstance(error, SecretChatError):
        return f"{REFUSED}{error}"
    return None


def account_label(account: Optional[str]) -> str:
    """The label an account's key store is kept under.

    ``get_client`` resolves ``None`` to the sole client but hands back the client
    rather than its name, and a key store is addressed by name. One copy of the
    rule, because two would be two chances to disagree about which account a key
    belongs to - and a key filed under the wrong account is a chat that can never
    be read again.

    The import is deferred: ``connection`` builds real clients at import time.
    """
    from telegram_mcp.connection import clients

    if account is None:
        if len(clients) == 1:
            return next(iter(clients))
        raise ValueError(f"Account is required. Available accounts: {', '.join(clients)}")
    label = account.lower()
    if label not in clients:
        raise ValueError(f"Unknown account '{account}'. Available accounts: {', '.join(clients)}")
    return label
