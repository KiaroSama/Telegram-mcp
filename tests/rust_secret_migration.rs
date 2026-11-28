use kiaro_secret_chat::{
    binding::AuthorizationBinding,
    custody::{CustodyLimits, CustodyStorage},
    ingress::EncryptedIngressStorage,
    storage::Storage,
};
use serde_json::{Value, json};
use std::{fs, path::Path};
use telegram_mcp::secret::migrate_legacy;

#[path = "rust_secret_migration/binding_upgrade.rs"]
mod binding_upgrade;

const LABEL: &str = "offline";

const AUTH: [u8; 256] = [0x3c; 256];

fn binding() -> AuthorizationBinding {
    AuthorizationBinding::v2(
        17,
        2,
        telegram_mcp::secret::auth_key_tag(&AUTH),
        "migration-test",
    )
    .unwrap()
}

fn fixture(legacy: &Value) -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let bytes = serde_json::to_vec_pretty(legacy).unwrap();
    let custody = dir.path().join("secret-chats");
    fs::create_dir(&custody).unwrap();
    fs::write(custody.join(LABEL), &bytes).unwrap();
    (dir, bytes)
}

fn backups(dir: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(dir.join("legacy-backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[cfg(windows)]
fn restore_fixture_permissions(backup: &Path, original: &Path) {
    fs::set_permissions(backup, fs::metadata(original).unwrap().permissions()).unwrap();
}

#[test]
fn consumer_receipts_are_validated_before_publication_and_deduplicated_on_restart() {
    let (dir, original) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let custody = dir.path().join("secret-chats");
    let source = custody.join("offline-consumer.json");
    let receipt = |seq, id, ttl, save| json!({"seq_no":seq,"message":{"message_id":id,"can_be_saved":save,"self_destructs_after_seconds":ttl,"text":"fixture","private":"never exposed"},"media_reference":null,"committed_at":100+seq});
    let mut consumer = json!({"version":1,"chats":{"12":{"0":receipt(0,42,0,true),"1":receipt(1,42,0,true),"2":receipt(2,43,1,true),"3":receipt(4,44,0,false)}}});
    use kiaro_secret_chat::{
        files::FileKey,
        media::{DocumentMediaInput, MediaKind, MediaReference, Metadata, document_media},
    };
    let key = FileKey::generate().unwrap();
    let media = document_media(DocumentMediaInput {
        kind: MediaKind::Document,
        name: "sample",
        mime: "application/octet-stream",
        size: 16,
        layer: 143,
        key: &key,
        caption: "",
        metadata: &Metadata::default(),
    })
    .unwrap();
    let file = grammers_tl_types::types::EncryptedFile {
        id: 441,
        access_hash: 99,
        size: 16,
        dc_id: 2,
        key_fingerprint: key.fingerprint(),
    };
    let media = MediaReference::new(12, &file, &media)
        .unwrap()
        .to_json()
        .unwrap();
    for seq in ["0", "1", "2", "3"] {
        consumer["chats"]["12"][seq]["media_reference"] = media.clone();
    }
    let broken = serde_json::to_vec_pretty(&consumer).unwrap();
    fs::write(&source, &broken).unwrap();
    assert!(migrate_legacy(dir.path(), LABEL, &binding()).is_err());
    assert_eq!(fs::read(&source).unwrap(), broken);
    assert_eq!(fs::read(custody.join(LABEL)).unwrap(), original);
    assert!(!custody.join("offline.v1.enc").exists());
    consumer["chats"]["12"]["3"]["seq_no"] = json!(3);
    consumer["chats"]["12"]["0"]["media_reference"]["chat_id"] = json!(56);
    fs::write(&source, consumer.to_string()).unwrap();
    assert!(migrate_legacy(dir.path(), LABEL, &binding()).is_err());
    consumer["chats"]["12"]["0"]["media_reference"] = media.clone();
    let valid = serde_json::to_vec(&consumer).unwrap();
    fs::write(&source, &valid).unwrap();
    for _ in 0..2 {
        drop(migrate_legacy(dir.path(), LABEL, &binding()).unwrap());
        let read = |suffix: &str| -> Value {
            serde_json::from_slice(&fs::read(custody.join(format!("offline{suffix}"))).unwrap())
                .unwrap()
        };
        let history = read(".history.v1.json");
        assert_eq!(history["12"].as_array().unwrap().len(), 1);
        assert_eq!(history["12"][0]["message_id"], 42);
        assert!(history["12"][0].get("private").is_none());
        let refs = read(".media.v1.json");
        assert_eq!(refs["12"].as_object().unwrap().len(), 1);
        assert_eq!(refs["12"]["42"]["reference"], media);
        let receipts = read(".archive.v1.json");
        assert_eq!(receipts["delivery_receipts"]["12:0"], 42);
        assert_eq!(receipts["delivery_receipts"]["12:1"], 42);
        assert_eq!(fs::read(&source).unwrap(), valid);
    }
    #[cfg(windows)]
    for entry in fs::read_dir(&backups(&custody)[0]).unwrap().flatten() {
        restore_fixture_permissions(&entry.path(), &custody.join(entry.file_name()));
    }
}

#[test]
fn earlier_native_migration_imports_consumer_sidecar_once_without_replacing_newer_rows() {
    let (dir, _) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let custody = dir.path().join("secret-chats");
    drop(migrate_legacy(dir.path(), LABEL, &binding()).unwrap());
    let history = custody.join("offline.history.v1.json");
    fs::write(
        &history,
        br#"{"12":[{"message_id":99,"text":"newer native row","date":"2026-01-01T00:00:00Z"}]}"#,
    )
    .unwrap();
    let legacy=br#"{"version":1,"chats":{"12":{"0":{"seq_no":0,"committed_at":1546800000,"message":{"message_id":42,"text":"from legacy"},"media_reference":null}}}}"#;
    fs::write(custody.join("offline-consumer.json"), legacy).unwrap();
    let conflict = custody.join("offline.media.v1.json");
    fs::write(&conflict, b"invalid").unwrap();
    let original_history = fs::read(&history).unwrap();
    let manifest = custody.join("offline.migration.json");
    let original_manifest = fs::read(&manifest).unwrap();
    assert!(migrate_legacy(dir.path(), LABEL, &binding()).is_err());
    assert_eq!(fs::read(&history).unwrap(), original_history);
    assert_eq!(fs::read(manifest).unwrap(), original_manifest);
    fs::remove_file(conflict).unwrap();
    for _ in 0..2 {
        drop(migrate_legacy(dir.path(), LABEL, &binding()).unwrap());
        let body: Value = serde_json::from_slice(&fs::read(&history).unwrap()).unwrap();
        assert_eq!(body["12"].as_array().unwrap().len(), 2);
        assert_eq!(body["12"][0]["message_id"], 42);
        assert_eq!(body["12"][1]["message_id"], 99);
    }
    #[cfg(windows)]
    for entry in fs::read_dir(&backups(&custody)[0]).unwrap().flatten() {
        restore_fixture_permissions(&entry.path(), &custody.join(entry.file_name()));
    }
}

#[test]
fn consumer_upgrade_preserves_existing_native_rows_with_the_same_message_id() {
    let (dir, _) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let custody = dir.path().join("secret-chats");
    drop(migrate_legacy(dir.path(), LABEL, &binding()).unwrap());
    let history = custody.join("offline.history.v1.json");
    let native =
        json!({"message_id":42,"text":"retained native row","date":"2026-01-01T00:00:00Z"});
    fs::write(&history, json!({"12":[native.clone()]}).to_string()).unwrap();
    fs::write(custody.join("offline-consumer.json"), br#"{"version":1,"chats":{"12":{"0":{"seq_no":0,"committed_at":1546800000,"message":{"message_id":42,"text":"older legacy copy"},"media_reference":null}}}}"#).unwrap();
    for _ in 0..2 {
        drop(migrate_legacy(dir.path(), LABEL, &binding()).unwrap());
        let body: Value = serde_json::from_slice(&fs::read(&history).unwrap()).unwrap();
        assert_eq!(body["12"], json!([native.clone()]));
        let receipts: Value =
            serde_json::from_slice(&fs::read(custody.join("offline.archive.v1.json")).unwrap())
                .unwrap();
        assert_eq!(receipts["delivery_receipts"]["12:0"], 42);
    }
    #[cfg(windows)]
    for entry in fs::read_dir(&backups(&custody)[0]).unwrap().flatten() {
        restore_fixture_permissions(&entry.path(), &custody.join(entry.file_name()));
    }
}

#[test]
fn empty_store_migrates_once_with_a_readonly_byte_identical_backup() {
    let (dir, bytes) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let custody = dir.path().join("secret-chats");
    let store = migrate_legacy(dir.path(), LABEL, &binding())
        .unwrap()
        .unwrap();
    let state = store.load().unwrap().unwrap();
    assert_eq!(state.version, 1);
    assert!(state.chats.is_empty());
    drop(store);
    let before = backups(&custody);
    assert_eq!(before.len(), 1);
    let backup = before[0].join(LABEL);
    assert_eq!(fs::read(&backup).unwrap(), bytes);
    assert!(fs::metadata(&backup).unwrap().permissions().readonly());
    assert_eq!(fs::read(custody.join(LABEL)).unwrap(), bytes);
    let encrypted = fs::read(custody.join("offline.v1.enc")).unwrap();
    drop(
        migrate_legacy(dir.path(), LABEL, &binding())
            .unwrap()
            .unwrap(),
    );
    assert_eq!(backups(&custody), before);
    assert_eq!(fs::read(custody.join("offline.v1.enc")).unwrap(), encrypted);
    #[cfg(windows)]
    restore_fixture_permissions(&backup, &custody.join(LABEL));
}

#[test]
fn renamed_migrated_account_reopens_without_changing_historical_backup() {
    use telegram_mcp::{account_manager::rename_account, state::State};

    let (dir, _) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{"fixture":true}}));
    let custody = dir.path().join("secret-chats");
    fs::rename(custody.join(LABEL), custody.join("old_label")).unwrap();
    for suffix in ["-history.json", "-media.json"] {
        fs::write(custody.join(format!("old_label{suffix}")), b"{}").unwrap();
    }
    let binding = binding();
    drop(
        migrate_legacy(dir.path(), "old_label", &binding)
            .unwrap()
            .unwrap(),
    );
    fs::write(custody.join("old_label.archive.v1.json"), b"{}").unwrap();
    let state = State::at_with_installation(dir.path().into(), vec![], dir.path().into()).unwrap();
    let backup_dirs = backups(&custody);
    let [backup] = backup_dirs.as_slice() else {
        panic!("Expected one legacy backup");
    };
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("old_label-")
    );
    let backup_files = [
        "old_label",
        "old_label-history.json",
        "old_label-media.json",
    ]
    .map(|name| {
        let path = backup.join(name);
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    });
    let native_files = [
        ".v1.enc",
        ".store.key",
        ".migration.json",
        ".history.v1.json",
        ".media.v1.json",
        ".archive.v1.json",
    ]
    .map(|suffix| {
        (
            suffix,
            fs::read(custody.join(format!("old_label{suffix}"))).unwrap(),
        )
    });

    rename_account(&state, "old_label", "new_label").unwrap();
    for _ in 0..2 {
        let store = migrate_legacy(dir.path(), "new_label", &binding)
            .unwrap()
            .unwrap();
        assert_eq!(store.load().unwrap().unwrap().settings["fixture"], true);
    }

    assert_eq!(backups(&custody), backup_dirs);
    for (suffix, bytes) in native_files {
        assert!(!custody.join(format!("old_label{suffix}")).exists());
        assert_eq!(
            fs::read(custody.join(format!("new_label{suffix}"))).unwrap(),
            bytes
        );
    }

    // Simulate interruption after writing the receipt but before publishing the store.
    fs::remove_file(custody.join("new_label.v1.enc")).unwrap();
    let sidecar = custody.join("new_label-history.json");
    fs::write(&sidecar, br#"{"changed":true}"#).unwrap();
    assert!(migrate_legacy(dir.path(), "new_label", &binding).is_err());
    fs::write(sidecar, b"{}").unwrap();
    let store = migrate_legacy(dir.path(), "new_label", &binding)
        .unwrap()
        .unwrap();
    assert_eq!(store.load().unwrap().unwrap().settings["fixture"], true);
    drop(store);
    assert_eq!(backups(&custody), backup_dirs);
    assert_eq!(fs::read_dir(backup).unwrap().count(), backup_files.len());
    for (path, bytes) in backup_files {
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        #[cfg(windows)]
        {
            let name = path.file_name().unwrap().to_str().unwrap();
            let suffix = name.strip_prefix("old_label").unwrap();
            restore_fixture_permissions(&path, &custody.join(format!("new_label{suffix}")));
        }
    }
}

#[test]
fn multiple_chats_and_sidecars_survive_migration() {
    let burned = [0x5a; 256];
    let fingerprint = kiaro_secret_chat::crypto::SharedKey::from_slice(&burned)
        .unwrap()
        .fingerprint();
    let chat = |id| json!({"id":id,"access_hash":31,"peer_user_id":43,"is_outbound":true,"state":"ready","ttl":10,"layer":73,"wrapper_layer":46,"in_seq_no":4,"out_seq_no":5,"peer_in_seq_no":3,"key":{"__bytes__":"5a".repeat(burned.len())},"key_fingerprint":fingerprint});
    let (dir, _) = fixture(
        &json!({"chats":{"-698877786":chat(-698_877_786),"13":chat(13),"15":chat(15)},"in":{},"out":{},"settings":{"auto_save":{"on":false}}}),
    );
    let custody = dir.path().join("secret-chats");
    let history = br#"{"13":[{"message_id":321,"text":"offline sample"}]}"#;
    let media = br#"{"13":{}}"#;
    for (suffix, bytes) in [
        ("-history.json", history.as_slice()),
        ("-media.json", media.as_slice()),
        (".owner.json", br#"{"user_id":17}"#.as_slice()),
    ] {
        fs::write(custody.join(format!("{LABEL}{suffix}")), bytes).unwrap();
    }
    let binding = binding();
    drop(
        migrate_legacy(dir.path(), LABEL, &binding)
            .unwrap()
            .unwrap(),
    );
    let key = fs::read(custody.join("offline.store.key")).unwrap();
    let max_bytes = 64 * 1024 * 1024;
    let reopened = EncryptedIngressStorage::open(
        custody.join("offline.v1.enc"),
        key.try_into().unwrap(),
        binding.clone(),
        max_bytes,
    )
    .unwrap();
    let limits = CustodyLimits::new(4096, max_bytes, 30.0 * 86400.0).unwrap();
    let store = CustodyStorage::new(reopened, binding.digest(), limits)
        .expect("migrated secret chats must reopen with bounded custody");
    let state = store.load().unwrap().unwrap();
    assert_eq!(
        state.chats.keys().copied().collect::<Vec<_>>(),
        [-698_877_786, 13, 15]
    );
    let first = &state.chats[&13];
    assert_eq!(
        (first.ttl, first.capability_layer, first.wrapper_layer),
        (10, 73, 46)
    );
    assert_eq!(
        (first.in_seq_no, first.out_seq_no, first.peer_in_seq_no),
        (4, 5, 3)
    );
    assert_eq!(first.key.as_ref().unwrap().fingerprint(), fingerprint);
    assert_eq!(state.settings["auto_save"], json!({"on":false}));
    assert_eq!(
        fs::read(custody.join("offline.history.v1.json")).unwrap(),
        history
    );
    assert_eq!(
        fs::read(custody.join("offline.media.v1.json")).unwrap(),
        media
    );
    #[cfg(windows)]
    for file in fs::read_dir(&backups(&custody)[0]).unwrap() {
        let file = file.unwrap();
        restore_fixture_permissions(&file.path(), &custody.join(file.file_name()));
    }
}

#[tokio::test]
async fn migrated_store_accepts_a_baseline_on_the_original_unstarted_pool() {
    use grammers_mtsender::SenderPool;
    use grammers_session::{SessionData, storages::MemorySession, updates::MessageBoxes};
    use kiaro_secret_chat::{custody::IngressLimits, ingress::DurableIngress};
    use std::sync::Arc;
    let (dir, _) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let store = migrate_legacy(dir.path(), LABEL, &binding())
        .unwrap()
        .unwrap();
    let mut session = SessionData {
        home_dc: 2,
        ..Default::default()
    };
    session.dc_options.get_mut(&2).unwrap().auth_key = Some(AUTH);
    session.updates_state.pts = 17;
    session.updates_state.qts = 4;
    session.updates_state.date = 1_700_000_000;
    session.updates_state.seq = 12;
    let baseline = MessageBoxes::load(session.updates_state.clone())
        .durable_checkpoint()
        .unwrap();
    let pool = SenderPool::new(Arc::new(MemorySession::from(session)), 1);
    let ingress =
        DurableIngress::new(store, IngressLimits::new(32, 1024 * 1024, 3600.0).unwrap()).unwrap();
    assert!(ingress.prepare(&pool.handle, None).await.is_err());
    ingress.prepare(&pool.handle, Some(baseline)).await.unwrap();
    drop(pool);
    drop(ingress);
    #[cfg(windows)]
    {
        let custody = dir.path().join("secret-chats");
        restore_fixture_permissions(&backups(&custody)[0].join(LABEL), &custody.join(LABEL));
    }
}

#[test]
fn corrupt_legacy_store_is_left_untouched() {
    let (dir, _) = fixture(&json!({}));
    let custody = dir.path().join("secret-chats");
    let source = custody.join(LABEL);
    fs::write(&source, b"{ invalid json").unwrap();
    assert!(migrate_legacy(dir.path(), LABEL, &binding()).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"{ invalid json");

    let mismatched = br#"{"chats":{"-698877786":{"id":7,"access_hash":31,"peer_user_id":43,"is_outbound":true,"state":"closed","in_seq_no":0,"out_seq_no":0}},"in":{},"out":{},"settings":{}}"#;
    fs::write(&source, mismatched).unwrap();
    let error = migrate_legacy(dir.path(), LABEL, &binding()).err().unwrap();
    assert_eq!(
        error.to_string(),
        "Legacy secret-chat identifier does not match its record"
    );
    assert_eq!(fs::read(&source).unwrap(), mismatched);
    assert!(!custody.join("offline.v1.enc").exists());
    assert!(!custody.join("offline.migration.json").exists());
}

#[test]
fn refuses_linked_legacy_sidecar_and_path_escape() {
    let (dir, _) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let custody = dir.path().join("secret-chats");
    let target = dir.path().join("foreign");
    let (link, source) = if cfg!(windows) {
        (custody.clone(), target.join("offline-history.json"))
    } else {
        (custody.join("offline-history.json"), target.clone())
    };
    #[cfg(windows)]
    fs::rename(&custody, &target).unwrap();
    fs::write(&source, b"{}").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .current_dir(dir.path())
            .args(["/D", "/C", "mklink", "/J", "secret-chats", "foreign"])
            .output()
            .unwrap();
        assert!(output.status.success(), "Cannot create directory junction");
    }
    let error = migrate_legacy(dir.path(), LABEL, &binding()).err().unwrap();
    let message = error.to_string();
    assert!(
        message.contains("symbolic link") || cfg!(windows) && message.contains("reparse point")
    );
    assert!(!custody.join("offline.v1.enc").exists());
    assert!(migrate_legacy(dir.path(), "../foreign", &binding()).is_err());
    assert_eq!(fs::read(source).unwrap(), b"{}");

    #[cfg(unix)]
    fs::remove_file(&link).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&link).unwrap();
    fs::rename(&target, &link).unwrap();
    assert!(
        migrate_legacy(dir.path(), LABEL, &binding())
            .unwrap()
            .is_some()
    );
    #[cfg(windows)]
    for name in [LABEL, "offline-history.json"] {
        restore_fixture_permissions(&backups(&custody)[0].join(name), &custody.join(name));
    }
}

