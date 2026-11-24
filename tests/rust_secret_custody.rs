//! Deferred secret backend custody: synthetic local records, never Telegram I/O.
use base64::Engine;
use grammers_session::{Session, SessionData, types::UpdateState};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use telegram_mcp::{
    session::{PersistentSession, check_deferred_custody, check_deferred_custody_with_config},
    state::State,
};

#[path = "rust_secret_custody/authorization.rs"]
mod authorization;
#[cfg(windows)]
#[path = "common/windows_acl.rs"]
mod windows_acl;

struct Fixture {
    _temporary: tempfile::TempDir,
    state: Arc<State>,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let state = Arc::new(State::at(temporary.path().join("state"), Vec::new()).unwrap());
        Self {
            _temporary: temporary,
            state,
        }
    }
    fn snapshot(&self, origin: Option<&str>, qts: i32) {
        let mut data = SessionData::default();
        data.updates_state.qts = qts;
        let mut value = json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[],"updates_state":data.updates_state});
        if let Some(origin) = origin {
            value["authorization_origin"] = json!(origin);
        }
        self.state.write_json("work.session.json", &value).unwrap();
    }
    fn directory(&self) -> std::path::PathBuf {
        let path = self.state.root().join("secret-chats");
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn read(&self) -> Value {
        self.state
            .read_json("work.session.json", Value::Null)
            .unwrap()
    }
}

