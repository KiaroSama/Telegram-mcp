//! Synthetic Telethon schema fixtures: no credentials or binary databases are committed.
use grammers_session::{
    Session,
    types::{PeerAuth, PeerId},
};
use rusqlite::{Connection, params};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use telegram_mcp::{session::PersistentSession, state::State};

fn string_session(port: u16, key: u8) -> String {
    use base64::Engine;
    let mut bytes = vec![2, 149, 154, 167, 50];
    bytes.extend(port.to_be_bytes());
    bytes.extend([key; 256]);
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}

fn assert_invalid_string_session_preserves_empty_authorization(port: u16, key: u8) {
    let f = Fixture::new(8);
    let session = f.session();
    assert!(session.import_telethon(&string_session(port, key)).is_err());
    assert!(!has_key(&session));
    assert!(!f.state.root().join("native.session.json").exists());
    session.import_telethon(&string_session(443, 0x73)).unwrap();
    assert_eq!(session.home_dc_id().unwrap(), 2);
    assert_eq!(
        session.dc_option(2).unwrap().unwrap().auth_key,
        Some([0x73; 256])
    );
}

#[test]
fn string_import_refuses_zero_port_without_publishing_authorization() {
    assert_invalid_string_session_preserves_empty_authorization(0, 0x72);
}

#[test]
fn string_import_refuses_zero_key_without_publishing_authorization() {
    assert_invalid_string_session_preserves_empty_authorization(443, 0);
}

struct Fixture {
    _temp: tempfile::TempDir,
    state: Arc<State>,
    path: PathBuf,
}
impl Fixture {
    fn new(version: i64) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("state");
        let files = base.join("files");
        std::fs::create_dir(&files).unwrap();
        let state = Arc::new(State::at(root, vec![files]).unwrap());
        let path = base.join("legacy.session");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE version (version integer primary key);CREATE TABLE entities (id integer primary key,hash integer not null,username text,phone integer,name text,date integer);CREATE TABLE sent_files (md5_digest blob,file_size integer,type integer,id integer,hash integer,primary key(md5_digest,file_size,type));CREATE TABLE update_state (id integer primary key,pts integer,qts integer,date integer,seq integer);").unwrap();
        db.execute_batch(if version==7{"CREATE TABLE sessions (dc_id integer primary key,server_address text,port integer,auth_key blob,takeout_id integer);"}else{"CREATE TABLE sessions (dc_id integer primary key,server_address text,port integer,auth_key blob,takeout_id integer,tmp_auth_key blob);"}).unwrap();
        db.execute("INSERT INTO version VALUES (?)", [version])
            .unwrap();
        db.execute("INSERT INTO sessions (dc_id,server_address,port,auth_key,takeout_id) VALUES (4,'149.154.167.50',443,?,NULL)",[vec![42u8;256]]).unwrap();
        db.execute("INSERT INTO entities VALUES (42,-199,'private_username',123456,'Private Name',1770000000)",[]).unwrap();
        db.execute(
            "INSERT INTO entities VALUES (-88,0,NULL,NULL,'Private Group',1770000000)",
            [],
        )
        .unwrap();
        db.execute("INSERT INTO entities VALUES (-1000000000777,299,NULL,NULL,'Private Channel',1770000000)",[]).unwrap();
        db.execute("INSERT INTO update_state VALUES (0,10,4,1800000000,3)", [])
            .unwrap();
        db.execute(
            "INSERT INTO update_state VALUES (777,99,0,1800000000,0)",
            [],
        )
        .unwrap();
        drop(db);
        Self {
            _temp: temp,
            state,
            path,
        }
    }
    fn session(&self) -> PersistentSession {
        PersistentSession::open(Arc::clone(&self.state), "native").unwrap()
    }
    fn change(&self, sql: &str) {
        Connection::open(&self.path)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
    fn unchanged(&self, before: &[u8]) {
        assert_eq!(std::fs::read(&self.path).unwrap(), before);
    }
}
fn has_key(session: &PersistentSession) -> bool {
    (1..=5).any(|dc| session.dc_option(dc).unwrap().unwrap().auth_key.is_some())
}

