"""Read one recipient's Stars gift ledger entry through the existing account."""

from telegram_mcp.runtime import *
from telegram_mcp.service_receipts import stars_receipt_details, stars_transaction_details


@mcp.tool(
    annotations=ToolAnnotations(
        title="Get Stars Receipt",
        openWorldHint=True,
        readOnlyHint=True,
        destructiveHint=False,
        idempotentHint=True,
    )
)
@with_account(readonly=True)
async def get_stars_receipt(message_id: int, account: str = None) -> str:
    """Read the exact Stars ledger entry named by one Telegram service receipt.

    message_id is a gift receipt in this account's Telegram service chat (777000).
    Its opaque transaction identifier is read internally, never supplied in arguments.
    Reads only this account's own ledger, using the existing client. No purchase,
    transfer, refund, mark-read, new login or link navigation. Optional transaction_url
    is an untrusted Telegram-reported link, not a universal supplier delivery proof.
    Missing or conflicting data is unavailable, never inferred success.
    """
    unavailable = {"status": "unavailable"}
    if type(message_id) is not int or not 0 < message_id < 2**31:
        return json.dumps({**unavailable, "code": "invalid_message_id"})
    try:
        cl = get_client(account)
        await ensure_connected(cl)
        message = await cl.get_messages(777000, ids=message_id)
        receipt = stars_receipt_details(getattr(message, "action", None))
        reference = (receipt or {}).get("transaction_id")
        if getattr(message, "id", None) != message_id or not reference:
            return json.dumps({**unavailable, "code": "stars_receipt_unavailable"})
        result = await cl(
            functions.payments.GetStarsTransactionsByIDRequest(
                peer=types.InputPeerSelf(), id=[types.InputStarsTransaction(id=reference)]
            )
        )
        history = getattr(result, "history", None)
        transaction = (
            stars_transaction_details(history[0], reference)
            if isinstance(history, list) and len(history) == 1
            else None
        )
        if transaction is None:
            return json.dumps({**unavailable, "code": "stars_transaction_unavailable"})
        return json.dumps(
            {
                "status": "observed",
                "message_id": message_id,
                "receipt": receipt,
                "transaction": transaction,
            },
            ensure_ascii=False,
        )
    except Exception:
        # Upstream errors can contain financial identifiers; keep diagnostics content-free.
        log_event(logging.ERROR, "Stars receipt lookup unavailable", message_id=message_id)
        return json.dumps({**unavailable, "code": "stars_lookup_unavailable"})


__all__ = ["get_stars_receipt"]