#[tokio::test]
async fn deleted_chat_export_requires_choices_and_resolves_the_supplied_user() {
    use telegram_mcp::{backend::Telegram, tools};
    struct Archive;
    #[async_trait::async_trait]
    impl Telegram for Archive {
        async fn invoke(&self, _: &str, args: Value) -> anyhow::Result<Value> {
            Ok(args)
        }
        async fn resolve_peer(&self, value: &Value) -> anyhow::Result<Value> {
            assert!(value == "@archive_fixture" || value == &json!(37));
            Ok(json!({"_":"inputPeerUser","user_id":37,"access_hash":999}))
        }
    }
    let mut args = json!({"secret_chat_id":42,"peer_user":"@archive_fixture"});
    let response = tools::call(&Archive, "export_secret_chat", &args)
        .await
        .unwrap();
    assert_eq!(response["status"], "choices_required");
    assert!(
        response["required"]
            .as_array()
            .unwrap()
            .contains(&json!("destination"))
    );
    for key in [
        "photos",
        "videos",
        "voice_messages",
        "video_messages",
        "stickers",
        "gifs",
        "files",
    ] {
        args[key] = json!(false);
    }
    for key in ["date_from", "date_to", "destination"] {
        args[key] = json!("");
    }
    args["format"] = json!("json");
    args["size_limit_mb"] = json!(10);
    for peer in [json!(37), json!("@archive_fixture")] {
        args["peer_user"] = peer;
        let result = tools::call(&Archive, "export_secret_chat", &args)
            .await
            .unwrap();
        assert_eq!(result["peer_user"], 37);
        assert!(!result.to_string().contains("access_hash"));
    }
}

