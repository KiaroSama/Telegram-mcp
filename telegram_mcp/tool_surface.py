"""What an MCP client sees of the tools: every result stamped `audience=["user"]`, the
`TELEGRAM_EXPOSED_TOOLS` pruning, and the ceiling on a whole tool call.

Split out of `runtime.py` when that file reached the size ceiling (plan 014). It depends
on the server object and on nothing Telegram; `runtime` re-exports every name here, so
`runtime._apply_exposed_tools_mode` and the rest keep working for `runner` and the tests.
"""

import os
from typing import Optional

from mcp.types import Annotations

# Annotate all tool results with audience=["user"] so MCP clients know
# the content is user-generated data, not instructions for the model.
# Installed as server middleware so it runs for every tools/call, whatever the
# transport, and injects annotations into the final CallToolResult while
# preserving structured output.
_USER_AUDIENCE = Annotations(audience=["user"])


def _annotate_for_user(content: list) -> list:
    """Mark every unannotated content block as user data.

    Every block type MCP defines carries an ``annotations`` field - text,
    image, audio, resource link and embedded resource alike - and a tool that
    returns a screenshot is handing back user data exactly as much as one that
    returns a message body. This used to test ``isinstance(block, TextContent)``,
    so an image came back with nothing said about it at all. Asking the model
    for the field instead of listing the classes also means a block type added
    by a later MCP release is covered on arrival.
    """
    annotated = []
    for block in content:
        fields = getattr(type(block), "model_fields", {})
        if "annotations" in fields and getattr(block, "annotations", None) is None:
            block = block.model_copy(update={"annotations": _USER_AUDIENCE})
        annotated.append(block)
    return annotated


class _UserAudienceMiddleware:
    """Stamp every tool result as user data, on the way out.

    Under mcp 1.x this reached into `mcp._mcp_server.request_handlers` and
    replaced the `CallToolRequest` entry. 2.x removed `_mcp_server`; the
    supported seam is the server's middleware chain, which every request passes
    through regardless of transport. That is a better fit than it looks: the old
    hook could only ever see the one handler it swapped, while middleware sees
    the result whatever produced it.

    The unwrapping is deliberately permissive. A result may arrive as a bare
    `CallToolResult` or wrapped in a `ServerResult`, and this must annotate
    either without caring which - a shape it does not recognise is passed
    through untouched rather than dropped, because failing to annotate is a
    smaller harm than swallowing a tool's answer.
    """

    async def __call__(self, ctx, call_next):
        from mcp.types import CallToolResult

        result = await call_next(ctx)

        target = result
        if not isinstance(target, CallToolResult):
            target = getattr(result, "root", None)
        if isinstance(target, CallToolResult) and target.content:
            target.content = _annotate_for_user(target.content)
        return result


def _server(server):
    if server is None:
        from telegram_mcp.runtime import mcp

        return mcp
    return server


def _install_annotation_hook(server=None) -> None:
    """Append the stamper to the middleware chain, exactly once."""
    server = _server(server)
    if any(isinstance(m, _UserAudienceMiddleware) for m in server.middleware):
        return
    server.middleware.append(_UserAudienceMiddleware())


def install(server) -> None:
    """The audience stamp, then the ceiling on the whole tool call - the order runtime
    has always installed them in. Every Telegram call is bounded individually; the CALL
    was not, so a request that wedged left the client waiting for its own idle timeout
    and then reporting a transport failure for a stalled operation."""
    from telegram_mcp.tool_budget import install as install_tool_budget

    _install_annotation_hook(server)
    install_tool_budget(server)


_EXPOSED_TOOLS_MODES = {"all", "read-only"}
_EXPOSED_TOOLS_ALLOW_SEPARATOR = "+"


def _split_exposed_tools_mode(mode: str) -> tuple[str, list[str]]:
    """Split a normalised exposure mode into its base mode and write allowlist."""
    base, separator, raw_allowlist = mode.partition(_EXPOSED_TOOLS_ALLOW_SEPARATOR)
    if not separator:
        return base, []
    return base, [name.strip() for name in raw_allowlist.split(",") if name.strip()]