#[test]
fn version8_authorization_import_preserves_dc_endpoint_and_exact_key() {
    let f = Fixture::new(8);
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    assert_eq!(session.home_dc_id().unwrap(), 4);
    let dc = session.dc_option(4).unwrap().unwrap();
    assert_eq!(dc.auth_key, Some([42; 256]));
    assert_eq!(dc.ipv4.ip().to_string(), "149.154.167.50");
    assert_eq!(dc.ipv4.port(), 443);
}
#[test]
fn version7_is_read_without_upgrading_the_source_database() {
    let f = Fixture::new(7);
    let before = std::fs::read(&f.path).unwrap();
    f.session().import_telethon_sqlite(&f.path).unwrap();
    f.unchanged(&before);
    let db = Connection::open(&f.path).unwrap();
    assert_eq!(
        db.query_row("SELECT version FROM version", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
}
#[tokio::test]
async fn peer_capabilities_keep_user_chat_and_channel_identity() {
    let f = Fixture::new(8);
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    assert_eq!(
        session
            .peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .unwrap()
            .auth(),
        Some(PeerAuth::from_hash(-199))
    );
    assert!(
        session
            .peer(PeerId::chat(88).unwrap())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        session
            .peer(PeerId::channel(777).unwrap())
            .await
            .unwrap()
            .unwrap()
            .auth(),
        Some(PeerAuth::from_hash(299))
    );
}
#[tokio::test]
async fn primary_and_channel_update_cursors_survive_the_import() {
    let f = Fixture::new(8);
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    let state = session.updates_state().await.unwrap();
    assert_eq!(
        (state.pts, state.qts, state.date, state.seq),
        (10, 4, 1800000000, 3)
    );
    assert_eq!(state.channels.len(), 1);
    assert_eq!((state.channels[0].id, state.channels[0].pts), (777, 99));
}
#[test]
fn imported_authorization_survives_reopening_native_storage() {
    let f = Fixture::new(8);
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    drop(session);
    let reopened = f.session();
    assert_eq!(reopened.home_dc_id().unwrap(), 4);
    assert_eq!(
        reopened.dc_option(4).unwrap().unwrap().auth_key,
        Some([42; 256])
    );
}
#[test]
fn importer_never_replaces_existing_native_authorization() {
    let f = Fixture::new(8);
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    let before = std::fs::read(f.state.root().join("native.session.json")).unwrap();
    f.change("UPDATE sessions SET auth_key=zeroblob(256)");
    assert!(
        session
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("overwrite")
    );
    assert_eq!(
        std::fs::read(f.state.root().join("native.session.json")).unwrap(),
        before
    );
}
#[test]
fn future_sqlite_schema_is_refused_without_starting_a_new_login() {
    let f = Fixture::new(9);
    let session = f.session();
    assert!(
        session
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("versions 7 and 8")
    );
    assert!(!has_key(&session));
    assert!(!f.state.root().join("native.session.json").exists());
}
#[test]
fn older_schema_with_potentially_stale_hashes_is_refused() {
    let f = Fixture::new(6);
    assert!(f.session().import_telethon_sqlite(&f.path).is_err());
}
#[test]
fn unexpected_table_is_refused_without_reading_its_records() {
    let f = Fixture::new(8);
    f.change(
        "CREATE TABLE secret_chats (private_key BLOB);INSERT INTO secret_chats VALUES (X'010203')",
    );
    let before = std::fs::read(&f.path).unwrap();
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("unexpected")
    );
    f.unchanged(&before);
}
#[test]
fn unexpected_schema_column_is_refused() {
    let f = Fixture::new(8);
    f.change("ALTER TABLE sessions ADD COLUMN unexpected TEXT");
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("layout")
    );
}
#[test]
fn source_database_is_not_modified_and_no_sidecars_are_created() {
    let f = Fixture::new(8);
    let before = std::fs::read(&f.path).unwrap();
    let permissions = std::fs::metadata(&f.path).unwrap().permissions();
    let modified = std::fs::metadata(&f.path).unwrap().modified().unwrap();
    f.session().import_telethon_sqlite(&f.path).unwrap();
    f.unchanged(&before);
    assert_eq!(
        std::fs::metadata(&f.path).unwrap().permissions(),
        permissions
    );
    assert_eq!(
        std::fs::metadata(&f.path).unwrap().modified().unwrap(),
        modified
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!Path::new(&format!("{}{suffix}", f.path.display())).exists());
    }
}
#[test]
fn null_authorization_key_is_refused() {
    let f = Fixture::new(8);
    f.change("UPDATE sessions SET auth_key=NULL");
    let session = f.session();
    assert!(session.import_telethon_sqlite(&f.path).is_err());
    assert!(!has_key(&session));
}
#[test]
fn short_authorization_key_is_refused() {
    let f = Fixture::new(8);
    Connection::open(&f.path)
        .unwrap()
        .execute("UPDATE sessions SET auth_key=?", [vec![42u8; 255]])
        .unwrap();
    assert!(f.session().import_telethon_sqlite(&f.path).is_err());
}
#[test]
fn zero_authorization_key_is_refused() {
    let f = Fixture::new(8);
    f.change("UPDATE sessions SET auth_key=zeroblob(256)");
    assert!(f.session().import_telethon_sqlite(&f.path).is_err());
}
#[test]
fn multiple_authorization_rows_are_refused() {
    let f = Fixture::new(8);
    Connection::open(&f.path).unwrap().execute("INSERT INTO sessions (dc_id,server_address,port,auth_key) VALUES (5,'149.154.171.5',443,?)",[vec![43u8;256]]).unwrap();
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("multiple")
    );
}
#[test]
fn malformed_ip_is_refused_without_dns_lookup() {
    let f = Fixture::new(8);
    f.change("UPDATE sessions SET server_address='attacker.example'");
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("literal")
    );
}
#[test]
fn invalid_port_is_refused() {
    let f = Fixture::new(8);
    f.change("UPDATE sessions SET port=70000");
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("port")
    );
}
#[test]
fn unsupported_datacenter_is_refused() {
    let f = Fixture::new(8);
    f.change("UPDATE sessions SET dc_id=99");
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("datacenter")
    );
}
#[test]
fn malformed_peer_id_aborts_the_entire_import() {
    let f = Fixture::new(8);
    f.change("INSERT INTO entities VALUES (0,13,NULL,NULL,NULL,0)");
    let session = f.session();
    assert!(session.import_telethon_sqlite(&f.path).is_err());
    assert!(!has_key(&session));
}
#[test]
fn active_wal_requires_legacy_owner_to_close_and_checkpoint_it() {
    let f = Fixture::new(8);
    std::fs::write(format!("{}-wal", f.path.display()), [1u8; 64]).unwrap();
    let session = f.session();
    assert!(
        session
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("checkpoint")
    );
    assert!(!has_key(&session));
}
#[test]
fn malformed_database_bytes_are_refused() {
    let f = Fixture::new(8);
    std::fs::write(&f.path, vec![7u8; 1024]).unwrap();
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("SQLite 3")
    );
}
#[test]
fn oversized_database_is_refused_before_querying() {
    let f = Fixture::new(8);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&f.path)
        .unwrap()
        .set_len(128 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        f.session()
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("128 MiB")
    );
}
#[test]
fn failed_native_persistence_leaves_in_memory_authorization_unchanged() {
    let f = Fixture::new(8);
    let session = f.session();
    std::fs::create_dir(f.state.root().join("native.session.json")).unwrap();
    let before = std::fs::read(&f.path).unwrap();
    assert!(session.import_telethon_sqlite(&f.path).is_err());
    assert!(!has_key(&session));
    f.unchanged(&before);
}
#[test]
fn temporary_auth_key_is_left_in_the_source_and_not_substituted() {
    let f = Fixture::new(8);
    Connection::open(&f.path)
        .unwrap()
        .execute(
            "UPDATE sessions SET tmp_auth_key=?",
            params![vec![99u8; 256]],
        )
        .unwrap();
    let before = std::fs::read(&f.path).unwrap();
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    assert_eq!(
        session.dc_option(4).unwrap().unwrap().auth_key,
        Some([42; 256])
    );
    f.unchanged(&before);
}
#[test]
fn pii_is_not_copied_into_native_peer_capability_snapshot() {
    let f = Fixture::new(8);
    f.session().import_telethon_sqlite(&f.path).unwrap();
    let native = std::fs::read_to_string(f.state.root().join("native.session.json")).unwrap();
    assert!(!native.contains("private_username"));
    assert!(!native.contains("Private Name"));
}
#[cfg(unix)]
#[test]
fn symlink_source_is_refused() {
    let f = Fixture::new(8);
    let link = f.path.with_file_name("link.session");
    std::os::unix::fs::symlink(&f.path, &link).unwrap();
    assert!(
        f.session()
            .import_telethon_sqlite(&link)
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
}
#[cfg(unix)]
#[test]
fn symlink_parent_is_refused() {
    let f = Fixture::new(8);
    let root = tempfile::tempdir().unwrap();
    let link = root.path().join("linked");
    std::os::unix::fs::symlink(f.path.parent().unwrap(), &link).unwrap();
    assert!(
        f.session()
            .import_telethon_sqlite(&link.join("legacy.session"))
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
}
#[test]
fn ipv6_session_endpoint_is_restored() {
    let f = Fixture::new(8);
    f.change("UPDATE sessions SET server_address='2001:b28:f23f:f005::a',port=80");
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    let dc = session.dc_option(4).unwrap().unwrap();
    assert_eq!(dc.ipv6.ip().to_string(), "2001:b28:f23f:f005::a");
    assert_eq!(dc.ipv6.port(), 80);
}
#[test]
fn checkpointed_closed_wal_database_is_read_without_recreating_sidecars() {
    let f = Fixture::new(8);
    let db = Connection::open(&f.path).unwrap();
    let mode: String = db
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    db.execute("UPDATE sessions SET port=80", []).unwrap();
    drop(db);
    let before = std::fs::read(&f.path).unwrap();
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    f.unchanged(&before);
    assert_eq!(session.dc_option(4).unwrap().unwrap().ipv4.port(), 80);
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!Path::new(&format!("{}{suffix}", f.path.display())).exists());
    }
}
#[test]
fn unicode_legacy_path_round_trips_through_read_only_sqlite_uri() {
    let mut f = Fixture::new(8);
    let path = f.path.with_file_name("legacy-東京-حساب.session");
    std::fs::rename(&f.path, &path).unwrap();
    f.path = path;
    let before = std::fs::read(&f.path).unwrap();
    let session = f.session();
    session.import_telethon_sqlite(&f.path).unwrap();
    f.unchanged(&before);
    assert!(has_key(&session));
}
#[test]
fn read_only_source_permissions_are_sufficient_and_preserved() {
    let f = Fixture::new(8);
    let original = std::fs::metadata(&f.path).unwrap().permissions();
    let mut read_only = original.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&f.path, read_only).unwrap();
    let session = f.session();
    let result = session.import_telethon_sqlite(&f.path);
    let retained = std::fs::metadata(&f.path).unwrap().permissions().readonly();
    std::fs::set_permissions(&f.path, original).unwrap();
    result.unwrap();
    assert!(retained);
}
#[test]
fn peer_row_budget_rejects_the_whole_import_instead_of_dropping_capabilities() {
    let f = Fixture::new(8);
    f.change("DELETE FROM entities;WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id<100001) INSERT INTO entities SELECT id,13,NULL,NULL,NULL,0 FROM ids;");
    let session = f.session();
    assert!(
        session
            .import_telethon_sqlite(&f.path)
            .unwrap_err()
            .to_string()
            .contains("100,000")
    );
    assert!(!has_key(&session));
    assert!(!f.state.root().join("native.session.json").exists());
}
