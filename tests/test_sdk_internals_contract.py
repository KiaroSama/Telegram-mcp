"""The private SDK attributes this project reaches for, named in one place (plan 003).

mcp 2.x documents its middleware list as provisional and documents none of the registry
internals below. A version bump that renamed one used to fail as whichever test tripped
first; this file fails first and says which module depends on the moved name. The
account gate is fail-open on a missing registry lookup, so this is also what keeps it shut.
"""

import pydantic

from telegram_mcp.tools import mcp  # noqa: F401  (registers every tool)

USED_BY = {
    "mcp._tool_manager.get_tool": (
        "telegram_mcp/account_gate.py",
        "telegram_mcp/preflight.py",
        "telegram_mcp/safeguard/wiring.py",
    ),
    "mcp._tool_manager.list_tools": ("telegram_mcp/runtime.py", "telegram_mcp/account_gate.py"),
    "mcp._tool_manager.remove_tool": ("telegram_mcp/runtime.py",),
    "tool.fn_metadata.arg_model": ("telegram_mcp/preflight.py",),
    "tool.fn_metadata.pre_parse_json": ("telegram_mcp/preflight.py",),
    "tool.parameters['properties']": ("telegram_mcp/account_gate.py",),
    "tool.annotations.read_only_hint/destructive_hint": ("telegram_mcp/safeguard/wiring.py",),
    "mcp.server.mcpserver.utilities.func_metadata.ArgModelBase": ("telegram_mcp/runtime.py",),
    "mcp.middleware (list, insert/append)": (
        "telegram_mcp/tools/__init__.py",
        "telegram_mcp/command_log.py",
    ),
    "ServerRequestContext is a dataclass (dataclasses.replace)": ("telegram_mcp/preflight.py",),
}


def _expect(condition, key):
    assert condition, f"the SDK no longer offers {key}; used by {', '.join(USED_BY[key])}"


def test_the_registry_internals_are_still_there():
    manager = getattr(mcp, "_tool_manager", None)
    _expect(manager is not None, "mcp._tool_manager.get_tool")
    tool = manager.get_tool("get_me")
    _expect(tool is not None and tool.name == "get_me", "mcp._tool_manager.get_tool")
    _expect(manager.get_tool("no_such_tool") is None, "mcp._tool_manager.get_tool")
    _expect(callable(getattr(manager, "list_tools", None)), "mcp._tool_manager.list_tools")
    _expect(callable(getattr(manager, "remove_tool", None)), "mcp._tool_manager.remove_tool")


def test_the_argument_model_internals_are_still_there():
    tool = mcp._tool_manager.get_tool("send_message")
    metadata = getattr(tool, "fn_metadata", None)
    model = getattr(metadata, "arg_model", None)
    _expect(
        isinstance(model, type) and issubclass(model, pydantic.BaseModel),
        "tool.fn_metadata.arg_model",
    )
    _expect(callable(getattr(metadata, "pre_parse_json", None)), "tool.fn_metadata.pre_parse_json")
    properties = tool.parameters.get("properties")
    _expect(
        isinstance(properties, dict) and "account" in properties, "tool.parameters['properties']"
    )
    _expect(
        isinstance(tool.annotations.read_only_hint, bool)
        and isinstance(tool.annotations.destructive_hint, bool),
        "tool.annotations.read_only_hint/destructive_hint",
    )


def test_the_forbid_extra_hook_the_middleware_list_and_the_context_are_still_there():
    import dataclasses

    from mcp.server.context import ServerRequestContext
    from mcp.server.mcpserver.utilities.func_metadata import ArgModelBase

    _expect(
        ArgModelBase.model_config.get("extra") == "forbid",
        "mcp.server.mcpserver.utilities.func_metadata.ArgModelBase",
    )
    _expect(
        isinstance(mcp.middleware, list) and len(mcp.middleware) >= 4,
        "mcp.middleware (list, insert/append)",
    )
    _expect(
        dataclasses.is_dataclass(ServerRequestContext),
        "ServerRequestContext is a dataclass (dataclasses.replace)",
    )
