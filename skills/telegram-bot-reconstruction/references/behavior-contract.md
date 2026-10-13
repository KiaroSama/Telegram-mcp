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

## Commands and visible output
Command case: actor (bot/user account) | chat type | role | exact ready-to-send
form/prefix and arguments | precondition | permission scope | response/edit |
effect | readback | cleanup | outcome. Include denied roles, missing/invalid args,
unknown command, cancel/back, repeat and restart; do not invent unsupported cases.
Approval is per effect, not per menu label. Destructive admin/selfbot cases remain
fixture-only unless a disposable owned target and that exact action are approved.
Keep session export, device cloning, payment and bypass outside these cases.

Output case: text_fidelity + full entities/link targets | media/caption | keyboard
rows/indices/raw labels/kinds | style.background/style.icon_document_id | URL destination |
source evidence | dynamic exceptions. New callbacks are independently implemented.
Inspect current token before any approved press; stale labels do not bind payloads.
Message typography is supported entities, not font-family/CSS. Native glass styles:
primary/blue, success/green, danger/red; omission means app-specific default. Exact
pixels require client/version/theme/viewport evidence, not just matching enums.

## Downloader hypothesis ledger
observation | candidate mechanism | alternative | counterevidence | approved
validation case | status (UNKNOWN until supported). Examples, not target facts:
- Queued/progress text: worker queue vs immediate work with staged UI; controlled
  sequential timing alone cannot prove parallel workers or a queue implementation.
- Delivered media: extractor/download vs relay of existing media; MP4 can be source
  format, remux or transcode. Inspect permitted output metadata, never protected input.
- Login prompt/failure: provider auth vs bot role gate; use supplied error evidence,
  not credential extraction, private endpoint probes or bypass.
- Faster repeat: cache vs warm worker/network/source CDN; require discriminating
  evidence before a cache claim. Never infer cache key/retention from one request.
- Delay/retry-after: bot throttle vs Telegram/provider limits; keep scope/time/effect
  unknown without evidence. Respect observed retry-after; no load test by default.
Cancellation reply proves UI state only; verify the authorized job/effect separately.
A maintained public downloader is a design reference, not this target's stack.

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
