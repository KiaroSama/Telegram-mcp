"""Read-only capability diagnostics; availability never authorizes a cutover."""


def readiness():
    from telegram_mcp import secret_backend as backend

    available = callable(getattr(backend, "StrongSecretChatManager", None))
    verified = False
    if available:
        try:
            backend.verify_ingress_prerequisite()
            verified = True
        except Exception:
            pass  # Only fixed diagnostics leave this boundary, never source paths.
    blockers = [
        "strong_lifecycle_not_integrated",
        "archive_off_cutover_not_approved",
        "verified_cutover_baseline_required",
        "protected_key_and_binding_required",
    ]
    if not available:
        blockers.append("strong_library_unavailable")
    if not verified:
        blockers.append("patched_telethon_prerequisite_unverified")
    return {
        "library_available": available,
        "integrated": False,
        "active": False,
        "activation_ready": False,
        "prerequisite_verified": verified,
        "requires_package_archive_off": True,
        "backpressure_scope": "all same-account RPCs",
        "blockers": blockers,
    }