#[test]
fn configured_custody_migrates_only_for_the_bound_account() {
    let f = Fixture::new();
    let mut session = SessionData::default();
    session
        .dc_options
        .get_mut(&session.home_dc)
        .unwrap()
        .auth_key = Some([73; 256]);
    let native = json!({"home_dc":session.home_dc,"dc_options":session.dc_options.values().collect::<Vec<_>>(),"peer_infos":[],"updates_state":session.updates_state,"authorization_origin":"legacy"});
    f.state.write_json("work.session.json", &native).unwrap();
    let directory = f.directory();
    let legacy = br#"{"chats":{},"in":{},"out":{},"settings":{}}"#;
    let config = telegram_mcp::account_manager::ConfigSnapshot {
        values: Default::default(),
        digest: String::new(),
        process_keys: Default::default(),
        path: f.state.root().join("test.env"),
    };
    let source = directory.join("work");
    let owner = directory.join("work.owner.json");
    std::fs::write(&source, legacy).unwrap();
    std::fs::write(&owner, br#"{"user_id":91}"#).unwrap();
    // An imported session without a verified cached self peer must not bind or publish custody.
    assert!(check_deferred_custody_with_config(&f.state, "work", &config).is_err());
    assert!(!directory.join("work.v1.enc").exists());
    assert!(!directory.join("work.migration.json").exists());
    let wrong_self = grammers_session::types::PeerInfo::User {
        id: 92,
        auth: None,
        bot: Some(false),
        is_self: Some(true),
    };
    let wrong_native = json!({"home_dc":session.home_dc,"dc_options":session.dc_options.values().collect::<Vec<_>>(),"peer_infos":[wrong_self],"updates_state":session.updates_state,"authorization_origin":"legacy"});
    f.state
        .write_json("work.session.json", &wrong_native)
        .unwrap();
    assert!(check_deferred_custody_with_config(&f.state, "work", &config).is_err());
    assert!(!directory.join("work.v1.enc").exists());
    assert!(!directory.join("work.migration.json").exists());
    let verified_self = grammers_session::types::PeerInfo::User {
        id: 91,
        auth: None,
        bot: Some(false),
        is_self: Some(true),
    };
    let verified_native = json!({"home_dc":session.home_dc,"dc_options":session.dc_options.values().collect::<Vec<_>>(),"peer_infos":[verified_self],"updates_state":session.updates_state,"authorization_origin":"legacy"});
    f.state
        .write_json("work.session.json", &verified_native)
        .unwrap();
    std::fs::write(&source, b"broken-json").unwrap();
    assert!(check_deferred_custody_with_config(&f.state, "work", &config).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"broken-json");
    assert!(!directory.join("work.v1.enc").exists());
    std::fs::write(&source, legacy).unwrap();
    check_deferred_custody_with_config(&f.state, "work", &config).unwrap();
    assert!(directory.join("work.v1.enc").is_file());

    use grammers_client::Client;
    use grammers_mtsender::SenderPool;
    use grammers_session::storages::MemorySession;
    use kiaro_secret_chat::{
        manager::SecretChatManager, storage::MemoryStorage, transport::GrammersTransport,
    };
    use telegram_mcp::secret::HISTORY_HANDLER_LABEL;

    let pool = SenderPool::new(Arc::new(MemorySession::from(session)), 1);
    let manager = SecretChatManager::new(
        GrammersTransport::new(Client::new(pool.handle.clone())),
        MemoryStorage::default(),
        "ab".repeat(32),
        Default::default(),
    )
    .unwrap();
    manager
        .on_async("MessageReceived", HISTORY_HANDLER_LABEL, |_| async {
            Ok(())
        })
        .unwrap();
    manager
        .on("ServiceActionReceived", HISTORY_HANDLER_LABEL, |_| Ok(()))
        .unwrap();
    manager
        .on("ChatClosed", HISTORY_HANDLER_LABEL, |_| Ok(()))
        .unwrap();
    assert!(directory.join("work.migration.json").is_file());
    let backup = std::fs::read_dir(directory.join("legacy-backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(backup.join("work")).unwrap(), legacy);
    std::fs::write(&owner, br#"{"user_id":92}"#).unwrap();
    assert!(check_deferred_custody_with_config(&f.state, "work", &config).is_err());
    #[cfg(windows)]
    for entry in std::fs::read_dir(backup).unwrap() {
        let entry = entry.unwrap();
        let original = directory.join(entry.file_name());
        let permissions = std::fs::metadata(original).unwrap().permissions();
        std::fs::set_permissions(entry.path(), permissions).unwrap();
    }
}

fn string_session() -> String {
    let mut bytes = vec![2, 149, 154, 167, 50];
    bytes.extend(443u16.to_be_bytes());
    bytes.extend([0x72; 256]);
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}

#[test]
fn legacy_custody_directory_blocks_before_any_session_or_lock_is_created() {
    let f = Fixture::new();
    let directory = f.directory().join("work");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("opaque-private-state");
    let canary = [0, 255, 17, 9, 0, 34];
    std::fs::write(&path, canary).unwrap();
    let error = check_deferred_custody(&f.state, "work")
        .unwrap_err()
        .to_string();
    assert!(error.contains("separate Rust secret backend"));
    assert!(!error.contains("opaque-private-state"));
    assert_eq!(std::fs::read(path).unwrap(), canary);
    assert!(!f.state.root().join("work.session.lock").exists());
    assert!(!f.state.root().join("work.session.json").exists());
}

#[test]
fn legacy_custody_owner_record_blocks_without_parsing_or_replacing_its_bytes() {
    let f = Fixture::new();
    let path = f.directory().join("work.owner.json");
    let bytes = b"not JSON; preserve every byte\0";
    std::fs::write(&path, bytes).unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn old_snapshot_without_origin_is_treated_as_legacy_and_left_byte_identical() {
    let f = Fixture::new();
    f.snapshot(None, 17);
    std::fs::write(f.directory().join("work"), b"legacy storage file").unwrap();
    let path = f.state.root().join("work.session.json");
    let before = std::fs::read(&path).unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn explicit_legacy_origin_and_unknown_origins_cannot_bypass_existing_custody() {
    for origin in ["legacy", "unknown", ""] {
        let f = Fixture::new();
        f.snapshot(Some(origin), 0);
        std::fs::create_dir(f.directory().join("work")).unwrap();
        assert!(check_deferred_custody(&f.state, "work").is_err());
    }
}

#[test]
fn fresh_native_origin_is_allowed_without_touching_the_old_store() {
    let f = Fixture::new();
    f.snapshot(Some("native"), 0);
    let path = f.directory().join("work");
    std::fs::write(&path, b"old-key-canary").unwrap();
    check_deferred_custody(&f.state, "work").unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"old-key-canary");
}

#[test]
fn nonzero_qts_alone_does_not_claim_a_secret_chat_exists() {
    for origin in [None, Some("legacy"), Some("native")] {
        let f = Fixture::new();
        f.snapshot(origin, 999);
        check_deferred_custody(&f.state, "work").unwrap();
    }
}

#[test]
fn other_account_custody_does_not_block_an_unrelated_label() {
    let f = Fixture::new();
    std::fs::write(f.directory().join("other"), b"other-account-state").unwrap();
    check_deferred_custody(&f.state, "work").unwrap();
}

#[test]
fn empty_regular_custody_parent_is_allowed_without_creating_an_account_store() {
    let f = Fixture::new();
    let directory = f.directory();
    check_deferred_custody(&f.state, "work").unwrap();
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}

#[test]
fn malformed_native_record_refuses_instead_of_resetting_or_adopting_custody() {
    let f = Fixture::new();
    let path = f.state.root().join("work.session.json");
    let bytes = b"{invalid-native-json";
    std::fs::write(&path, bytes).unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn a_regular_file_at_the_custody_parent_fails_closed() {
    let f = Fixture::new();
    let path = f.state.root().join("secret-chats");
    std::fs::write(&path, b"opaque-owner-data").unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"opaque-owner-data");
}

fn directory_link(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Could not construct a directory junction fixture"
        );
    }
}

#[test]
fn redirected_parent_is_not_followed_even_when_its_target_has_no_account_data() {
    let f = Fixture::new();
    let target = f._temporary.path().join("redirected");
    std::fs::create_dir(&target).unwrap();
    directory_link(&f.state.root().join("secret-chats"), &target);
    assert!(check_deferred_custody(&f.state, "work").is_err());
    assert_eq!(std::fs::read_dir(target).unwrap().count(), 0);
}

#[test]
fn dangling_parent_link_cannot_turn_a_custody_error_into_missing_children() {
    let f = Fixture::new();
    let target = f._temporary.path().join("removed");
    std::fs::create_dir(&target).unwrap();
    directory_link(&f.state.root().join("secret-chats"), &target);
    std::fs::remove_dir(target).unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
}

#[test]
fn linked_account_store_is_treated_as_existing_custody() {
    let f = Fixture::new();
    let target = f._temporary.path().join("external-store");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("canary"), b"untouched").unwrap();
    directory_link(&f.directory().join("work"), &target);
    assert!(check_deferred_custody(&f.state, "work").is_err());
    assert_eq!(std::fs::read(target.join("canary")).unwrap(), b"untouched");
}

#[test]
fn unreadable_custody_metadata_refuses_without_changing_permissions() {
    let f = Fixture::new();
    let directory = f.directory();
    std::fs::write(directory.join("work"), b"unreadable-canary").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let original = std::fs::metadata(&directory).unwrap().permissions();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o0)).unwrap();
        let result = check_deferred_custody(&f.state, "work");
        let mode = std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(&directory, original).unwrap();
        assert!(result.is_err());
        assert_eq!(mode, 0);
    }
    #[cfg(windows)]
    {
        // The deny includes READ_CONTROL: restoring by reopening the path
        // would itself be denied. Keep the original handle until restoration.
        let mut denied = windows_acl::ReadDenied::new(&directory).unwrap();
        let before = denied.current_acl().unwrap();
        assert_eq!(
            std::fs::read_dir(&directory).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let result = check_deferred_custody(&f.state, "work");
        let after = denied.current_acl().unwrap();
        let access = std::fs::read_dir(&directory);
        denied.restore().unwrap();
        assert_eq!(before, after, "Custody inspection changed its DACL");
        assert_eq!(
            access.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(result.is_err());
        assert_eq!(denied.current_acl().unwrap(), denied.original_acl());
    }
    assert_eq!(
        std::fs::read(directory.join("work")).unwrap(),
        b"unreadable-canary"
    );
}

#[tokio::test]
async fn successful_string_import_persists_legacy_origin_across_reopen_and_checkpoint() {
    let f = Fixture::new();
    let session = PersistentSession::open(Arc::clone(&f.state), "work").unwrap();
    session.import_telethon(&string_session()).unwrap();
    assert_eq!(f.read()["authorization_origin"], "legacy");
    drop(session);
    let session = PersistentSession::open(Arc::clone(&f.state), "work").unwrap();
    session
        .set_update_state(UpdateState::Primary {
            pts: 9,
            date: 10,
            seq: 11,
        })
        .await
        .unwrap();
    assert_eq!(f.read()["authorization_origin"], "legacy");
    assert_eq!(
        session.dc_option(2).unwrap().unwrap().auth_key,
        Some([0x72; 256])
    );
    std::fs::write(f.directory().join("work"), b"now-detected").unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
}

#[tokio::test]
async fn failed_import_rolls_origin_back_before_the_next_successful_persistence() {
    let f = Fixture::new();
    let session = PersistentSession::open(Arc::clone(&f.state), "work").unwrap();
    session.set_home_dc_id(2).await.unwrap();
    let path = f.state.root().join("work.session.json");
    let saved = f.state.root().join("saved-snapshot");
    std::fs::rename(&path, &saved).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(session.import_telethon(&string_session()).is_err());
    assert!(session.dc_option(2).unwrap().unwrap().auth_key.is_none());
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(saved, &path).unwrap();
    session.set_home_dc_id(3).await.unwrap();
    assert_eq!(f.read()["authorization_origin"], "native");
}

#[tokio::test]
async fn old_snapshot_reopens_with_legacy_origin_and_primary_checkpoints_preserve_qts() {
    let f = Fixture::new();
    f.snapshot(None, 17);
    let session = PersistentSession::open(Arc::clone(&f.state), "work").unwrap();
    session
        .set_update_state(UpdateState::Primary {
            pts: 20,
            date: 21,
            seq: 22,
        })
        .await
        .unwrap();
    session
        .set_update_state(UpdateState::Channel { id: 7, pts: 30 })
        .await
        .unwrap();
    assert_eq!(session.updates_state().await.unwrap().qts, 17);
    assert_eq!(f.read()["authorization_origin"], "legacy");
}

#[tokio::test]
async fn fresh_native_session_persistence_keeps_native_origin_with_synthetic_authorization() {
    let f = Fixture::new();
    let session = PersistentSession::open(Arc::clone(&f.state), "work").unwrap();
    let mut option = session.dc_option(2).unwrap().unwrap();
    option.auth_key = Some([0x43; 256]);
    session.set_dc_option(&option).await.unwrap();
    assert_eq!(f.read()["authorization_origin"], "native");
    std::fs::write(f.directory().join("work"), b"legacy-store-unchanged").unwrap();
    check_deferred_custody(&f.state, "work").unwrap();
}

#[test]
fn sqlite_import_persists_legacy_origin_and_preserves_the_source_database() {
    let f = Fixture::new();
    // macOS exposes its temporary root through /var -> /private/var. Resolve
    // the owned fixture root before exercising the importer's symlink guard.
    let path = f
        ._temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("legacy.session");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE version(version integer primary key);INSERT INTO version VALUES(7);CREATE TABLE sessions(dc_id integer primary key,server_address text,port integer,auth_key blob,takeout_id integer);CREATE TABLE entities(id integer primary key,hash integer not null,username text,phone integer,name text,date integer);CREATE TABLE sent_files(md5_digest blob,file_size integer,type integer,id integer,hash integer,primary key(md5_digest,file_size,type));CREATE TABLE update_state(id integer primary key,pts integer,qts integer,date integer,seq integer);").unwrap();
    db.execute(
        "INSERT INTO sessions VALUES(2,'149.154.167.50',443,?,NULL)",
        [vec![0x64u8; 256]],
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    let session = PersistentSession::open(Arc::clone(&f.state), "work").unwrap();
    session.import_telethon_sqlite(&path).unwrap();
    assert_eq!(f.read()["authorization_origin"], "legacy");
    assert_eq!(std::fs::read(path).unwrap(), before);
    std::fs::write(
        f.directory().join("work.owner.json"),
        b"opaque-owner-binding",
    )
    .unwrap();
    assert!(check_deferred_custody(&f.state, "work").is_err());
}
