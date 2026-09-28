"""Every call that could act through an account names it (spec 019).

A call without ``account`` used to fan out over every account (reads) or reach the
safeguard, ask the owner for approval and only then fail with "'account' is required"
(writes). The owner's rule: without an account the call is meaningless. So:

* every tool that takes ``account`` lists it as REQUIRED in its schema, so a client asks
  the model for it before the call is sent;
* a call that still arrives without one, or with a label that is not configured, is
  answered here - in front of the safeguard, so no approval is ever asked for it - with
  the labels to choose from. Nothing runs.

This holds with one account configured too: the rule does not change when a second
account is added. Tools without ``account`` act on the server itself and pass untouched.
"""

from __future__ import annotations

from typing import Callable, Iterable, Optional

__all__ = ["AccountGate", "install", "require_account"]

# Where `account` only FILTERS a view of this server's own state - omitted, every account
# is listed - or names whose grant to drop, which may belong to an account no longer
# configured. Nothing here acts on Telegram through the account it names.
SCOPE_ONLY = frozenset(
    {"safeguard_status", "get_ghost_mode", "get_connection_route", "revoke_always_approval"}
)

_DESCRIPTION = (
    "Required: the label of the account this call acts through (see list_accounts). "
    "A call without it is refused."
)


def _refusal(tool: str, text: str):
    from mcp.types import CallToolResult, TextContent

    return CallToolResult(
        content=[TextContent(type="text", text=f"ACCOUNT REQUIRED: {tool} was not run. {text}")],
        is_error=True,
    )


def _registered_takes_account(name: str) -> bool:
    from telegram_mcp.runtime import mcp

    tool = mcp._tool_manager.get_tool(name)
    return _acts_through_account(tool)


def _acts_through_account(tool) -> bool:
    return (
        tool is not None
        and tool.name not in SCOPE_ONLY
        and "account" in (tool.parameters.get("properties") or {})
    )


def _configured_labels() -> Iterable[str]:
    from telegram_mcp import connection

    connection.refresh_accounts()
    return sorted(connection.clients)


class AccountGate:
    """Middleware: refuse an account tool call that names no configured account."""

    def __init__(
        self,
        takes_account: Optional[Callable[[str], bool]] = None,
        labels: Optional[Callable[[], Iterable[str]]] = None,
    ) -> None:
        self._takes_account = takes_account or _registered_takes_account
        self._labels = labels or _configured_labels

    async def __call__(self, ctx, call_next):
        params = getattr(ctx, "params", None)
        if getattr(ctx, "method", None) != "tools/call" or not params:
            return await call_next(ctx)
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if not isinstance(name, str) or not isinstance(arguments, dict):
            return await call_next(ctx)
        if not self._takes_account(name):
            return await call_next(ctx)
        labels = list(self._labels())
        choices = ", ".join(labels) or "none configured"
        account = arguments.get("account")
        if not isinstance(account, str) or not account.strip():
            return _refusal(name, f"Name the account with `account`: {choices}.")
        if account.strip().lower() not in {label.lower() for label in labels}:
            return _refusal(name, f"No account is called '{account}'. Use one of: {choices}.")
        return await call_next(ctx)


def require_account(server) -> None:
    """List ``account`` as required in every schema that has it."""
    for tool in server._tool_manager.list_tools():
        if not _acts_through_account(tool):
            continue
        schema = tool.parameters["properties"]["account"]
        schema.pop("default", None)
        schema["type"] = "string"
        schema.pop("anyOf", None)
        schema["description"] = _DESCRIPTION
        required = tool.parameters.setdefault("required", [])
        if "account" not in required:
            required.append("account")


def install(server) -> None:
    """Schemas once; the gate once, immediately in front of the safeguard."""
    require_account(server)
    if any(isinstance(m, AccountGate) for m in server.middleware):
        return
    from telegram_mcp.safeguard import Safeguard

    at = next((i for i, m in enumerate(server.middleware) if isinstance(m, Safeguard)), 0)
    server.middleware.insert(at, AccountGate())
