mod local_support;
use base64::Engine;
use local_support::fixture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use telegram_mcp::account_manager::{
    begin_login, config_digest, label_for, parse_config, read_config, recover_rename,
    rename_account, session_validate, write_env_value,
};

#[test]
fn process_values_win_over_conflicting_file_and_interpolation() {
    let process = BTreeMap::from([("TELEGRAM_SESSION_STRING_WORK".into(), "external".into())]);
    let values=parse_config("TELEGRAM_SESSION_STRING_WORK=file\nTELEGRAM_SESSION_NAME_COPY=${TELEGRAM_SESSION_STRING_WORK}\n",&process).unwrap();
    assert_eq!(values["TELEGRAM_SESSION_STRING_WORK"], "external");
    assert_eq!(values["TELEGRAM_SESSION_NAME_COPY"], "external");
}
#[test]
fn file_account_removal_does_not_resurrect_a_previous_file_value() {
    let process = BTreeMap::new();
    let first = parse_config("TELEGRAM_SESSION_NAME_WORK=work\n", &process).unwrap();
    let next = parse_config("OTHER=kept\n", &process).unwrap();
    assert!(first.contains_key("TELEGRAM_SESSION_NAME_WORK"));
    assert!(!next.contains_key("TELEGRAM_SESSION_NAME_WORK"));
}
#[test]
fn same_size_configuration_replacement_changes_the_content_digest() {
    assert_ne!(config_digest("token-a"), config_digest("token-b"));
}
#[test]
fn duplicate_configuration_keys_are_rejected() {
    assert!(
        parse_config(
            "TELEGRAM_SESSION_NAME_WORK=a\nTELEGRAM_SESSION_NAME_WORK=b\n",
            &BTreeMap::new()
        )
        .is_err()
    );
}
#[test]
fn malformed_configuration_does_not_become_empty_accounts() {
    assert!(
        parse_config(
            "TELEGRAM_SESSION_NAME_WORK='unterminated\n",
            &BTreeMap::new()
        )
        .is_err()
    );
    assert!(parse_config("bad key=value\n", &BTreeMap::new()).is_err());
}
#[test]
fn quoted_hash_and_unicode_are_preserved() {
    let values = parse_config(
        "TELEGRAM_SESSION_NAME_WORK=\"نام # یک\"\nOTHER=plain # comment\n",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(values["TELEGRAM_SESSION_NAME_WORK"], "نام # یک");
    assert_eq!(values["OTHER"], "plain");
}
#[test]
fn multiline_quoted_value_is_parsed_without_replacement() {
    let values = parse_config("OTHER=\"first\nsecond\"\n", &BTreeMap::new()).unwrap();
    assert_eq!(values["OTHER"], "first\nsecond");
}
#[test]
fn quoted_variable_literal_does_not_expand() {
    let values = parse_config(
        "OTHER='${MISSING}'\nFALLBACK=${MISSING:-default}\n",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(values["OTHER"], "${MISSING}");
    assert_eq!(values["FALLBACK"], "default");
}
#[test]
fn utf8_bom_and_windows_endings_are_accepted() {
    assert_eq!(
        parse_config(
            "\u{feff}TELEGRAM_SESSION_NAME_WORK=work\r\n",
            &BTreeMap::new()
        )
        .unwrap()["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
}
#[test]
fn only_real_account_keys_map_to_labels() {
    assert_eq!(label_for("TELEGRAM_SESSION_STRING").unwrap(), "default");
    assert_eq!(label_for("TELEGRAM_SESSION_NAME_WORK_2").unwrap(), "work_2");
    assert!(label_for("TELEGRAM_API_HASH").is_err());
    assert!(label_for("TELEGRAM_SESSION_STRING_BAD-LABEL").is_err());
}
#[test]
fn blank_session_import_is_empty() {
    assert_eq!(session_validate(" \n"), "empty");
}
#[test]
fn invalid_session_version_and_lengths_are_refused() {
    assert_eq!(session_validate("2AAAA"), "invalid");
    assert_eq!(session_validate("1AAAA"), "invalid");
    assert_eq!(session_validate("1not base64!"), "invalid");
}
#[test]
fn ipv4_and_ipv6_session_import_formats_validate_without_network() {
    for ip_len in [4, 16] {
        let mut bytes = vec![1];
        bytes.extend(vec![1; ip_len]);
        bytes.extend(443u16.to_be_bytes());
        bytes.extend(vec![7; 256]);
        let value = format!(
            "1{}",
            base64::engine::general_purpose::URL_SAFE.encode(bytes)
        );
        assert_eq!(session_validate(&value), "valid");
    }
}
#[test]
fn invalid_datacenter_or_zero_port_is_refused() {
    let mut bytes = vec![1];
    bytes.extend([127, 0, 0, 1]);
    bytes.extend([0, 0]);
    bytes.extend(vec![7; 256]);
    assert_eq!(
        session_validate(&format!(
            "1{}",
            base64::engine::general_purpose::URL_SAFE.encode(&bytes)
        )),
        "invalid"
    );
    bytes[0] = 99;
    bytes[5] = 1;
    assert_eq!(
        session_validate(&format!(
            "1{}",
            base64::engine::general_purpose::URL_SAFE.encode(bytes)
        )),
        "invalid"
    );
}

#[test]
fn env_transaction_preserves_bom_endings_comments_and_other_keys() {
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    let original =
        "\u{feff}# محفوظ\r\nTELEGRAM_SESSION_NAME_WORK=old\r\nOTHER='untouched # literal'\r\n";
    std::fs::write(&path, original).unwrap();
    write_env_value(
        &path,
        "TELEGRAM_SESSION_NAME_WORK",
        "new",
        Some(&config_digest("old")),
    )
    .unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.starts_with("\u{feff}# محفوظ\r\n"));
    assert!(after.contains("OTHER='untouched # literal'\r\n"));
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "new"
    );
}
#[test]
fn env_transaction_preserves_dollar_quotes_and_backslashes() {
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    let value = "a\\b'\"$not_expanded";
    write_env_value(&path, "TELEGRAM_SESSION_STRING_WORK", value, None).unwrap();
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_STRING_WORK"],
        value
    );
}
#[test]
fn stale_env_generation_is_not_replaced() {
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    std::fs::write(&path, "TELEGRAM_SESSION_NAME_WORK=current\n").unwrap();
    assert!(
        write_env_value(
            &path,
            "TELEGRAM_SESSION_NAME_WORK",
            "new",
            Some(&config_digest("stale"))
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "TELEGRAM_SESSION_NAME_WORK=current\n"
    );
}
#[test]
fn invalid_utf8_env_is_preserved_on_failed_write() {
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "new", None).is_err());
    assert_eq!(std::fs::read(path).unwrap(), [0xff, 0xfe]);
}
#[test]
fn env_write_rejects_multiline_injection() {
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    assert!(
        write_env_value(
            &path,
            "TELEGRAM_SESSION_NAME_WORK",
            "work\nOTHER=injected",
            None
        )
        .is_err()
    );
    assert!(!path.exists());
}
#[test]
fn env_backups_are_private_and_bounded_to_one_revision() {
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "one", None).unwrap();
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "two", None).unwrap();
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "three", None).unwrap();
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".env.backup-"))
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);
    let text = std::fs::read_to_string(backups[0].path()).unwrap();
    assert!(text.contains("two"));
}

