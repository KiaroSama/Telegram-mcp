"""Persisted backend policy; switching never migrates protocol custody."""

import json
from dataclasses import asdict

KEY = "mcp_secret_backend_v1"


def make(mode, **budgets):
    from telegram_mcp.secret_backend import CustodyLimits

    if mode not in ("normal", "bounded"):
        raise ValueError("Backend mode must be normal or bounded")
    if mode == "normal":
        if any(value is not None for value in budgets.values()):
            raise ValueError("Normal mode does not take custody budgets")
        limits = None
    else:
        values = {name: value for name, value in budgets.items() if value is not None}
        if not {"records", "bytes", "age_seconds"}.issubset(values):
            raise ValueError("Bounded mode requires records, bytes and age_seconds")
        limits = asdict(CustodyLimits(**values))
    return {"version": 1, "mode": mode, "limits": limits}


def load(storage):
    value = storage.load_setting(KEY)
    if value is None:
        return make("normal")
    if (
        not isinstance(value, dict)
        or set(value) != {"version", "mode", "limits"}
        or type(value["version"]) is not int
        or value["version"] != 1
    ):
        raise ValueError("Invalid secret backend policy; state was preserved")
    limits = value["limits"]
    if value["mode"] == "bounded" and not isinstance(limits, dict):
        raise ValueError("Invalid bounded budgets; state was preserved")
    if value["mode"] == "normal" and limits is not None:
        raise ValueError("Normal backend has incompatible budgets")
    return make(value["mode"], **(limits or {}))


def require_empty(storage):
    if storage.list() or storage.load_setting("bounded_secret_chat_custody_v1") not in (None, {}):
        raise ValueError(
            "Backend switching requires an empty secret-chat store; existing state was preserved"
        )
    path = getattr(storage, "path", None)
    if path is not None:
        document = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(document, dict) or any(
            document.get(name) != {} for name in ("chats", "out", "in")
        ):
            raise ValueError(
                "Backend switching requires an empty valid store, including all queues"
            )


def save_empty(storage, policy):
    require_empty(storage)
    with storage.transaction():
        storage.save_setting(KEY, policy)
