"""Explicit bounded consumer settlement, separate from protocol custody.

The owner-only snapshot is the consumer commit, not the best-effort history
callback. Its message and private media reference survive an interrupted library
ack. No polling, callback or RAM receipt releases a delivery obligation.
"""

import json
import math
import os
from time import time as _commit_time
import tempfile
import threading
from copy import deepcopy
from pathlib import Path

from telegram_mcp import secret_backend as backend, secret_history
from telegram_mcp.alias_store import normalise_account_label
from telegram_mcp.owner_only import restrict_to_owner_strict
from telegram_mcp.sanitize import sanitize_user_content
from telegram_mcp.settings import state_dir

__all__ = [
    "acknowledge",
    "media_source",
    "pending_page",
    "pause_record",
    "read_receipt",
    "read_history",
    "receipt_media_source",
    "require_bounded",
    "validate_identity",
    "validate_page",
]

# ponytail: process-local lock; the backend already owns the account's store lease.
_lock = threading.RLock()
_UNSET = object()


def require_bounded(manager):
    if not isinstance(manager, backend.BoundedSecretChatManager):
        raise ValueError("This operation requires the optional bounded secret backend")
    return manager


def validate_identity(chat_id, seq_no=_UNSET):
    if type(chat_id) is not int or chat_id <= 0:
        raise ValueError("secret_chat_id must identify a positive secret chat")
    if seq_no is not _UNSET and (type(seq_no) is not int or seq_no < 0):
        raise ValueError("seq_no must be a nonnegative wire sequence integer")


def validate_page(limit, offset):
    if type(limit) is not int or not 1 <= limit <= 100:
        raise ValueError("limit must be an integer from 1 to 100")
    if type(offset) is not int or not 0 <= offset <= 4096:
        raise ValueError("offset must be an integer from 0 to 4096")


def pause_record(status):
    """Allowlisted payload-free projection, never generic dataclass serialization."""
    return {
        "secret_chat_id": status.chat_id,
        "paused": status.paused,
        "resumable": status.resumable,
        "reason": status.reason,
        "records": status.records,
        "bytes": status.bytes,
    }


def _message(message):
    kind = secret_history.media_kind(message.media)
    record = secret_history.entry(
        message_id=message.random_id,
        is_outgoing=False,
        text=sanitize_user_content(message.text) if message.text else "",
        kind=kind,
        ttl=message.ttl,
        file_id=message.random_id if kind else None,
    )
    # Wire messages have no date. A polling time is not a stable delivery field.
    record.pop("date")
    if message.reply_to is not None:
        record["reply_to_message_id"] = message.reply_to
    return record


def pending_page(manager, chat_id, *, limit=50, offset=0):
    require_bounded(manager)
    validate_identity(chat_id)
    validate_page(limit, offset)
    pending = manager.pending_deliveries(chat_id)
    page = pending[offset : offset + limit]
    next_offset = offset + len(page)
    return {
        "secret_chat_id": chat_id,
        "deliveries": [
            {"seq_no": item.seq_no, "message": _message(item.message)} for item in page
        ],
        "total": len(pending),
        "offset": offset,
        "next_offset": next_offset if next_offset < len(pending) else None,
        "acknowledged": False,
    }


def _path(account):
    if not isinstance(account, str) or normalise_account_label(account) != account:
        raise ValueError("A canonical account label is required for consumer storage")
    return state_dir() / "secret-chats" / f"{account}-consumer.json"


def _read(account):
    try:
        value = json.loads(_path(account).read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {"version": 1, "chats": {}}
    if (
        not isinstance(value, dict)
        or set(value) != {"version", "chats"}
        or type(value["version"]) is not int
        or value["version"] != 1
        or not isinstance(value["chats"], dict)
    ):
        raise ValueError("Invalid consumer receipt snapshot; its bytes were preserved")
    for chat_id, receipts in value["chats"].items():
        if (
            not isinstance(chat_id, str)
            or not chat_id.isascii()
            or not chat_id.isdigit()
            or int(chat_id) <= 0
            or str(int(chat_id)) != chat_id
            or not isinstance(receipts, dict)
        ):
            raise ValueError("Invalid consumer receipt conversation; state was preserved")
        for sequence, receipt in receipts.items():
            if (
                not isinstance(sequence, str)
                or not sequence.isascii()
                or not sequence.isdigit()
                or str(int(sequence)) != sequence
                or not isinstance(receipt, dict)
                or set(receipt) != {"seq_no", "message", "media_reference", "committed_at"}
                or type(receipt["committed_at"]) not in (int, float)
                or not math.isfinite(receipt["committed_at"])
                or receipt["committed_at"] <= 0
                or type(receipt["seq_no"]) is not int
                or receipt["seq_no"] != int(sequence)
                or not isinstance(receipt["message"], dict)
                or type(receipt["message"].get("message_id")) is not int
                or (
                    receipt["media_reference"] is not None
                    and not isinstance(receipt["media_reference"], dict)
                )
            ):
                raise ValueError("Invalid consumer receipt delivery; state was preserved")
    return value


def _write(account, value):
    path = _path(account)
    path.parent.mkdir(parents=True, exist_ok=True)
    if not restrict_to_owner_strict(path.parent):
        raise PermissionError("Consumer receipt directory could not be made owner-only")
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, suffix=".consumer.tmp")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as handle:
            if not restrict_to_owner_strict(temporary):
                raise PermissionError("Consumer receipt snapshot could not be made owner-only")
            json.dump(value, handle, ensure_ascii=False, allow_nan=False)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        if os.name != "nt":
            directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
    except BaseException:
        try:
            Path(temporary).unlink(missing_ok=True)
        except OSError:
            pass  # Never hide the original commit failure.
        raise


