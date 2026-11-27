# Clone Blueprint Templates

Copy these tables into the clone workspace (for example `docs/clone/`) and fill them in during the phases of `SKILL.md`. Use stable IDs so the parity ledger can point back to every row. Keep raw captures in fixtures with private data removed.

## 1. Command Surface

| ID | Command | Scope and language | Source (`menu`, `observed`, `both`, `hidden-candidate`) | Reply screen ID | Notes |
| --- | --- | --- | --- | --- | --- |
| C01 | `/start` | default, all | both | S01 | |

## 2. Screen Inventory

One row per distinct screen the bot can show.

| ID | Title or purpose | Reached from (screen, action) | Role | Message kind (text, photo, document, album, sticker, poll, other) | Edited in place or new | Keyboard ID | Fixture file | Probe status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| S01 | Welcome | C01 | user | photo with caption | new | K01 | `fixtures/s01.json` | OBSERVED |

Probe status values: `OBSERVED`, `UNPROBED` (needs consent or access), `BLOCKED` (tool or access limit).

## 3. Exact Copy Table

Store text verbatim. Entity offsets and lengths are UTF-16 code units.

| Screen ID | Field (text, caption, alert, toast, prompt, error) | Exact text | Entities (type, offset, length, extra) | Custom emoji (ID as string, pack, substitute or none) | Parse notes (RTL marks, ZWNJ, non-BMP, spoilers, quotes, link preview) |
| --- | --- | --- | --- | --- | --- |
| S01 | caption | | | | |

## 4. Keyboard and Button Matrix

One row per button. Equal labels stay separate rows.

| Keyboard ID | Kind (inline, reply, remove, force-reply) | Row | Index | Label (raw) | Button kind (callback, url, web app, switch inline, copy, contact, location, disabled) | Style (primary, success, danger, default) | Icon ID (string) | Callback data or hash, URL, or app target | Result (screen ID or effect) | Needs consent |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| K01 | inline | 0 | 0 | | callback | default | none | | S02 | no |

Callback tokens expire. Record the data shape, not a reusable token. Re-inspect before every live press.

## 5. State-Transition Graph

| From state | Trigger (command, button, text, media, timeout) | Guard (role, input rule) | To state | Side effect | Reply (screen ID or error ID) |
| --- | --- | --- | --- | --- | --- |
| idle | `/start` | any | menu | create user record (hypothesis) | S01 |

Include the awaiting-input states, the cancel and back edges, and the timeout edges. Also list invalid-input rows, each with the exact error copy.

## 6. Function and Handler Map

| Handler | Responds to (command or callback pattern) | Reads | Writes | Calls out (observed or hypothesis) | Screens produced | Tests |
| --- | --- | --- | --- | --- | --- | --- |
| `start` | C01 | user store | user store | none | S01 | `start_sends_welcome` |

## 7. Data and Persistence Contract

| Entity | Fields | Source (observed or hypothesis) | Evidence or competing explanation and discriminating test | Retention |
| --- | --- | --- | --- | --- |
| user | telegram id, language, role | hypothesis | Language may come from the client setting or a stored choice. Test: change the client language and repeat `/start`. | |

Keep external integrations (payment providers, download backends, AI services, other APIs) in a second table. Each is `UNKNOWN` until the owner supplies evidence.

## 8. Dynamic-Field Comparison Rules

Decide per field how the original and the clone are compared.

| Field type | Rule |
| --- | --- |
| Static copy and entities | Exact match, including offsets and types |
| Custom emoji | Same ID, or an approved substitute listed in the Exact Copy table |
| Timestamps and dates | Compare format and timezone behavior, not the value |
| Counters and totals | Compare format; value must follow the same data |
| User names, mentions, IDs | Replace with placeholders on both sides, then match exactly |
| Random or generated IDs | Compare shape and length |
| Pagination | Same page size, same first and last page behavior |
| Message IDs | Ignore |
| File names and sizes | Exact for static assets; shape for generated files |
| Latency | Record only; flag a difference that changes the flow (for example a missing progress edit) |

Add project-specific rows as you meet new dynamic fields.

## 9. Parity Verification Ledger

| ID | Script step (action) | Role | Original result (fixture or capture) | Clone result | Compared on (text, entities, emoji, markup, edit-vs-new, alert, media, state) | Status (`MATCH`, `DIFF-FIXED`, `DIFF-ACCEPTED`, `UNPROBED`, `BLOCKED`) | Evidence | Owner note |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P001 | `/start` from a fresh chat | user | `fixtures/s01.json` | | | | | |

Sign-off block:

- Owner:
- Date:
- Rows by status:
- Known unprobed areas:
- Unknown backend areas:
