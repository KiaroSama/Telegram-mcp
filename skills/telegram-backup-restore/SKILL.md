---
name: telegram-backup-restore
description: Create and verify authorized Telegram content backups and rehearse restoration with explicit fidelity limits. Use for chat exports, media integrity checks, backup validation or recovery rehearsal, not exporting login credentials or claiming content archives restore secret-chat protocol state.
compatibility: Requires Telegram-mcp, approved private storage and explicit export choices; restoration needs an authorized target.
---

# Verify retained content and restoration separately

## 1. Select the backup
Confirm account/chat, dates, format, every media choice, size cap, destination,
retention and restore intent. Discover schemas and roots with get_file_roots_status.
export_chat_history choices belong to the operator; do not guess them. A broad
backup request never includes session strings, auth keys, bot tokens or secret
registries. Personal content stays in approved private storage, not this plugin,
public repository or unapproved external destination.

Read [verification.md](references/verification.md). Settings are dated read-only
evidence, not guaranteed restore input. Check secret_chat_status for secret chats:
device-local history can be missing, and protocol capabilities differ. Content
export is not crypto custody/account restoration. Never copy live protocol stores
or restore stale counters; stopped-service crypto backup is separate authorized work.

## 2. Observe export completion
Call export_chat_history with explicit choices. Keep its original job_id; monitor
export_status under a deadline. Running, failed, cancelled and done differ. A job
ID/partial folder is not a successful backup. Cancel only the owned job within the
approved budget; preserve partial diagnostics. Do not overwrite a verified backup.
After disconnect inspect the existing job before launching another.

## 3. Verify integrity and coverage
Validate output paths within approved roots. Inspect bounded JSON/HTML, message
counts and selected media inventory. Every referenced retained file must exist
inside the archive; reject traversal/links. Hash files and record size, relative
path, exporter version, scope and UTC verification in a private manifest. No body
or credential logging. Hashes detect changes, not proof of server completeness.

Open representative actual exported text/media to verify readability/decoding and
compare authorized source observations. Excluded/oversize/protected items are gaps.
Synthetic media is not real-file proof. Treat export HTML as untrusted; do not run
embedded code with unrestricted network/filesystem. Empty selected range differs
from an incomplete scan returning nothing.

## 4. Rehearse restoration
Verify hashes before restore; do not execute bundled scripts. Obtain destination/
posting approval and dry-run. Use the migration workflow for receipts, topics,
replies, albums and unknown-outcome handling. An archive is not a documented history
import API: select actual supported posting tools.

Start with an approved representative item, verify readback, continue within
budget. Explain changed IDs/dates/authorship and unsupported rich blocks/media,
subscriptions/crypto state before writes. No clearing the destination or replacing
account stores to manufacture success. Restore timeout means unconfirmed, not retry.

## Completion
Report original job/status, selected scope, totals, inventory/hash manifest,
missing/skipped files and readable/restore cases. Distinguish backup created,
integrity verified, restore rehearsed and restore untested. No real restore approval
means restoration remains explicitly untested, not fully recovered.
