# Observation and parity contract
Execution records belong in an approved private destination, not this plugin or
public fixtures. No tokens, initData or signed launch URLs. Sanitize reusable cases.

## Scope
Source, account, authorization/asset rights, independent identity/workspace,
languages, allowed interactions, prohibited effects, observation/time ceiling,
agreed commands/keyboards/screens, available devices.

## Cases
case_id | source state | input/entry point | button kind | expected text/entities/
media/keyboard | next state | effect | evidence | observed/inferred/untested/
inaccessible | verification result. Include invalid input and cancellation.
Example synthetic B01: fresh -> /start -> two callback rows -> home. B02:
home -> Catalog -> empty list. B03: invalid selection rejected without changing
another user. New implementation owns its callbacks; do not replay source payloads.

## Mini App
Launch mode, viewport/theme, typography/computed spacing, responsive/safe-area
layout, keyboard/back, validation, loading/empty/error, supported SDK features,
bot handoff, asset rights. Missing backend knowledge is unknown, not fake data.
Independent backend: current signature validation, auth_date freshness, user/
session binding, input authorization, idempotent effects and expired/invalid
credential tests. Token-based HMAC and third-party Ed25519 modes are distinct;
verify the chosen algorithm in current official docs.

## Acceptance
Case | source evidence | implementation | execution evidence | verified/mismatch/
untested | reason. Keep failed cases visible. Coverage refers only to agreed cases.
Same screenshot conditions; DOM presence alone does not prove visual parity.
Native Telegram behavior requires actual native-client/device evidence.

Sources: https://core.telegram.org/bots/api,
https://core.telegram.org/api/bots,
https://core.telegram.org/bots/webapps#validating-data-received-via-the-mini-app.
Refresh for the chosen target version before implementation.
