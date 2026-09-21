"""A secret-chat manager that records what it was asked to do.

Shared by the four secret-tool suites, because they all need the same three things
and a fake per file would be three chances for them to disagree about what the
backend's contract is.

Nothing here opens a socket or holds key material. What it stands in for is the
package's manager: the object `telegram_mcp.secret_backend.secret_manager` returns,
whose surface `tests/test_secret_backend_contract.py` pins against the real one.
"""

from types import SimpleNamespace


class FakeState:
    """One chat's state, in the shape the tools read it."""

    def __init__(self, value):
        self.value = value


class FakeChat:
    def __init__(self, chat_id, *, peer=5001, state="ready", outbound=True, ttl=0, layer=144):
        self.id = chat_id
        self.peer_user_id = peer
        self.state = FakeState(state)
        self.is_outbound = outbound
        self.ttl = ttl
        self.layer = layer


class FakeManager:
    """Records every call; raises what the real one raises for an unknown chat."""

    def __init__(self, chats=()):
        self._chats = {chat.id: chat for chat in chats}
        self.sent = []
        self.files = []
        self.ttls = []
        self.read = []
        self.deleted = []
        self.flushed = []
        self.typing = []
        self.closed = []
        self.created = []
        self.saved = []
        self.history = {}
        self.next_id = 700_000

    # --- lifecycle ---------------------------------------------------------
    async def create(self, user):
        chat = FakeChat(910_000, peer=getattr(user, "id", user), state="requested")
        self._chats[chat.id] = chat
        self.created.append(user)
        return chat

    async def close(self, chat_id, reason="closed"):
        self._require(chat_id)
        self.closed.append(int(chat_id))

    def list(self):
        return list(self._chats.values())

    def status(self, chat_id):
        return self._require(chat_id)

    # --- sending -----------------------------------------------------------
    async def send_message(self, chat_id, text, entities=None, reply_to=None):
        self._require(chat_id)
        self.next_id += 1
        self.sent.append(
            SimpleNamespace(chat_id=chat_id, text=text, entities=entities, reply_to=reply_to)
        )
        return self.next_id

    async def send_file(
        self, chat_id, path, *, caption="", mime_type=None, kind=None, reply_to=None
    ):
        self._require(chat_id)
        self.next_id += 1
        self.files.append(
            SimpleNamespace(
                chat_id=chat_id, path=path, caption=caption, kind=kind, reply_to=reply_to
            )
        )
        return self.next_id

    async def save_file(self, message, path):
        self.saved.append((message, path))
        path.write_bytes(b"decrypted")
        return path

    # --- actions -----------------------------------------------------------
    async def set_ttl(self, chat_id, seconds):
        chat = self._require(chat_id)
        chat.ttl = int(seconds)
        self.ttls.append((int(chat_id), int(seconds)))

    async def mark_read(self, chat_id, random_ids):
        self._require(chat_id)
        self.read.append((int(chat_id), list(random_ids)))

    async def delete_messages(self, chat_id, random_ids):
        self._require(chat_id)
        self.deleted.append((int(chat_id), list(random_ids)))

    async def flush_history(self, chat_id):
        self._require(chat_id)
        self.flushed.append(int(chat_id))

    async def set_typing(self, chat_id, action=None):
        self._require(chat_id)
        self.typing.append((int(chat_id), type(action).__name__))

    def read_history(self, chat_id, limit=50):
        return self.history.get(int(chat_id), [])[-limit:]

    def _require(self, chat_id):
        chat = self._chats.get(int(chat_id))
        if chat is None:
            raise KeyError(f"no secret chat {chat_id} in this manager")
        return chat


#: The published `chat_id` for the fake chat every suite uses, and its own id.
SECRET_ID = 627857491
CHAT_ID = SECRET_ID - 2_000_000_000_000
