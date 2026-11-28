use super::*;
use std::collections::{BTreeMap, BTreeSet};
use telegram_mcp::{account_manager::ConfigSnapshot, session::check_deferred_custody_with_config};

fn config(f: &Fixture, values: &[(&str, String)]) -> ConfigSnapshot {
    ConfigSnapshot {
        values: values
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        digest: String::new(),
        path: f._temporary.path().canonicalize().unwrap().join(".env"),
        process_keys: BTreeSet::new(),
    }
}
fn string_with(key: u8, endpoint: [u8; 4], port: u16) -> String {
    let mut bytes = vec![2];
    bytes.extend(endpoint);
    bytes.extend(port.to_be_bytes());
    bytes.extend([key; 256]);
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}
fn string(key: u8) -> String {
    string_with(key, [149, 154, 167, 50], 443)
}
fn snapshot(f: &Fixture, label: &str, key: Option<u8>, origin: &str) {
    let mut data = SessionData::default();
    data.dc_options.get_mut(&2).unwrap().auth_key = key.map(|key| [key; 256]);
    let value = json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[],"updates_state":data.updates_state,"authorization_origin":origin});
    f.state
        .write_json(&format!("{label}.session.json"), &value)
        .unwrap();
}
fn custody(f: &Fixture) -> std::path::PathBuf {
    let path = f.directory().join("old");
    std::fs::write(&path, b"opaque-secret-custody-canary").unwrap();
    path
}

fn native_custody(f: &Fixture, key: u8) -> std::path::PathBuf {
    use kiaro_secret_chat::{
        binding::AuthorizationBinding,
        chat::{Chat, ChatState},
        crypto::SharedKey,
        ingress::EncryptedIngressStorage,
        storage::Storage,
    };

    snapshot(f, "old", Some(key), "native");
    let binding = AuthorizationBinding::v2(
        91,
        2,
        telegram_mcp::secret::auth_key_tag(&[key; 256]),
        "telegram-mcp-secret-v1",
    )
    .unwrap();
    let path = f.directory().join("old.v1.enc");
    let mut store = EncryptedIngressStorage::open(&path, [0x51; 32], binding, 65536).unwrap();
    let mut state = store.load().unwrap().unwrap();
    let mut chat = Chat::new(42, 123, 92, true, ChatState::Ready);
    chat.key = Some(SharedKey::from_slice(&[0x52; 256]).unwrap());
    state.chats.insert(42, chat);
    store.commit(&state).unwrap();
    path
}

#[test]
fn native_encrypted_custody_blocks_the_same_authorization_under_another_label() {
    let f = Fixture::new();
    let path = native_custody(&f, 0x72);
    let before = std::fs::read(&path).unwrap();
    let config = config(&f, &[("TELEGRAM_SESSION_STRING_ALIAS", string(0x72))]);

    assert!(check_deferred_custody_with_config(&f.state, "alias", &config).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!f.state.root().join("alias.session.json").exists());
    assert!(!f.state.root().join("alias.session.lock").exists());
    assert!(!f.directory().join("alias.v1.enc").exists());

    for origin in ["legacy", "native"] {
        snapshot(&f, "alias", Some(0x72), origin);
        assert!(check_deferred_custody_with_config(&f.state, "alias", &config).is_err());
    }
}

#[test]
fn native_encrypted_custody_allows_its_owner_and_distinct_authorizations() {
    let f = Fixture::new();
    let path = native_custody(&f, 0x72);
    let before = std::fs::read(&path).unwrap();
    let config = config(&f, &[("TELEGRAM_SESSION_STRING_ALIAS", string(0x73))]);

    check_deferred_custody_with_config(&f.state, "old", &config).unwrap();
    check_deferred_custody_with_config(&f.state, "alias", &config).unwrap();
    for origin in ["legacy", "native"] {
        snapshot(&f, "alias", Some(0x73), origin);
        check_deferred_custody_with_config(&f.state, "alias", &config).unwrap();
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

fn sqlite(f: &Fixture, name: &str, key: u8) -> std::path::PathBuf {
    let path = f._temporary.path().canonicalize().unwrap().join(name);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE version(version integer primary key);INSERT INTO version VALUES(7);CREATE TABLE sessions(dc_id integer primary key,server_address text,port integer,auth_key blob,takeout_id integer);CREATE TABLE entities(id integer primary key,hash integer not null,username text,phone integer,name text,date integer);CREATE TABLE sent_files(md5_digest blob,file_size integer,type integer,id integer,hash integer,primary key(md5_digest,file_size,type));CREATE TABLE update_state(id integer primary key,pts integer,qts integer,date integer,seq integer);").unwrap();
    db.execute(
        "INSERT INTO sessions VALUES(2,'149.154.167.50',443,?,NULL)",
        [vec![key; 256]],
    )
    .unwrap();
    drop(db);
    path
}

#[test]
fn two_labels_with_the_same_string_authorization_cannot_bypass_secret_custody() {
    let f = Fixture::new();
    let path = custody(&f);
    let config = config(
        &f,
        &[
            ("TELEGRAM_SESSION_STRING_OLD", string(0x72)),
            ("TELEGRAM_SESSION_STRING_ALIAS", string(0x72)),
        ],
    );
    let error = check_deferred_custody_with_config(&f.state, "alias", &config)
        .unwrap_err()
        .to_string();
    assert!(error.contains("existing secret-chat custody"));
    assert!(!error.contains(&string(0x72)));
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"opaque-secret-custody-canary"
    );
    assert!(!f.state.root().join("alias.session.json").exists());
    assert!(!f.state.root().join("alias.session.lock").exists());
    assert!(!std::fs::read_dir(f.state.root()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("auth-")
    }));
}

