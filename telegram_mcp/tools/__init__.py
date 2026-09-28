"""Import tool modules so their MCP decorators register with the shared server."""

from telegram_mcp.tools.accounts import *
from telegram_mcp.tools.contacts import *
from telegram_mcp.tools.contact_aliases import *
from telegram_mcp.tools.chats import *
from telegram_mcp.tools.topics import *
from telegram_mcp.tools.topic_admin import *
from telegram_mcp.tools.peer_lookup import *
from telegram_mcp.tools.inline_bots import *
from telegram_mcp.tools.chat_state import *
from telegram_mcp.tools.messages import *
from telegram_mcp.tools.messages_delete import *
from telegram_mcp.tools.messages_relay import *
from telegram_mcp.tools.messages_read import *
from telegram_mcp.tools.messages_state import *
from telegram_mcp.tools.messages_queue import *
from telegram_mcp.tools.groups import *
from telegram_mcp.tools.chat_photo_history import *
from telegram_mcp.tools.moderation import *
from telegram_mcp.tools.admin_rights import *
from telegram_mcp.tools.invites import *
from telegram_mcp.tools.media import *
from telegram_mcp.tools.gifs import *
from telegram_mcp.tools.profile import *
from telegram_mcp.tools.profile_privacy import *
from telegram_mcp.tools.identity_tools import *
from telegram_mcp.tools.photos import *
from telegram_mcp.tools.folders import *
from telegram_mcp.tools.events import *
from telegram_mcp.tools.diagnostics import *
from telegram_mcp.tools.command_preview import *
from telegram_mcp.tools.channel_settings import *

# Visual and deep structured access. Kept last because these layer on top of the
# modules above; the grouping is historic (they were once the only fork-authored
# files) but the ordering still reflects the dependency direction.
from telegram_mcp.tools.inspection import *
from telegram_mcp.tools.media_inspection import *
from telegram_mcp.tools.visual import *
from telegram_mcp.tools.effects import *
from telegram_mcp.tools.buttons import *
from telegram_mcp.tools.scheduled import *
from telegram_mcp.tools.ephemeral import *
from telegram_mcp.tools.secret_chats import *
from telegram_mcp.tools.secret_messaging import *
from telegram_mcp.tools.secret_actions import *
from telegram_mcp.tools.secret_timed import *
from telegram_mcp.tools.rich_messages import *
from telegram_mcp.tools.mini_apps import *
from telegram_mcp.tools.invite_links import *
from telegram_mcp.tools.quick_replies import *
from telegram_mcp.tools.custom_emoji_edit import *
from telegram_mcp.tools.polls import *
from telegram_mcp.tools.stories import *
from telegram_mcp.tools.channel_admin import *
from telegram_mcp.tools.saved import *
from telegram_mcp.tools.stickers import *
from telegram_mcp.tools.saved_gifs import *
from telegram_mcp.tools.authorizations import *
from telegram_mcp.tools.read_receipts import *
from telegram_mcp.tools.message_search import *
from telegram_mcp.tools.poll_creation import *
from telegram_mcp.tools.channel_stats import *
from telegram_mcp.tools.ghost_tools import *
from telegram_mcp.tools.proxy_tools import *
from telegram_mcp.tools.chat_list import *
from telegram_mcp.tools.chat_notifications import *
from telegram_mcp.tools.chat_removal import *
from telegram_mcp.tools.members import *
from telegram_mcp.tools.reply_keyboard import *
from telegram_mcp.tools.checklists import *
from telegram_mcp.tools.poll_manage import *
from telegram_mcp.tools.stats_analysis import *
from telegram_mcp.tools.invite_link_admin import *
from telegram_mcp.tools.communities import *
from telegram_mcp.tools.community_moderation import *

# Spec 022 (plans 015-018): the rest of the channel settings, view counts, pinned-chat
# order, fact-checks, chat export and Telegram Business.
from telegram_mcp.tools.channel_identity import *
from telegram_mcp.tools.channel_moderation import *
from telegram_mcp.tools.channel_appearance import *
from telegram_mcp.tools.channel_structure import *
from telegram_mcp.tools.message_views import *
from telegram_mcp.tools.pinned_order import *
from telegram_mcp.tools.fact_checks import *
from telegram_mcp.tools.chat_export import *
from telegram_mcp.tools.business import *
from telegram_mcp.tools.business_links import *

# translation.py, not translate.py: `import *` binds the tool name `translate`
# into this package, which would otherwise shadow the submodule of the same name.
from telegram_mcp.tools.translation import *

# Last, once every tool is registered and runtime has installed the time budget: the
# safeguard goes IN FRONT of the budget, so a five-minute approval is not cut at 55 s.
from telegram_mcp.runtime import mcp as _mcp  # noqa: E402
from telegram_mcp.safeguard import install as _install_safeguard  # noqa: E402

_install_safeguard(_mcp)

# In front of the safeguard: a call that names no account is refused before anyone is
# asked to approve it (spec 019).
from telegram_mcp.account_gate import install as _install_account_gate  # noqa: E402

_install_account_gate(_mcp)

# Then arguments a tool is bound to refuse (spec 021): answered with the reason, again
# before the owner is asked.
from telegram_mcp.preflight import install as _install_preflight  # noqa: E402

_install_preflight(_mcp)

# Outside even the safeguard: every call an agent makes is recorded, refusals included.
from telegram_mcp.command_log import install as _install_command_log  # noqa: E402

_install_command_log(_mcp)

__all__ = [name for name in globals() if not name.startswith("_")]
