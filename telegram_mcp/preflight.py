"""Refuse a call that cannot succeed BEFORE anyone is asked to approve it (spec 021).

The safeguard asks the owner first and the tool checks its arguments afterwards, so a call
bound to fail spent a real approval: measured live, deleting the General topic was
approved on the phone and only then refused. This middleware sits after the account gate
and before the safeguard and runs two checks that need no network:

* the tool's own argument schema (the same pydantic model the SDK validates with, which
  otherwise only runs after the whole middleware chain), naming the field that is wrong;
* ``RULES``: argument combinations a tool refuses by itself, keyed by tool name.

A refusal says ``PREFLIGHT:``, the reason, and that nothing ran and no approval was asked.
The tools keep their own checks - they are also called directly - so these rules only move
the answer earlier, they never replace it.
"""

from __future__ import annotations

import dataclasses
from types import SimpleNamespace
from typing import Any, Callable, Dict, Optional

__all__ = ["Preflight", "RULES", "drop_null_defaults", "install"]


def _general_topic(args: Dict[str, Any]) -> Optional[str]:
    from telegram_mcp.tools.topic_admin import GENERAL_TOPIC

    if str(args.get("topic_id")) == str(GENERAL_TOPIC):
        return "The General topic (id 1) cannot be deleted."
    return None


def _pinned_order(args: Dict[str, Any]) -> Optional[str]:
    return None if args.get("topic_ids") else "Give at least one topic id."


def _paid_link_fee(args: Dict[str, Any]) -> Optional[str]:
    try:
        fee = int(args.get("monthly_fee_stars"))
    except (TypeError, ValueError):
        return None  # the schema check names a missing or non-numeric fee
    return None if fee >= 1 else "monthly_fee_stars must be at least 1."


def _parse_mode_or_entities(args: Dict[str, Any]) -> Optional[str]:
    if args.get("parse_mode") and args.get("entities"):
        return "Give parse_mode or entities, not both."
    return None


def _pinned_chats(args: Dict[str, Any]) -> Optional[str]:
    return None if args.get("order") else "Give at least one chat in order."


def _link_slug(args: Dict[str, Any]) -> Optional[str]:
    return None if str(args.get("slug") or "").strip() else "Give the link's slug."


def _work_hours(args: Dict[str, Any]) -> Optional[str]:
    if args.get("intervals") is None:
        return None  # clearing the hours
    from telegram_mcp.tools.business import normalize_work_hours

    try:
        return normalize_work_hours(args.get("intervals"))[1]
    except (TypeError, ValueError):
        return None  # a malformed shape is the schema check's to name


def _cancel_ids(args: Dict[str, Any]) -> Optional[str]:
    ids = args.get("message_id")
    if isinstance(ids, list) and not ids:
        return "message_id must not be an empty list."
    return None


RULES: Dict[str, Callable[[Dict[str, Any]], Optional[str]]] = {
    "delete_forum_topic": _general_topic,
    "reorder_pinned_topics": _pinned_order,
    "create_paid_invite_link": _paid_link_fee,
    "schedule_message": _parse_mode_or_entities,
    "cancel_scheduled_message": _cancel_ids,
    "reorder_pinned_chats": _pinned_chats,
    "delete_business_chat_link": _link_slug,
    "set_business_hours": _work_hours,
}


def _refusal(tool: str, reason: str):
    from mcp.types import CallToolResult, TextContent

    text = (
        f"PREFLIGHT: {tool} was not run. Reason: {reason} "
        "Nothing was changed and no approval was asked."
    )
    return CallToolResult(content=[TextContent(type="text", text=text)], is_error=True)


def _schema_problem(name: str, arguments: Dict[str, Any]) -> Optional[str]:
    from pydantic import ValidationError

    from telegram_mcp.runtime import mcp

    tool = mcp._tool_manager.get_tool(name)
    if tool is None:
        return None
    metadata = tool.fn_metadata
    try:
        metadata.arg_model.model_validate(metadata.pre_parse_json(dict(arguments)))
    except ValidationError as error:
        problems = [
            f"{'.'.join(str(p) for p in item['loc']) or 'arguments'}: {item['msg']}"
            for item in error.errors()[:5]
        ]
        return "invalid arguments - " + "; ".join(problems) + "."
    return None


def drop_null_defaults(name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
    """The arguments without an explicit ``null`` for a parameter whose default is None.

    Most tools spell an optional parameter ``X = None`` without ``Optional``, so the
    published schema allows no null and a client that sends ``bot: null`` for "not
    given" was refused - by this check and by the SDK after it (reported 2026-09-29:
    ``set_profile_photo`` could not set an account's own photo). ``null`` there means
    the default, so it is dropped and the default applies. A required parameter keeps
    its null and is refused by name.
    """
    from telegram_mcp.runtime import mcp

    tool = mcp._tool_manager.get_tool(name)
    if tool is None:
        return arguments
    fields = tool.fn_metadata.arg_model.model_fields
    return {
        key: value
        for key, value in arguments.items()
        if not (
            value is None
            and key in fields
            and not fields[key].is_required()
            and fields[key].default is None
        )
    }


def _with_arguments(ctx, arguments: Dict[str, Any]):
    """``ctx`` carrying ``arguments``: the SDK reads params off the ctx it is handed."""
    params = dict(ctx.params, arguments=arguments)
    if dataclasses.is_dataclass(ctx):
        return dataclasses.replace(ctx, params=params)
    return SimpleNamespace(**{**vars(ctx), "params": params})


class Preflight:
    """Middleware: answer a doomed tool call with its reason, before the safeguard."""

    async def __call__(self, ctx, call_next):
        params = getattr(ctx, "params", None)
        if getattr(ctx, "method", None) != "tools/call" or not params:
            return await call_next(ctx)
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if not isinstance(name, str) or not isinstance(arguments, dict):
            return await call_next(ctx)
        cleaned = drop_null_defaults(name, arguments)
        if cleaned != arguments:
            arguments, ctx = cleaned, _with_arguments(ctx, cleaned)
        reason = _schema_problem(name, arguments)
        rule = RULES.get(name)
        if reason is None and rule is not None:
            reason = rule(arguments)
        if reason:
            return _refusal(name, reason)
        return await call_next(ctx)


def install(server) -> None:
    """Once, immediately in front of the safeguard (after the account gate)."""
    if any(isinstance(m, Preflight) for m in server.middleware):
        return
    from telegram_mcp.safeguard import Safeguard

    at = next((i for i, m in enumerate(server.middleware) if isinstance(m, Safeguard)), 0)
    server.middleware.insert(at, Preflight())
