# Read-only audit matrix
Identity: list_accounts/get_me, explicit label.
Chat settings: get_full_chat, missing fields unknown/not applicable.
Admin rights: get_admins/get_member_admin_status, chat-specific semantics.
Members/exceptions: scoped readers, page/visibility limits.
Privacy: get_privacy_settings, selected keys, preserve rule order.
Devices: list_authorizations, eligibility versus acceptance, no termination.
Invites: list/get readers, private links redacted, no creation/revocation.
Server roots/safeguard: only expressly selected scope.

Evidence: check ID, target, UTC, actual tool/field, observed value, intended policy,
confirmed difference/unknown, consequence, recommendation, visibility gap.
No access hashes or signed URLs. Synthetic truncated participants page is partial,
not no members. Device acceptance is not eligibility proof. Intentional contacts-
only privacy is described, not silently changed to nobody.
Sources: https://core.telegram.org/api/rights,
https://core.telegram.org/api/privacy, https://core.telegram.org/api/invites,
https://core.telegram.org/api/auth.