#[test]
fn different_string_endpoints_do_not_hide_an_identical_authorization_key() {
    let f = Fixture::new();
    custody(&f);
    let config = config(
        &f,
        &[
            ("TELEGRAM_SESSION_STRING_OLD", string(0x72)),
            (
                "TELEGRAM_SESSION_STRING_ALIAS",
                string_with(0x72, [149, 154, 167, 40], 80),
            ),
        ],
    );
    assert!(check_deferred_custody_with_config(&f.state, "alias", &config).is_err());
}

#[test]
fn an_identified_different_legacy_authorization_is_allowed_without_modifying_custody() {
    let f = Fixture::new();
    let path = custody(&f);
    let before = std::fs::read(path.clone()).unwrap();
    let config = config(
        &f,
        &[
            ("TELEGRAM_SESSION_STRING_OLD", string(0x72)),
            ("TELEGRAM_SESSION_STRING_ALIAS", string(0x73)),
        ],
    );
    check_deferred_custody_with_config(&f.state, "alias", &config).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn copying_an_imported_native_snapshot_to_an_alias_does_not_remove_custody() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "old", Some(0x72), "legacy");
    snapshot(&f, "alias", Some(0x72), "legacy");
    let before = std::fs::read(f.state.root().join("alias.session.json")).unwrap();
    assert!(check_deferred_custody_with_config(&f.state, "alias", &config(&f, &[])).is_err());
    assert_eq!(
        std::fs::read(f.state.root().join("alias.session.json")).unwrap(),
        before
    );
}

#[test]
fn the_actual_imported_authorization_takes_precedence_over_the_old_configured_source() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "old", Some(0x72), "legacy");
    let config = config(
        &f,
        &[
            ("TELEGRAM_SESSION_STRING_OLD", string(0x73)),
            ("TELEGRAM_SESSION_STRING_ALIAS", string(0x72)),
        ],
    );
    assert!(check_deferred_custody_with_config(&f.state, "alias", &config).is_err());
}

#[test]
fn string_and_sqlite_sources_compare_the_same_authorization_in_both_directions() {
    for old_is_sqlite in [true, false] {
        let f = Fixture::new();
        custody(&f);
        let path = sqlite(&f, "legacy.session", 0x72);
        let before = std::fs::read(&path).unwrap();
        let values = if old_is_sqlite {
            vec![
                (
                    "TELEGRAM_SESSION_NAME_OLD",
                    path.to_str().unwrap().to_owned(),
                ),
                ("TELEGRAM_SESSION_STRING_ALIAS", string(0x72)),
            ]
        } else {
            vec![
                ("TELEGRAM_SESSION_STRING_OLD", string(0x72)),
                (
                    "TELEGRAM_SESSION_NAME_ALIAS",
                    path.to_str().unwrap().to_owned(),
                ),
            ]
        };
        assert!(
            check_deferred_custody_with_config(&f.state, "alias", &config(&f, &values)).is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!path.with_file_name("legacy.session-wal").exists());
        assert!(!path.with_file_name("legacy.session-shm").exists());
    }
}

#[test]
fn sqlite_hardlink_aliases_cannot_hide_shared_custody_or_modify_the_source() {
    let f = Fixture::new();
    custody(&f);
    let old = sqlite(&f, "old.session", 0x72);
    let alias = old.with_file_name("alias.session");
    std::fs::hard_link(&old, &alias).unwrap();
    let before = std::fs::read(&old).unwrap();
    let config = config(
        &f,
        &[
            ("TELEGRAM_SESSION_NAME_OLD", old.to_str().unwrap().into()),
            (
                "TELEGRAM_SESSION_NAME_ALIAS",
                alias.to_str().unwrap().into(),
            ),
        ],
    );
    assert!(check_deferred_custody_with_config(&f.state, "alias", &config).is_err());
    assert_eq!(std::fs::read(old).unwrap(), before);
    assert_eq!(std::fs::read(alias).unwrap(), before);
}

