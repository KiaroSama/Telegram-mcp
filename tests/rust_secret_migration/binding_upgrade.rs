//! Authentic encrypted-v1 fixtures for the public custody upgrade boundary.
//! Keep this module under rust_secret_migration so it can reuse its parent fixtures.

use super::*;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use kiaro_secret_chat::{
    binding_migration::{LegacyAuthorizationBinding, migrate_v1_enc_store_to_v2},
    custody::IngressLimits,
    ingress::DurableIngress,
    storage::{FileStorage, SecretRecord, State as StoredState, StoreMode},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

const MAX_STORE_BYTES: usize = 64 * 1024 * 1024;
#[derive(Debug, PartialEq, Eq)]
enum BackupEntry {
    Directory,
    File { bytes: Vec<u8>, readonly: bool },
    Symlink(PathBuf),
}

type FileSnapshot = BTreeMap<PathBuf, BackupEntry>;
type ProtectedSnapshot = BTreeMap<&'static str, Option<(Vec<u8>, bool)>>;

struct HistoricalFixture {
    directory: tempfile::TempDir,
    old: LegacyAuthorizationBinding,
    key: [u8; 32],
    expected: StoredState,
    ciphertext_v1: Vec<u8>,
}

fn lowercase_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").unwrap();
    }
    result
}

