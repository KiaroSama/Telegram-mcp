# Secret-chat backend integration is deferred

The owner explicitly removed secret-chat rewriting and connection from the current
Rust migration scope while separately rewriting `Telethon-Secret-Chat` in Rust.
This branch does not import the Python package, advertise working encrypted chats or
replace its cryptographic protocol with a cloud-chat request.

Preserve every existing private key store, counter, pending receipt and archive until
the new backend's reviewed migration contract is available. A grammers authorization
session is not a backup of secret-chat protocol state; re-login cannot regenerate
lost one-time media keys or chat keys.

The native custody gate blocks imported/legacy authorization associated with known
owned secret-chat sidecars before network startup. grammers full differences can
implicitly acknowledge qts without a ReceivedQueue call, so disabled tools alone do
not preserve pending encrypted updates. Use a fresh separate native authorization
for ordinary chats while this backend is deferred. Files stay intact, and the gate
makes no claim to detect unknown external stores.

When the owner declares the Rust backend ready, review its current source and tests,
pin an exact compatible revision and connect it through a dedicated adapter. The
integration must establish account identity, key/counter custody, restart recovery,
update acknowledgment, cancellation/unknown-send outcomes, archive/media references,
peer deletion semantics, bounded ingress and owner approvals. Restore corresponding
native tool schemas and behavioral tests only when real behavior is implemented.
Do not re-enable a tool because its old name exists in a catalogue.

The former backend was
[KiaroSama/Telethon-Secret-Chat](https://github.com/KiaroSama/Telethon-Secret-Chat).
Its historical Python revision is recorded in the pre-migration Git history; it is
reference material, not a runtime dependency of this Rust branch.
