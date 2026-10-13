# Migration ledger
Private UTF-8 records with owner-only access and atomic replacement; no session data.
Header: version, account/source/destination, mode, fixed initial range, attribution,
media choices, topic map, approvals, budgets and checkpoint.
Entry: operation_id, source IDs, grouped_id, source topic/reply, destination IDs,
mapped topic/reply, fingerprint, planned/confirmed/unconfirmed, evidence/skip reason.
Fingerprints supplement IDs, never replace them.

planned -> call -> confirmed only after destination identity/readback. Missing
acknowledgement -> unconfirmed -> inspect/reconcile. Unique observed match can
confirm; no match does not prove failed send. Ambiguity remains stopped. No
exactly-once guarantee from a text match.

Synthetic example: album[41,42] spans pages; collect both. copied2 is not a mapping;
observe destination[81,82] before checkpoint42. Timeout on43 stops, never blind
resend. Reply44->41 needs destination81 and actual reply-capable route; otherwise
report/agree the gap before posting.

Sync: schedule/lifetime, ceilings, high-water mark, edit/conflict policy, worker
ownership and stop control. No deletion propagation. Receipts grant no authority.
Sources: https://core.telegram.org/api/messages,
https://core.telegram.org/api/forward, https://core.telegram.org/api/forum.