impl HistoricalFixture {
    fn new() -> Self {
        let secret_key = [0x5a; 256];
        let fingerprint = kiaro_secret_chat::crypto::SharedKey::from_slice(&secret_key)
            .unwrap()
            .fingerprint();
        let (directory, _) = fixture(&json!({
            "chats": {"13": {
                "id": 13, "access_hash": 31, "peer_user_id": 43,
                "is_outbound": true, "state": "ready", "ttl": 10,
                "layer": 73, "wrapper_layer": 46,
                "in_seq_no": 4, "out_seq_no": 5, "peer_in_seq_no": 3,
                "key": {"__bytes__": "5a".repeat(secret_key.len())},
                "key_fingerprint": fingerprint
            }},
            "in": {}, "out": {},
            "settings": {"fixture": {"enabled": true}, "auto_save": {"on": false}}
        }));
        let custody = directory.path().join("secret-chats");
        fs::write(
            custody.join(format!("{LABEL}-history.json")),
            br#"{"13":[{"message_id":321,"text":"retained fixture history"}]}"#,
        )
        .unwrap();
        fs::write(custody.join(format!("{LABEL}-media.json")), br#"{"13":{}}"#).unwrap();
        fs::write(
            custody.join(format!("{LABEL}.owner.json")),
            br#"{"user_id":17}"#,
        )
        .unwrap();

        // Reuse the production legacy backup, sidecars, and storage key.
        let store = migrate_legacy(directory.path(), LABEL, &binding())
            .unwrap()
            .unwrap();
        let mut expected = store.load().unwrap().unwrap();
        drop(store);
        let key: [u8; 32] = fs::read(custody.join(format!("{LABEL}.store.key")))
            .unwrap()
            .try_into()
            .unwrap();
        let auth_id = i64::from_le_bytes(sha1::Sha1::digest(AUTH)[12..].try_into().unwrap());
        let old = LegacyAuthorizationBinding::from_session_parts(17, auth_id, 2, "migration-test")
            .unwrap();
        expected.authorization = old.fingerprint();
        install_journal(&mut expected);
        expected.validate().unwrap();

        // Encode the historical format explicitly. Current v2 storage cannot
        // construct an old-binding snapshot, so reopening v2 is not a substitute.
        let nonce = [0x73; 12];
        let aad = [
            b"KIARO-INGRESS-RUST-1\0".as_slice(),
            expected.authorization.as_bytes(),
        ]
        .concat();
        let plaintext = serde_json::to_vec(&expected).unwrap();
        let ciphertext = Aes256Gcm::new_from_slice(&key)
            .unwrap()
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .unwrap();
        let mut outer = StoredState::empty(old.fingerprint()).unwrap();
        outer.mode = StoreMode::Ingress;
        outer.private_records.insert(
            "aead/ingress-v1".into(),
            SecretRecord::new([nonce.as_slice(), ciphertext.as_slice()].concat()),
        );
        let encrypted = custody.join(format!("{LABEL}.v1.enc"));
        {
            let mut owner = FileStorage::open(&encrypted).unwrap();
            owner.commit(&outer).unwrap();
        }
        let receipt = custody.join(format!("{LABEL}.migration.json"));
        let mut saved: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
        saved["authorization"] = json!(old.fingerprint());
        fs::write(&receipt, serde_json::to_vec_pretty(&saved).unwrap()).unwrap();
        let ciphertext_v1 = fs::read(encrypted).unwrap();
        Self {
            directory,
            old,
            key,
            expected,
            ciphertext_v1,
        }
    }

    fn custody(&self) -> PathBuf {
        self.directory.path().join("secret-chats")
    }

    fn encrypted(&self) -> PathBuf {
        self.custody().join(format!("{LABEL}.v1.enc"))
    }

    fn receipt(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.custody().join(format!("{LABEL}.migration.json"))).unwrap(),
        )
        .unwrap()
    }

    fn binding_backup(&self) -> PathBuf {
        self.custody().join("legacy-backups").join(format!(
            "{LABEL}-binding-v1-{}.enc",
            lowercase_hex(&Sha256::digest(&self.ciphertext_v1))
        ))
    }

    fn open(&self) -> anyhow::Result<Option<EncryptedIngressStorage>> {
        telegram_mcp::secret::open_store_with_legacy(
            self.directory.path(),
            LABEL,
            &binding(),
            Some(&self.old),
            true,
        )
    }

    fn expected_v2(&self) -> StoredState {
        let mut expected = self.expected.clone();
        expected.authorization = binding().digest();
        expected
    }

    fn assert_state(&self, store: &EncryptedIngressStorage) {
        assert_eq!(
            serde_json::to_value(store.load().unwrap().unwrap()).unwrap(),
            serde_json::to_value(self.expected_v2()).unwrap(),
            "keys, counters, settings, custody controls, and pending ingress must survive"
        );
    }

    fn protected_snapshot(&self) -> ProtectedSnapshot {
        [
            "",
            ".v1.enc",
            ".store.key",
            ".migration.json",
            "-history.json",
            "-media.json",
            ".owner.json",
            "-consumer.json",
            ".history.v1.json",
            ".media.v1.json",
            ".archive.v1.json",
        ]
        .into_iter()
        .map(|suffix| {
            let path = self.custody().join(format!("{LABEL}{suffix}"));
            let value = match fs::read(&path) {
                Ok(bytes) => Some((bytes, fs::metadata(path).unwrap().permissions().readonly())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("Cannot snapshot fixture sidecar: {error}"),
            };
            (suffix, value)
        })
        .collect()
    }

    fn backup_snapshot(&self) -> FileSnapshot {
        fn visit(base: &Path, current: &Path, files: &mut FileSnapshot) {
            for entry in fs::read_dir(current).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                let relative = path.strip_prefix(base).unwrap().to_path_buf();
                if kind.is_dir() {
                    files.insert(relative, BackupEntry::Directory);
                    visit(base, &path, files);
                } else if kind.is_symlink() {
                    files.insert(relative, BackupEntry::Symlink(fs::read_link(path).unwrap()));
                } else {
                    assert!(kind.is_file(), "Unexpected fixture backup entry type");
                    files.insert(
                        relative,
                        BackupEntry::File {
                            bytes: fs::read(&path).unwrap(),
                            readonly: fs::metadata(path).unwrap().permissions().readonly(),
                        },
                    );
                }
            }
        }
        let base = self.custody().join("legacy-backups");
        let mut files = FileSnapshot::new();
        visit(&base, &base, &mut files);
        files
    }

    fn pending_consumer(&self) {
        fs::write(
            self.custody().join(format!("{LABEL}-consumer.json")),
            serde_json::to_vec(&json!({
                "version": 1,
                "chats": {"13": {"0": {
                    "seq_no": 0, "committed_at": 1700000000,
                    "message": {
                        "message_id": 501, "text": "pending fixture receipt",
                        "can_be_saved": true, "self_destructs_after_seconds": 0
                    },
                    "media_reference": null
                }}}
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

// Windows does not remove readonly files when TempDir is dropped.
#[cfg(windows)]
impl Drop for HistoricalFixture {
    fn drop(&mut self) {
        fn writable_files(directory: &Path) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    writable_files(&entry.path());
                } else if kind.is_file()
                    && let Ok(metadata) = entry.metadata()
                {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(false);
                    let _ = fs::set_permissions(entry.path(), permissions);
                }
            }
        }
        writable_files(&self.custody().join("legacy-backups"));
    }
}

fn install_journal(state: &mut StoredState) {
    use grammers_session::updates::{MessageBoxes, UpdatesLike, durable};
    use grammers_tl_types::types;

    let mut boxes = MessageBoxes::new();
    boxes.set_state(types::updates::State {
        pts: 10,
        qts: 40,
        date: 123,
        seq: 1,
        unread_count: 0,
    });
    let checkpoint = boxes.durable_checkpoint().unwrap();
    let receipt = durable::encode_live(&UpdatesLike::Updates(
        types::Updates {
            updates: vec![
                types::UpdateNewEncryptedMessage {
                    message: types::EncryptedMessageService {
                        random_id: 41,
                        chat_id: 13,
                        date: 124,
                        bytes: vec![1, 2, 3],
                    }
                    .into(),
                    qts: 41,
                }
                .into(),
            ],
            users: vec![],
            chats: vec![],
            date: 124,
            seq: 2,
        }
        .into(),
    ))
    .unwrap();
    let ledger = json!({
        "version": 1, "checkpoint": lowercase_hex(&checkpoint),
        "receipt_frontier": 40, "next_id": 2, "batches": [], "paused": false,
        "inputs": [{"id": 1, "envelope": lowercase_hex(&receipt), "created_ms": 1}]
    });
    state.private_records.insert(
        "ingress/journal-v1".into(),
        SecretRecord::new(serde_json::to_vec(&ledger).unwrap()),
    );
}

fn assert_original_backups_retained(fixture: &HistoricalFixture, before: &FileSnapshot) {
    let after = fixture.backup_snapshot();
    for (path, expected) in before {
        assert_eq!(after.get(path), Some(expected));
    }
}

#[test]
fn authentic_v1_binding_upgrade_preserves_state_receipt_and_readonly_ciphertext_backup() {
    let fixture = HistoricalFixture::new();
    let prior_receipt = fixture.receipt();
    let prior_backups = fixture.backup_snapshot();
    let store = fixture.open().unwrap().unwrap();
    fixture.assert_state(&store);
    let limits = IngressLimits::new(4096, MAX_STORE_BYTES, 30.0 * 86400.0).unwrap();
    let ingress = DurableIngress::new(store, limits).unwrap();
    let boxes = grammers_session::updates::MessageBoxes::from_durable_checkpoint(
        &ingress.checkpoint().unwrap(),
    )
    .unwrap();
    assert_eq!(boxes.session_state().qts, 40);
    drop(ingress);

    let receipt = fixture.receipt();
    assert_eq!(receipt["authorization"], binding().digest());
    for field in ["version", "backup", "sha256"] {
        assert_eq!(receipt[field], prior_receipt[field]);
    }
    assert_original_backups_retained(&fixture, &prior_backups);
    let backup = fixture.binding_backup();
    assert_eq!(fs::read(&backup).unwrap(), fixture.ciphertext_v1);
    assert!(fs::metadata(backup).unwrap().permissions().readonly());
    let after_backups = fixture.backup_snapshot();
    assert_eq!(after_backups.len(), prior_backups.len() + 1);
    assert!(
        after_backups
            .keys()
            .all(|path| !path.to_string_lossy().ends_with(".store.key"))
    );
    let current = fs::read(fixture.encrypted()).unwrap();
    assert_ne!(current, fixture.ciphertext_v1);
    let protected = fixture.protected_snapshot();
    let reopened = fixture.open().unwrap().unwrap();
    fixture.assert_state(&reopened);
    drop(reopened);
    assert_eq!(fixture.protected_snapshot(), protected);
    assert_eq!(fixture.backup_snapshot(), after_backups);
}

#[test]
fn v1_binding_upgrade_preflights_backup_and_pending_sidecars_before_mutation() {
    for invalid in [
        "backup_digest",
        "consumer",
        ".history.v1.json",
        ".media.v1.json",
        ".archive.v1.json",
    ] {
        let fixture = HistoricalFixture::new();
        if invalid == "backup_digest" {
            let mut receipt = fixture.receipt();
            receipt["sha256"][LABEL] = json!("00".repeat(32));
            fs::write(
                fixture.custody().join(format!("{LABEL}.migration.json")),
                serde_json::to_vec_pretty(&receipt).unwrap(),
            )
            .unwrap();
        } else {
            fixture.pending_consumer();
            let suffix = if invalid == "consumer" {
                "-consumer.json"
            } else {
                invalid
            };
            fs::write(
                fixture.custody().join(format!("{LABEL}{suffix}")),
                b"{ invalid fixture JSON",
            )
            .unwrap();
        }
        let protected = fixture.protected_snapshot();
        let backups = fixture.backup_snapshot();
        assert!(fixture.open().is_err(), "{invalid} must refuse");
        assert_eq!(
            fixture.protected_snapshot(),
            protected,
            "{invalid} changed original custody or sidecars before refusal"
        );
        assert_eq!(
            fixture.backup_snapshot(),
            backups,
            "{invalid} changed the backup inventory before refusal"
        );
    }
}

#[test]
fn v1_binding_upgrade_refuses_wrong_proof_key_active_owner_and_corruption() {
    for invalid in [
        "binding",
        "new_metadata",
        "key",
        "active_owner",
        "ciphertext",
        "outer_mode",
    ] {
        let fixture = HistoricalFixture::new();
        let wrong =
            LegacyAuthorizationBinding::from_session_parts(17, 42, 2, "migration-test").unwrap();
        assert_ne!(wrong.fingerprint(), fixture.old.fingerprint());
        let requested = if invalid == "new_metadata" {
            // The historical receipt and outer binding remain valid, so this
            // reaches the upstream proof check instead of receipt rejection.
            AuthorizationBinding::v2(
                17,
                3,
                telegram_mcp::secret::auth_key_tag(&AUTH),
                "migration-test",
            )
            .unwrap()
        } else {
            binding()
        };
        if invalid == "key" {
            let mut wrong_key = fixture.key;
            wrong_key[0] ^= 1;
            fs::write(
                fixture.custody().join(format!("{LABEL}.store.key")),
                wrong_key,
            )
            .unwrap();
        }
        if matches!(invalid, "ciphertext" | "outer_mode") {
            let mut owner = FileStorage::open(fixture.encrypted()).unwrap();
            let mut outer = owner.load().unwrap().unwrap();
            if invalid == "outer_mode" {
                outer.mode = StoreMode::Durable;
            } else {
                let record = outer.private_records.get_mut("aead/ingress-v1").unwrap();
                let mut blob = record.as_bytes().to_vec();
                blob[12] ^= 1;
                *record = SecretRecord::new(blob);
            }
            owner.commit(&outer).unwrap();
        }
        let owner =
            (invalid == "active_owner").then(|| FileStorage::open(fixture.encrypted()).unwrap());
        let protected = fixture.protected_snapshot();
        let backups = fixture.backup_snapshot();
        let result = telegram_mcp::secret::open_store_with_legacy(
            fixture.directory.path(),
            LABEL,
            &requested,
            Some(if invalid == "binding" {
                &wrong
            } else {
                &fixture.old
            }),
            true,
        );
        assert!(result.is_err(), "{invalid} must refuse");
        if invalid == "new_metadata" {
            let error = result.err().unwrap();
            assert!(matches!(
                error.downcast_ref::<kiaro_secret_chat::errors::SecretChatError>(),
                Some(kiaro_secret_chat::errors::SecretChatError::StoreCorrupt { reason, .. })
                    if reason == "historical and current authorization metadata do not match"
            ));
        }
        assert_eq!(fixture.protected_snapshot(), protected, "{invalid}");
        assert_eq!(
            fixture.backup_snapshot(),
            backups,
            "{invalid} changed the backup inventory before refusal"
        );
        drop(owner);
    }
}

#[test]
fn interrupted_v2_store_with_v1_receipt_recovers_without_reencrypting_or_duplicate_backups() {
    for existing_binding_backup in [false, true] {
        let fixture = HistoricalFixture::new();
        if existing_binding_backup {
            let backup = fixture.binding_backup();
            fs::write(&backup, &fixture.ciphertext_v1).unwrap();
            let mut permissions = fs::metadata(&backup).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(backup, permissions).unwrap();
        }
        migrate_v1_enc_store_to_v2(
            &fixture.encrypted(),
            fixture.key,
            &fixture.old,
            &binding(),
            MAX_STORE_BYTES,
        )
        .unwrap();
        assert_eq!(
            fixture.receipt()["authorization"],
            fixture.old.fingerprint()
        );
        let current = fs::read(fixture.encrypted()).unwrap();
        let backups = fixture.backup_snapshot();
        for _ in 0..2 {
            let store = fixture.open().unwrap().unwrap();
            fixture.assert_state(&store);
            drop(store);
            assert_eq!(fixture.receipt()["authorization"], binding().digest());
            assert_eq!(fs::read(fixture.encrypted()).unwrap(), current);
            assert_eq!(fixture.backup_snapshot(), backups);
        }
    }
}

#[test]
fn v1_binding_backup_recovery_reuses_exact_files_and_refuses_conflicting_entries() {
    for readonly in [false, true] {
        let fixture = HistoricalFixture::new();
        let backup = fixture.binding_backup();
        fs::write(&backup, &fixture.ciphertext_v1).unwrap();
        if readonly {
            let mut permissions = fs::metadata(&backup).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&backup, permissions).unwrap();
        }
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().readonly(),
            readonly
        );
        let mut expected_backups = fixture.backup_snapshot();
        let relative = backup
            .strip_prefix(fixture.custody().join("legacy-backups"))
            .unwrap()
            .to_path_buf();
        expected_backups.insert(
            relative,
            BackupEntry::File {
                bytes: fixture.ciphertext_v1.clone(),
                readonly: true,
            },
        );
        let store = fixture.open().unwrap().unwrap();
        fixture.assert_state(&store);
        drop(store);
        assert_eq!(fixture.receipt()["authorization"], binding().digest());
        assert_eq!(fixture.backup_snapshot(), expected_backups);
        let upgraded = fs::read(fixture.encrypted()).unwrap();
        assert_ne!(upgraded, fixture.ciphertext_v1);
        let protected = fixture.protected_snapshot();
        let reopened = fixture.open().unwrap().unwrap();
        fixture.assert_state(&reopened);
        drop(reopened);
        assert_eq!(fixture.protected_snapshot(), protected);
        assert_eq!(fixture.backup_snapshot(), expected_backups);
    }

    for conflict in ["bytes", "directory"] {
        let fixture = HistoricalFixture::new();
        let backup = fixture.binding_backup();
        if conflict == "bytes" {
            fs::write(backup, b"conflicting fixture backup").unwrap();
        } else {
            fs::create_dir(backup).unwrap();
        }
        let protected = fixture.protected_snapshot();
        let backups = fixture.backup_snapshot();
        assert!(fixture.open().is_err(), "{conflict} backup must refuse");
        assert_eq!(fixture.protected_snapshot(), protected, "{conflict}");
        assert_eq!(fixture.backup_snapshot(), backups, "{conflict}");
    }

    #[cfg(unix)]
    {
        let fixture = HistoricalFixture::new();
        let target = fixture.directory.path().join("unrelated-backup-target");
        fs::write(&target, &fixture.ciphertext_v1).unwrap();
        std::os::unix::fs::symlink(&target, fixture.binding_backup()).unwrap();
        let protected = fixture.protected_snapshot();
        let backups = fixture.backup_snapshot();
        assert!(fixture.open().is_err(), "Symlink backup must refuse");
        assert_eq!(fixture.protected_snapshot(), protected);
        assert_eq!(fixture.backup_snapshot(), backups);
        assert_eq!(fs::read(target).unwrap(), fixture.ciphertext_v1);
    }
}
