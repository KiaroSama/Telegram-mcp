"""Isolate proven invalid logins without silently accepting unknown startup failures."""

import asyncio

from telethon import errors

from telegram_mcp import admission, retirement
from telegram_mcp.settings import StartupMessage


async def start_accounts(accounts, connect, note):
    snapshot = list(accounts.items())
    outcomes = await asyncio.gather(
        *(connect(label, client) for label, client in snapshot), return_exceptions=True
    )
    for (label, client), outcome in zip(snapshot, outcomes):
        if not isinstance(outcome, BaseException):
            continue
        invalid = isinstance(outcome, errors.UnauthorizedError) or (
            isinstance(outcome, StartupMessage) and "is not authorized." in str(outcome)
        )
        if not invalid:
            raise outcome
        # Use the existing owner-aware retirement contract. Never release the
        # identity lease merely because the account left the serving registry.
        closing = retirement.retire(client)
        if closing is not None:
            closing = asyncio.ensure_future(closing)
        admission.forget(label, closing=closing, client=client)
        if closing is not None:
            try:
                await asyncio.wait_for(asyncio.shield(closing), timeout=5)
            except Exception:
                pass  # forget retains a lease whose close is unconfirmed.
        if accounts.get(label) is client:
            accounts.pop(label)
        note(
            f"[{label}] Login is invalid; account disabled until re-login. Other accounts continue."
        )
    if not accounts:
        raise StartupMessage(
            "No authorized accounts remain. Use Manage-Accounts.ps1 option 6 to re-login."
        )