#[test]
fn auth_key_tag_matches_grammers_client_vector_and_v2_reopen_is_idempotent() {
    use kiaro_secret_chat::binding_migration::LegacyAuthorizationBinding;
    use sha1::{Digest, Sha1};

    let key: [u8; 256] = std::array::from_fn(|i| i as u8);
    assert_eq!(
        telegram_mcp::secret::auth_key_tag(&key),
        [
            0x75, 0x61, 0xe9, 0x28, 0x04, 0xb6, 0x93, 0x58, 0x48, 0xbd, 0x6b, 0x8b, 0xc3, 0x68,
            0x37, 0xa4, 0xd3, 0x0c, 0xe6, 0x31, 0x9f, 0x19, 0xb4, 0xc3, 0x21, 0x9c, 0x9c, 0x29,
            0xc6, 0x1d, 0x2b, 0x65,
        ]
    );

    let (dir, _) = fixture(&json!({"chats":{},"in":{},"out":{},"settings":{}}));
    let v2 = binding();
    let legacy_id = i64::from_le_bytes(Sha1::digest(AUTH)[12..].try_into().unwrap());
    let v1 =
        LegacyAuthorizationBinding::from_session_parts(17, legacy_id, 2, "migration-test").unwrap();
    let first =
        telegram_mcp::secret::open_store_with_legacy(dir.path(), LABEL, &v2, Some(&v1), true)
            .unwrap()
            .unwrap();
    drop(first);
    let second =
        telegram_mcp::secret::open_store_with_legacy(dir.path(), LABEL, &v2, Some(&v1), true)
            .unwrap()
            .unwrap();
    assert_eq!(second.load().unwrap().unwrap().authorization, v2.digest());
}