#[test]
fn rename_moves_native_session_and_account_scoped_stores() {
    let (_dir, state) = fixture();
    state
        .write_json("old.session.json", &json!({"fixture":"private"}))
        .unwrap();
    state.set_ghost_mode(false, Some("old"), None).unwrap();
    state
        .alias_set("old", "close friend", json!({"user_id":"42"}), false)
        .unwrap();
    let moved = rename_account(&state, "OLD", "NEW").unwrap();
    assert!(moved.iter().any(|name| name == "old.session.json"));
    assert!(!state.root().join("old.session.json").exists());
    assert_eq!(
        state.read_json("new.session.json", Value::Null).unwrap()["fixture"],
        "private"
    );
    assert_eq!(state.ghost_mode(Some("new"), None)["effective"], false);
    assert!(state.alias_lookup("new", "close friend").unwrap().is_some());
    assert!(state.alias_lookup("old", "close friend").unwrap().is_none());
}
#[test]
fn rename_destination_conflict_leaves_every_source_unchanged() {
    let (_dir, state) = fixture();
    state
        .write_json("old.session.json", &json!({"old":true}))
        .unwrap();
    state
        .write_json("new.session.json", &json!({"new":true}))
        .unwrap();
    state.set_ghost_mode(false, Some("old"), None).unwrap();
    assert!(rename_account(&state, "old", "new").is_err());
    assert_eq!(
        state.read_json("old.session.json", Value::Null).unwrap(),
        json!({"old":true})
    );
    assert_eq!(state.ghost_mode(Some("old"), None)["effective"], false);
}
#[test]
fn corrupt_metadata_refuses_rename_before_moving_session() {
    let (_dir, state) = fixture();
    state
        .write_json("old.session.json", &json!({"old":true}))
        .unwrap();
    std::fs::write(state.root().join("ghost.json"), b"broken").unwrap();
    assert!(rename_account(&state, "old", "new").is_err());
    assert!(state.root().join("old.session.json").exists());
    assert!(!state.root().join("new.session.json").exists());
}
#[test]
fn live_account_lease_blocks_rename() {
    use fs2::FileExt;
    let (_dir, state) = fixture();
    state
        .write_json("old.session.json", &json!({"old":true}))
        .unwrap();
    let lease = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.root().join("old.session.lock"))
        .unwrap();
    FileExt::lock_exclusive(&lease).unwrap();
    assert!(rename_account(&state, "old", "new").is_err());
    assert!(state.root().join("old.session.json").exists());
}
#[test]
fn crash_journal_restores_files_and_metadata() {
    let (_dir, state) = fixture();
    state
        .write_json("new.session.json", &json!({"keys":"original"}))
        .unwrap();
    state
        .write_json(
            "ghost.json",
            &json!({"accounts":{"new":false},"chats":{},"default":true}),
        )
        .unwrap();
    let before = json!({"accounts":{"old":false},"chats":{},"default":true});
    state.write_json("account-rename-transaction.json",&json!({"files":[["old.session.json","new.session.json"]],"rewrites":[{"name":"ghost.json","before":before,"after":{}}]})).unwrap();
    recover_rename(&state).unwrap();
    assert!(state.root().join("old.session.json").exists());
    assert!(!state.root().join("new.session.json").exists());
    assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), before);
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
}
#[test]
fn deferred_secret_sidecars_are_renamed_without_interpretation() {
    let (_dir, state) = fixture();
    std::fs::create_dir_all(state.root().join("secret-chats/old")).unwrap();
    std::fs::write(state.root().join("secret-chats/old/opaque.bin"), [1, 2, 3]).unwrap();
    std::fs::write(state.root().join("secret-chats/old-media.json"), b"opaque").unwrap();
    rename_account(&state, "old", "new").unwrap();
    assert_eq!(
        std::fs::read(state.root().join("secret-chats/new/opaque.bin")).unwrap(),
        [1, 2, 3]
    );
    assert_eq!(
        std::fs::read(state.root().join("secret-chats/new-media.json")).unwrap(),
        b"opaque"
    );
}

