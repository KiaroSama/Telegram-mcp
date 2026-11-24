use kiaro_secret_chat::{
    binding::AuthorizationBinding,
    custody::{CustodyLimits, CustodyStorage},
    ingress::EncryptedIngressStorage,
    storage::Storage,
};
use serde_json::{Value, json};
use std::{fs, path::Path};
use telegram_mcp::secret::migrate_legacy;

const LABEL: &str = "offline";

const AUTH: [u8; 256] = [0x3c; 256];

fn binding() -> AuthorizationBinding {
    use sha1::{Digest, Sha1};
    let fingerprint = i64::from_le_bytes(Sha1::digest(AUTH)[12..].try_into().unwrap());
    AuthorizationBinding::from_session_parts(17, fingerprint, 2, "migration-test".into()).unwrap()
}

fn fixture(legacy: &Value) -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
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
    for (suffix, bytes) in native_files {
        assert!(!custody.join(format!("old_label{suffix}")).exists());
        assert_eq!(
            fs::read(custody.join(format!("new_label{suffix}"))).unwrap(),
            bytes
        );
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
    let target = dir.path().join("foreign");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("canary"), b"{}").unwrap();
    let link = dir.path().join("secret-chats/offline-history.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let command = format!(r#"mklink /J "{}" "{}""#, link.display(), target.display());
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C"])
            .raw_arg(command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Cannot create junction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(migrate_legacy(dir.path(), LABEL, &binding()).is_err());
    assert!(migrate_legacy(dir.path(), "../foreign", &binding()).is_err());
    assert_eq!(fs::read(target.join("canary")).unwrap(), b"{}");
    assert!(!dir.path().join("secret-chats/offline.v1.enc").exists());
}