#[test]
fn owner_sidecar_alone_blocks_an_alias_without_parsing_its_private_payload() {
    let f = Fixture::new();
    let path = f.directory().join("old.owner.json");
    std::fs::write(&path, b"opaque-unparseable-owner-canary").unwrap();
    let config = config(
        &f,
        &[
            ("TELEGRAM_SESSION_STRING_OLD", string(0x72)),
            ("TELEGRAM_SESSION_STRING_ALIAS", string(0x72)),
        ],
    );
    assert!(check_deferred_custody_with_config(&f.state, "alias", &config).is_err());
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"opaque-unparseable-owner-canary"
    );
}

#[test]
fn unidentified_custody_and_malformed_legacy_authorizations_fail_closed_before_import() {
    for corrupt in [false, true] {
        let f = Fixture::new();
        custody(&f);
        let values = if corrupt {
            vec![
                (
                    "TELEGRAM_SESSION_STRING_OLD",
                    "1private-invalid-authorization".into(),
                ),
                ("TELEGRAM_SESSION_STRING_ALIAS", string(0x73)),
            ]
        } else {
            vec![("TELEGRAM_SESSION_STRING_ALIAS", string(0x73))]
        };
        let error = check_deferred_custody_with_config(&f.state, "alias", &config(&f, &values))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Cannot identify"));
        assert!(!error.contains("private-invalid-authorization"));
        assert!(!f.state.root().join("alias.session.json").exists());
    }
}

#[test]
fn fresh_native_origin_keeps_its_exemption_even_when_old_custody_identity_is_unavailable() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "alias", Some(0x73), "native");
    check_deferred_custody_with_config(&f.state, "alias", &config(&f, &[])).unwrap();
}

#[test]
fn an_existing_snapshot_without_an_authorization_cannot_consume_the_encrypted_queue() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "alias", None, "legacy");
    check_deferred_custody_with_config(&f.state, "alias", &config(&f, &[])).unwrap();
}

#[test]
fn an_oversized_native_authorization_set_is_refused_before_comparing_custody() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "old", Some(0x72), "legacy");
    snapshot(&f, "alias", Some(0x73), "legacy");
    let mut alias = f
        .state
        .read_json("alias.session.json", Value::Null)
        .unwrap();
    let options: Vec<grammers_session::types::DcOption> =
        serde_json::from_value(alias["dc_options"].clone()).unwrap();
    let prototype = options[0].clone();
    alias["dc_options"] = (1..=33)
        .map(|id| {
            let mut option = prototype.clone();
            option.id = id;
            option.auth_key = Some([0x73u8; 256]);
            json!(option)
        })
        .collect();
    let _: Vec<grammers_session::types::DcOption> =
        serde_json::from_value(alias["dc_options"].clone()).unwrap();
    f.state.write_json("alias.session.json", &alias).unwrap();
    assert!(check_deferred_custody_with_config(&f.state, "alias", &config(&f, &[])).is_err());
    assert_eq!(
        f.state
            .read_json("alias.session.json", Value::Null)
            .unwrap(),
        alias
    );
}

#[test]
fn a_single_byte_difference_at_every_key_position_identifies_a_different_authorization() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "old", Some(0x72), "legacy");
    let original = string(0x72);
    let mut bytes = base64::engine::general_purpose::URL_SAFE
        .decode(&original[1..])
        .unwrap();
    let offset = bytes.len() - 256;
    for position in 0..256 {
        bytes[offset + position] ^= 1;
        let different = format!(
            "1{}",
            base64::engine::general_purpose::URL_SAFE.encode(&bytes)
        );
        check_deferred_custody_with_config(
            &f.state,
            "alias",
            &config(&f, &[("TELEGRAM_SESSION_STRING_ALIAS", different)]),
        )
        .unwrap_or_else(|_| panic!("Distinct authorization was confused at byte {position}"));
        bytes[offset + position] ^= 1;
    }
    assert!(
        check_deferred_custody_with_config(
            &f.state,
            "alias",
            &config(&f, &[("TELEGRAM_SESSION_STRING_ALIAS", original)])
        )
        .is_err()
    );
}

#[test]
fn an_authorization_shared_only_by_a_non_home_datacenter_still_blocks_custody() {
    let f = Fixture::new();
    custody(&f);
    snapshot(&f, "old", Some(0x72), "legacy");
    snapshot(&f, "alias", Some(0x73), "legacy");
    let mut alias = f
        .state
        .read_json("alias.session.json", Value::Null)
        .unwrap();
    let home = alias["home_dc"].as_i64().unwrap() as i32;
    let mut options: Vec<grammers_session::types::DcOption> =
        serde_json::from_value(alias["dc_options"].clone()).unwrap();
    let secondary = options.iter_mut().find(|option| option.id != home).unwrap();
    secondary.auth_key = Some([0x72; 256]);
    alias["dc_options"] = json!(options);
    f.state.write_json("alias.session.json", &alias).unwrap();
    let error =
        check_deferred_custody_with_config(&f.state, "alias", &config(&f, &[])).unwrap_err();
    assert!(error.to_string().contains("existing secret-chat custody"));
    assert_eq!(
        f.state
            .read_json("alias.session.json", Value::Null)
            .unwrap(),
        alias
    );
}