#[test]
fn interrupted_v1_receipt_resumes_to_v2_in_one_open() {
    use kiaro_secret_chat::binding_migration::LegacyAuthorizationBinding;
    use sha1::{Digest, Sha1};

    for create in [false, true] {
        let (dir, original) =
            fixture(&json!({"chats":{},"in":{},"out":{},"settings":{"retained":true}}));
        let custody = dir.path().join("secret-chats");
        let v2 = binding();
        drop(migrate_legacy(dir.path(), LABEL, &v2).unwrap().unwrap());
        let manifest = custody.join("offline.migration.json");
        let encrypted = custody.join("offline.v1.enc");
        let backup_dirs = backups(&custody);
        let legacy_id = i64::from_le_bytes(Sha1::digest(AUTH)[12..].try_into().unwrap());
        let v1 = LegacyAuthorizationBinding::from_session_parts(17, legacy_id, 2, "migration-test")
            .unwrap();

        // An older release may stop after its valid backup and v1 receipt, before publication.
        let mut receipt: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        receipt["authorization"] = json!(v1.fingerprint());
        fs::write(&manifest, serde_json::to_vec(&receipt).unwrap()).unwrap();
        fs::remove_file(&encrypted).unwrap();

        let store =
            telegram_mcp::secret::open_store_with_legacy(dir.path(), LABEL, &v2, Some(&v1), create)
                .expect("a valid interrupted v1 migration must complete in one open")
                .unwrap();
        let state = store.load().unwrap().unwrap();
        assert_eq!(state.authorization, v2.digest());
        assert_eq!(state.settings["retained"], true);
        drop(store);
        let updated: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(updated["authorization"], v2.digest());
        let ciphertext = fs::read(&encrypted).unwrap();
        let receipt_bytes = fs::read(&manifest).unwrap();
        drop(
            telegram_mcp::secret::open_store_with_legacy(dir.path(), LABEL, &v2, Some(&v1), create)
                .unwrap()
                .unwrap(),
        );
        assert_eq!(fs::read(&encrypted).unwrap(), ciphertext);
        assert_eq!(fs::read(&manifest).unwrap(), receipt_bytes);
        assert_eq!(backups(&custody), backup_dirs);
        assert_eq!(fs::read(custody.join(LABEL)).unwrap(), original);
        assert_eq!(fs::read(backup_dirs[0].join(LABEL)).unwrap(), original);
        assert!(
            fs::metadata(backup_dirs[0].join(LABEL))
                .unwrap()
                .permissions()
                .readonly()
        );
        #[cfg(windows)]
        restore_fixture_permissions(&backup_dirs[0].join(LABEL), &custody.join(LABEL));
    }
}
