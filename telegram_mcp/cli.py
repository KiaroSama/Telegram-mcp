"""Lightweight console dispatch before the server's import-time registration."""

import sys


def main() -> None:
    from telegram_mcp.install_guard import UnsafeInstallationError, assert_safe_distribution

    try:
        assert_safe_distribution()
    except UnsafeInstallationError as error:
        raise SystemExit(str(error)) from None

    if len(sys.argv) > 1 and sys.argv[1] == "bridge":
        if len(sys.argv) != 2:
            raise SystemExit("Usage: telegram-mcp bridge")
        try:
            from telegram_mcp.bridge_startup import ensure_service, verify_service
            from telegram_mcp.bridge import run

            url = ensure_service()
            run(url, verify_endpoint=lambda: verify_service(url))
        except KeyboardInterrupt:
            return
        except Exception:
            raise SystemExit(
                "Bridge connection failed; outstanding calls were not replayed."
            ) from None
        return

    # Import the compatibility entrypoint, including tools, before calling runner.main.
    from main import main as server_main

    server_main()