def _get_exposed_tools_mode(value: Optional[str] = None) -> str:
    """Return the configured MCP tool exposure mode.

    ``TELEGRAM_EXPOSED_TOOLS=read-only`` keeps only tools annotated with
    ``readOnlyHint=True``. ``read-only+send_message,reply_to_message`` keeps
    those plus the named write tools. The default is ``all`` for backward
    compatibility.
    """
    raw_value = os.getenv("TELEGRAM_EXPOSED_TOOLS", "all") if value is None else value
    mode = raw_value.strip().lower()
    base_mode, allowlist = _split_exposed_tools_mode(mode)
    if base_mode not in _EXPOSED_TOOLS_MODES:
        accepted = ", ".join(sorted(_EXPOSED_TOOLS_MODES))
        raise SystemExit(
            f"Invalid TELEGRAM_EXPOSED_TOOLS '{raw_value}'. Expected one of: {accepted}."
        )
    if _EXPOSED_TOOLS_ALLOW_SEPARATOR not in mode:
        return base_mode
    if base_mode != "read-only":
        raise SystemExit(
            f"Invalid TELEGRAM_EXPOSED_TOOLS '{raw_value}'. The "
            f"'{_EXPOSED_TOOLS_ALLOW_SEPARATOR}tool,tool' allowlist is only valid "
            "with read-only."
        )
    if not allowlist:
        raise SystemExit(
            f"Invalid TELEGRAM_EXPOSED_TOOLS '{raw_value}'. The "
            f"'{_EXPOSED_TOOLS_ALLOW_SEPARATOR}' allowlist must name at least one tool."
        )
    return f"{base_mode}{_EXPOSED_TOOLS_ALLOW_SEPARATOR}{','.join(allowlist)}"


def _is_read_only(annotations) -> bool:
    """Whether a tool declares itself read-only, under either SDK spelling.

    This decides what `TELEGRAM_EXPOSED_TOOLS=read-only` KEEPS, so reading the
    wrong attribute name is not cosmetic. mcp 1.x spelled the field
    `readOnlyHint`; 2.x renamed it `read_only_hint` and kept the old spelling
    only as a construction alias - so every `ToolAnnotations(readOnlyHint=True)`
    in this codebase still builds, while `getattr(a, "readOnlyHint", False)`
    silently returned the default for every tool. Read-only mode would have
    stripped the entire registry.

    Both names are accepted so the check cannot break again on whichever
    spelling the installed SDK happens to use, and the default stays False:
    a tool that does not clearly say it is read-only is not treated as one.
    """
    for attribute in ("read_only_hint", "readOnlyHint"):
        value = getattr(annotations, attribute, None)
        if value is not None:
            return bool(value)
    return False


def _apply_exposed_tools_mode(server=None, mode: Optional[str] = None) -> list[str]:
    """Prune registered MCP tools according to the configured exposure mode."""
    server = _server(server)
    selected_mode = _get_exposed_tools_mode() if mode is None else _get_exposed_tools_mode(mode)
    base_mode, allowlist = _split_exposed_tools_mode(selected_mode)
    if base_mode == "all":
        return []

    registered = {tool.name for tool in server._tool_manager.list_tools()}
    unknown = sorted(set(allowlist) - registered)
    if unknown:
        # Fail loudly: a typo must not silently degrade into a narrower allowlist
        # that looks like it worked.
        raise SystemExit(
            f"Invalid TELEGRAM_EXPOSED_TOOLS allowlist: unknown tool(s) {', '.join(unknown)}."
        )

    allowed = set(allowlist)
    removed: list[str] = []
    for tool in list(server._tool_manager.list_tools()):
        if tool.name in allowed:
            continue
        annotations = getattr(tool, "annotations", None)
        if not _is_read_only(annotations):
            server._tool_manager.remove_tool(tool.name)
            removed.append(tool.name)
    return removed