def read_receipt(account, chat_id, seq_no):
    """Private durable receipt; callers must not return its media reference."""
    validate_identity(chat_id, seq_no)
    with _lock:
        return _read(account)["chats"].get(str(chat_id), {}).get(str(seq_no))


def acknowledge(manager, account, chat_id, seq_no):
    """Commit consumer custody first, then settle the named library obligation.

    A failed consumer commit raises before ack. A failed library ack leaves the
    durable receipt and reports an unconfirmed settlement, reconcilable on retry.
    """
    require_bounded(manager)
    validate_identity(chat_id, seq_no)
    with _lock:
        state = _read(account)
        chat = state["chats"].setdefault(str(chat_id), {})
        previous = chat.get(str(seq_no))
        delivery = next(
            (item for item in manager.pending_deliveries(chat_id) if item.seq_no == seq_no), None
        )
        if delivery is None:
            if previous is None:
                raise ValueError("No pending delivery or consumer receipt has this wire sequence")
            return {
                "secret_chat_id": chat_id,
                "seq_no": seq_no,
                "consumer_committed": True,
                "acknowledged": True,
                "reconciled": True,
            }
        message = _message(delivery.message)
        reference = backend.MediaReference.from_message(delivery.message)
        receipt = {
            "seq_no": seq_no,
            "message": message,
            "media_reference": reference.to_dict() if reference else None,
            "committed_at": previous["committed_at"] if previous else _commit_time(),
        }
        # A retry retains the original observed commit time, never a new arrival date.
        if previous is not None and any(
            previous[key] != receipt[key] for key in ("seq_no", "message", "media_reference")
        ):
            raise ValueError(
                "Consumer receipt conflicts with this delivery; custody was preserved"
            )
        if previous is None:
            chat[str(seq_no)] = receipt
            _write(account, state)
        try:
            manager.acknowledge_delivery(chat_id, seq_no)
        except Exception as error:
            return {
                "secret_chat_id": chat_id,
                "seq_no": seq_no,
                "consumer_committed": True,
                "acknowledged": None,
                "reason": f"Library settlement unconfirmed ({type(error).__name__}); reconcile the same wire sequence",
            }
        return {
            "secret_chat_id": chat_id,
            "seq_no": seq_no,
            "consumer_committed": True,
            "acknowledged": True,
            "reconciled": previous is not None,
        }


def media_source(manager, account, chat_id, message_id):
    """Pending media first, existing references next, committed receipts last."""
    require_bounded(manager)
    validate_identity(chat_id)
    if type(message_id) is not int:
        raise ValueError("message_id must be an integer")
    for delivery in manager.pending_deliveries(chat_id):
        if delivery.message.random_id == message_id:
            return delivery.message
    source, _ttl = backend.media_source(manager, account, chat_id, message_id)
    if source is not None:
        return source
    source, _ttl = receipt_media_source(account, chat_id, message_id)
    if source is not None:
        return source
    raise ValueError("No retained media reference has this message_id")


def read_history(account, chat_id, limit):
    """Detached sanitized messages by consumer commit time, wire order on ties.

    Independent ack order is not receive order. The observed consumer commit
    time is explicitly labelled, never presented as a wire arrival timestamp.
    """
    validate_identity(chat_id)
    if type(limit) is not int or not 1 <= limit <= 10_000:
        raise ValueError("history limit must be an integer from 1 to 10000")
    with _lock:
        receipts = _read(account)["chats"].get(str(chat_id), {})
        messages = {}
        for key in sorted(receipts, key=lambda key: (receipts[key]["committed_at"], int(key))):
            receipt = receipts[key]
            message = deepcopy(receipt["message"])
            # Allowlisted fields only; a private media reference never follows it.
            message = {
                key: value
                for key, value in message.items()
                if key
                in {
                    "message_id",
                    "is_outgoing",
                    "type",
                    "can_be_saved",
                    "text",
                    "caption",
                    "file_id",
                    "self_destructs_after_seconds",
                    "reply_to_message_id",
                }
            }
            for field in ("text", "caption"):
                if field in message:
                    message[field] = sanitize_user_content(message[field])
            message["consumer_committed_at"] = receipt["committed_at"]
            message["date_basis"] = "consumer_commit"
            messages.setdefault(message["message_id"], message)
        return list(messages.values())[-limit:]


def receipt_media_source(account, chat_id, message_id):
    """Private (reference, ttl) fallback; never calls backend.media_source."""
    validate_identity(chat_id)
    if type(message_id) is not int:
        raise ValueError("message_id must be an integer")
    with _lock:
        for receipt in _read(account)["chats"].get(str(chat_id), {}).values():
            if (
                receipt["message"]["message_id"] == message_id
                and receipt["media_reference"] is not None
            ):
                return backend.MediaReference.from_dict(receipt["media_reference"]), int(
                    receipt["message"].get("self_destructs_after_seconds", 0)
                )
    return None, 0