#[test]
fn repair_without_known_identity_refuses_before_authentication() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    assert!(
        begin_login(
            state,
            "work",
            &env,
            Some("TELEGRAM_SESSION_NAME_WORK"),
            None,
            None
        )
        .is_err()
    );
}
#[test]
fn wrong_replacement_key_cannot_target_another_account() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_PERSONAL=personal\n").unwrap();
    assert!(
        begin_login(
            state,
            "work",
            &env,
            Some("TELEGRAM_SESSION_NAME_PERSONAL"),
            None,
            Some(42)
        )
        .is_err()
    );
}
#[test]
fn login_identity_mismatch_preserves_original_keys() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    let original = json!({"peer_infos":[{"User":{"id":42,"is_self":true}}],"keys":"old"});
    state.write_json("work.session.json", &original).unwrap();
    let transaction = begin_login(
        state.clone(),
        "work",
        &env,
        Some("TELEGRAM_SESSION_NAME_WORK"),
        None,
        Some(42),
    )
    .unwrap();
    state
        .write_json(
            &format!("{}.session.json", transaction.stage_label()),
            &json!({"keys":"new"}),
        )
        .unwrap();
    assert!(transaction.commit(99).is_err());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        original
    );
}
#[test]
fn login_source_changed_during_authentication_preserves_original_keys() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    let transaction = begin_login(
        state.clone(),
        "work",
        &env,
        Some("TELEGRAM_SESSION_NAME_WORK"),
        None,
        Some(42),
    )
    .unwrap();
    state
        .write_json(
            &format!("{}.session.json", transaction.stage_label()),
            &json!({"keys":"new"}),
        )
        .unwrap();
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=changed\n").unwrap();
    assert!(transaction.commit(42).is_err());
    assert!(!state.root().join("work.session.json").exists());
}

#[test]
fn new_login_commit_persists_verified_identity_and_configuration_binding() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    let transaction = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
    let mut data = grammers_session::SessionData::default();
    data.dc_options.get_mut(&data.home_dc).unwrap().auth_key = Some([7; 256]);
    let peer = grammers_session::types::PeerInfo::User {
        id: 42,
        auth: None,
        bot: Some(false),
        is_self: Some(true),
    };
    let snapshot = json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[peer],"updates_state":data.updates_state});
    let stage = transaction.stage_label().to_owned();
    state
        .write_json(&format!("{stage}.session.json"), &snapshot)
        .unwrap();
    assert_eq!(transaction.commit(42).unwrap()["saved"], true);
    let saved = state.read_json("work.session.json", Value::Null).unwrap();
    assert_eq!(
        saved["authorization_source"],
        format!("TELEGRAM_SESSION_NAME_WORK:{}", config_digest("work"))
    );
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
    assert!(!state.root().join(format!("{stage}.session.json")).exists());
}

#[test]
fn explicitly_selected_configuration_source_is_used_for_the_current_snapshot() {
    let (dir, _state) = fixture();
    let selected = dir.path().join("selected.env");
    std::fs::write(&selected, "TELEGRAM_SESSION_NAME_SELECTED=selected\n").unwrap();
    telegram_mcp::account_manager::select_source(&selected).unwrap();
    telegram_mcp::account_manager::capture_process_config();
    let snapshot = telegram_mcp::account_manager::current_config().unwrap();
    assert_eq!(snapshot.path, selected);
    assert_eq!(
        snapshot.values["TELEGRAM_SESSION_NAME_SELECTED"],
        "selected"
    );
    assert!(telegram_mcp::account_manager::select_source(&dir.path().join("other.env")).is_err());
}
