//! Account publication, legacy configuration and cross-writer regressions.
mod local_support;
use local_support::fixture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use telegram_mcp::{
    account_manager::{
        begin_login, config_digest, parse_config, read_config, recover_logins, recover_rename,
        remove_account, rename_configured_account, write_env_value,
    },
    state::State,
};

fn snapshot(user: i64, key: u8) -> Value {
    let mut data = grammers_session::SessionData::default();
    data.dc_options.get_mut(&data.home_dc).unwrap().auth_key = Some([key; 256]);
    json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[grammers_session::types::PeerInfo::User{id:user,auth:None,bot:Some(false),is_self:Some(true)}],"updates_state":data.updates_state})
}
fn login_marker(state: &State, env: &std::path::Path, installed: Value) -> String {
    let stage = "__login_recovery_fixture";
    let mut staged = installed.clone();
    staged["authorization_source"] = Value::Null;
    state
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    let name = "account-login-work-transaction.json";
    state.write_json(name,&json!({"account":"work","stage":stage,"stage_digest":config_digest(&serde_json::to_string(&staged).unwrap()),"env_file":env,"config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":config_digest("work"),"installed":installed})).unwrap();
    name.to_owned()
}
fn bound() -> Value {
    let mut value = snapshot(42, 7);
    value["authorization_source"] = json!(format!(
        "TELEGRAM_SESSION_NAME_WORK:{}",
        config_digest("work")
    ));
    value
}

fn completed_login_fixture(state: &State, stage: &str, staged: &Value) -> Value {
    use sha2::{Digest, Sha256};
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        state.root().as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let path = {
        use std::os::windows::ffi::OsStrExt;
        state
            .root()
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    json!({"version":2,"phase":"completed","owner":format!("{:x}",Sha256::digest(path)),"account":"work","stage":stage,"stage_digest":config_digest(&serde_json::to_string(staged).unwrap()),"committed":true})
}

#[test]
fn resurrected_completed_login_receipt_ignores_removed_config_and_preserves_later_authorization() {
    use fs2::FileExt;
    for replace_authorization in [false, true] {
        let (dir, state) = fixture();
        let env = dir.path().join("selected.env");
        std::fs::write(&env, "OTHER=keep\n").unwrap();
        let login = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
        let stage = login.stage_label().to_owned();
        let staged = snapshot(42, 7);
        state
            .write_json(&format!("{stage}.session.json"), &staged)
            .unwrap();
        let receipt = completed_login_fixture(&state, &stage, &staged);
        assert_eq!(login.commit(42).unwrap()["saved"], true);
        assert!(
            !state
                .root()
                .join("account-login-work-transaction.json")
                .exists()
        );
        let mut newer = if replace_authorization {
            snapshot(77, 9)
        } else {
            state.read_json("work.session.json", Value::Null).unwrap()
        };
        newer["updates_state"]["qts"] = json!(123);
        state.write_json("work.session.json", &newer).unwrap();
        std::fs::remove_file(&env).unwrap();
        let lease = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.root().join("work.session.lock"))
            .unwrap();
        lease.try_lock_exclusive().unwrap();
        state
            .write_json("account-login-work-transaction.json", &receipt)
            .unwrap();
        recover_logins(&state).unwrap();
        assert_eq!(
            state.read_json("work.session.json", Value::Null).unwrap(),
            newer
        );
        assert!(
            !state
                .root()
                .join("account-login-work-transaction.json")
                .exists()
        );
        assert!(
            !env.exists(),
            "Historical receipt recreated removed configuration"
        );
        drop(lease);
    }
}

#[test]
fn completed_login_cleanup_requires_its_reserved_stage_lease_and_unchanged_stage_digest() {
    use fs2::FileExt;
    let (_dir, state) = fixture();
    let stage = "__login_receipt_fixture";
    let staged = snapshot(42, 7);
    let receipt = completed_login_fixture(&state, stage, &staged);
    let target = snapshot(77, 9);
    state.write_json("work.session.json", &target).unwrap();
    state
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.root().join(format!("{stage}.session.lock")))
        .unwrap();
    lease.try_lock_exclusive().unwrap();
    state
        .write_json("account-login-work-transaction.json", &receipt)
        .unwrap();
    assert!(recover_logins(&state).is_err());
    assert_eq!(
        state
            .read_json(&format!("{stage}.session.json"), Value::Null)
            .unwrap(),
        staged
    );
    drop(lease);
    for changed in [snapshot(88, 11), Value::Null, json!(false)] {
        state
            .write_json(&format!("{stage}.session.json"), &changed)
            .unwrap();
        assert!(recover_logins(&state).is_err());
        assert!(state.root().join(format!("{stage}.session.json")).exists());
        assert_eq!(
            state
                .read_json(&format!("{stage}.session.json"), Value::Null)
                .unwrap(),
            changed
        );
        assert_eq!(
            state.read_json("work.session.json", Value::Null).unwrap(),
            target
        );
        assert!(
            state
                .root()
                .join("account-login-work-transaction.json")
                .exists()
        );
    }
    state
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    recover_logins(&state).unwrap();
    assert!(!state.root().join(format!("{stage}.session.json")).exists());
    assert!(
        !state
            .root()
            .join("account-login-work-transaction.json")
            .exists()
    );
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        target
    );
}

#[test]
fn malformed_or_copied_completed_login_receipts_preserve_all_native_and_staged_data() {
    let (_dir, state) = fixture();
    let stage = "__login_receipt_fixture";
    let staged = snapshot(42, 7);
    let target = snapshot(77, 9);
    state.write_json("work.session.json", &target).unwrap();
    state
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    let valid = completed_login_fixture(&state, stage, &staged);
    let mut invalid = Vec::new();
    for (field, value) in [
        ("version", json!(3)),
        ("owner", json!("0".repeat(64))),
        ("account", json!("personal")),
        ("stage", json!("work")),
        ("stage_digest", json!("invalid")),
        ("committed", json!("true")),
        ("phase", json!("unknown")),
    ] {
        let mut receipt = valid.clone();
        receipt[field] = value;
        invalid.push(receipt);
    }
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("version");
    missing["env_file"] = json!("must-not-be-read");
    invalid.push(missing);
    let mut extra = valid.clone();
    extra["installed"] = target.clone();
    invalid.push(extra);
    for receipt in invalid {
        state
            .write_json("account-login-work-transaction.json", &receipt)
            .unwrap();
        assert!(recover_logins(&state).is_err());
        assert_eq!(
            state
                .read_json("account-login-work-transaction.json", Value::Null)
                .unwrap(),
            receipt
        );
        assert_eq!(
            state.read_json("work.session.json", Value::Null).unwrap(),
            target
        );
        assert_eq!(
            state
                .read_json(&format!("{stage}.session.json"), Value::Null)
                .unwrap(),
            staged
        );
    }
    let (_other_dir, other) = fixture();
    other.write_json("work.session.json", &target).unwrap();
    other
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    other
        .write_json("account-login-work-transaction.json", &valid)
        .unwrap();
    assert!(recover_logins(&other).is_err());
    assert_eq!(
        other.read_json("work.session.json", Value::Null).unwrap(),
        target
    );
    assert_eq!(
        other
            .read_json(&format!("{stage}.session.json"), Value::Null)
            .unwrap(),
        staged
    );
}

#[test]
fn legacy_bare_dollar_names_remain_literal_while_braced_names_expand() {
    let values = parse_config(
        "ARCHIVE=archive\nBARE=/tmp/$ARCHIVE\nQUOTED=\"$ARCHIVE/${ARCHIVE}\"\n",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(values["BARE"], "/tmp/$ARCHIVE");
    assert_eq!(values["QUOTED"], "$ARCHIVE/archive");
}

#[test]
fn shared_bind_address_resolves_selected_file_interpolation_and_process_precedence() {
    if let Some(path) = std::env::var_os("TG_TEST_BIND_CONFIG") {
        telegram_mcp::account_manager::select_source(std::path::Path::new(&path)).unwrap();
        telegram_mcp::account_manager::capture_process_config();
        assert_eq!(
            telegram_mcp::account_manager::bind_address()
                .unwrap()
                .to_string(),
            std::env::var("TG_TEST_BIND_EXPECTED").unwrap()
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = dir.path().join("selected.env");
    std::fs::write(
        &env,
        "BIND_HOST=127.0.0.1\nMCP_HOST=${BIND_HOST}\nMCP_PORT=45678\n",
    )
    .unwrap();
    for override_process in [false, true] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "shared_bind_address_resolves_selected_file_interpolation_and_process_precedence",
                "--nocapture",
            ])
            .env("TG_TEST_BIND_CONFIG", &env)
            .env_remove("MCP_HOST")
            .env_remove("MCP_PORT");
        if override_process {
            child
                .env("MCP_HOST", "::1")
                .env("MCP_PORT", "9876")
                .env("TG_TEST_BIND_EXPECTED", "[::1]:9876");
        } else {
            child.env("TG_TEST_BIND_EXPECTED", "127.0.0.1:45678");
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn login_rejects_labels_without_a_valid_configuration_key_before_staging() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "OTHER=kept\n").unwrap();
    assert!(begin_login(state.clone(), "work-test", &env, None, None, None).is_err());
    assert_eq!(std::fs::read_to_string(env).unwrap(), "OTHER=kept\n");
    assert!(!std::fs::read_dir(state.root()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("login")
    }));
}

#[test]
fn pending_login_cannot_be_renamed_away_from_its_recovery_journal() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "OTHER=kept\n").unwrap();
    let installed = bound();
    state.write_json("work.session.json", &installed).unwrap();
    let marker = login_marker(&state, &env, installed.clone());
    assert!(telegram_mcp::account_manager::rename_account(&state, "work", "other").is_err());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        installed
    );
    assert!(state.root().join(marker).is_file());
    assert!(!state.root().join("other.session.json").exists());
    recover_logins(&state).unwrap();
    assert!(!state.root().join("work.session.json").exists());
}

#[tokio::test]
async fn unprobed_legacy_file_session_is_unknown_instead_of_invalid() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    std::fs::write(dir.path().join("work.session"), b"unprobed legacy fixture").unwrap();
    let answer = telegram_mcp::account_manager::session_health_with_state(
        &state,
        &env,
        "TELEGRAM_SESSION_NAME_WORK",
    )
    .await
    .unwrap();
    assert_eq!(answer["status"], "unknown");
    assert_eq!(answer["repairable"], false);
    assert!(!state.root().join("work.session.json").exists());
}

#[tokio::test]
async fn supported_loopback_https_discovery_reports_transport_failure_as_unverified() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    let original = bound();
    state.write_json("work.session.json", &original).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    state
        .write_json(
            "http-service.json",
            &json!({
                "url":format!("https://{address}/mcp"), "token":"h".repeat(64), "health_protocol":2,
            }),
        )
        .unwrap();
    let answer = telegram_mcp::account_manager::session_health_with_state(
        &state,
        &env,
        "TELEGRAM_SESSION_NAME_WORK",
    )
    .await
    .unwrap();
    assert_eq!(answer["status"], "busy");
    assert_eq!(answer["repairable"], false);
}

#[test]
fn removal_archives_authorization_and_scoped_records_before_freeing_the_label() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    std::fs::write(
        &env,
        "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=\"first\nsecond\"\n",
    )
    .unwrap();
    let original = bound();
    state.write_json("work.session.json", &original).unwrap();
    state
        .write_json(
            "ghost.json",
            &json!({"accounts":{"work":true,"personal":false}}),
        )
        .unwrap();
    let result = remove_account(&state, "work", &env, "TELEGRAM_SESSION_NAME_WORK").unwrap();
    let archive = result["archived"].as_str().unwrap();
    assert!(archive.starts_with("__removed_work_"));
    assert!(archive.len() <= 64);
    assert_eq!(result["removed"], true);
    assert!(!state.root().join("work.session.json").exists());
    let saved = state
        .read_json(&format!("{archive}.session.json"), Value::Null)
        .unwrap();
    assert_eq!(saved["dc_options"], original["dc_options"]);
    assert_eq!(
        state.read_json("ghost.json", Value::Null).unwrap()["accounts"][archive],
        true
    );
    assert_eq!(
        state.read_json("ghost.json", Value::Null).unwrap()["accounts"]["personal"],
        false
    );
    let config = read_config(&env).unwrap();
    assert!(!config.values.contains_key("TELEGRAM_SESSION_NAME_WORK"));
    assert_eq!(config.values["OTHER"], "first\nsecond");
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
}

#[test]
fn removal_refuses_a_process_owned_account_without_changing_files_or_authorization() {
    if let Some(root) = std::env::var_os("TG_TEST_REMOVAL_PROCESS_ROOT") {
        let root = std::path::PathBuf::from(root);
        let state = State::at(root.join("private"), Vec::new()).unwrap();
        let env = root.join("selected.env");
        let original = std::fs::read(&env).unwrap();
        let session = state.read_json("work.session.json", Value::Null).unwrap();
        assert!(remove_account(&state, "work", &env, "TELEGRAM_SESSION_NAME_WORK").is_err());
        assert_eq!(std::fs::read(env).unwrap(), original);
        assert_eq!(
            state.read_json("work.session.json", Value::Null).unwrap(),
            session
        );
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
        return;
    }
    let (dir, state) = fixture();
    std::fs::write(
        dir.path().join("selected.env"),
        "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=kept\n",
    )
    .unwrap();
    state.write_json("work.session.json", &bound()).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "removal_refuses_a_process_owned_account_without_changing_files_or_authorization",
            "--nocapture",
        ])
        .env("TG_TEST_REMOVAL_PROCESS_ROOT", dir.path())
        .env("TELEGRAM_SESSION_NAME_WORK", "process-owned-work")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn removal_refuses_an_active_account_and_symlinked_configuration_before_archival() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=kept\n").unwrap();
    state.write_json("work.session.json", &bound()).unwrap();
    let lease = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.root().join("work.session.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lease).unwrap();
    assert!(remove_account(&state, "work", &env, "TELEGRAM_SESSION_NAME_WORK").is_err());
    assert!(state.root().join("work.session.json").is_file());
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
    drop(lease);
    #[cfg(unix)]
    {
        let link = dir.path().join("linked.env");
        std::os::unix::fs::symlink(&env, &link).unwrap();
        assert!(remove_account(&state, "work", &link, "TELEGRAM_SESSION_NAME_WORK").is_err());
        assert!(state.root().join("work.session.json").is_file());
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
    }
}

fn interrupted_removal(
    state: &State,
    env: &std::path::Path,
    moved: bool,
) -> (&'static str, Value, Value) {
    let archive = "__removed_work_fixture";
    let before = json!({"accounts":{"work":true,"personal":false}});
    let after = json!({"accounts":{archive:true,"personal":false}});
    state
        .write_json("work.session.json", &snapshot(42, 7))
        .unwrap();
    state
        .write_json("ghost.json", if moved { &after } else { &before })
        .unwrap();
    if moved {
        std::fs::rename(
            state.root().join("work.session.json"),
            state.root().join(format!("{archive}.session.json")),
        )
        .unwrap();
    }
    state.write_json("account-rename-transaction.json", &json!({
        "from":"work", "to":archive,
        "files":[["work.session.json",format!("{archive}.session.json")]],
        "rewrites":[{"name":"ghost.json","before":before,"after":after}],
        "removal":{"env_file":env,"key":"TELEGRAM_SESSION_NAME_WORK","digest":config_digest("work")},
    })).unwrap();
    (archive, before, after)
}

#[test]
fn interrupted_removal_rolls_back_archival_but_preserves_unrelated_configuration_edits() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=concurrent\n").unwrap();
    let (archive, before, _) = interrupted_removal(&state, &env, true);
    recover_rename(&state).unwrap();
    assert!(state.root().join("work.session.json").is_file());
    assert!(
        !state
            .root()
            .join(format!("{archive}.session.json"))
            .exists()
    );
    assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), before);
    assert_eq!(read_config(&env).unwrap().values["OTHER"], "concurrent");
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
}

#[test]
fn committed_removal_finishes_archival_after_a_crash_without_restoring_the_deleted_key() {
    for moved in [false, true] {
        let (dir, state) = fixture();
        let env = dir.path().join("selected.env");
        std::fs::write(&env, "OTHER=concurrent\n").unwrap();
        let (archive, _, after) = interrupted_removal(&state, &env, moved);
        recover_rename(&state).unwrap();
        assert!(!state.root().join("work.session.json").exists());
        assert!(
            state
                .root()
                .join(format!("{archive}.session.json"))
                .is_file()
        );
        assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), after);
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "OTHER=concurrent\n");
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
    }
}

#[test]
fn removal_recovery_preserves_a_replaced_or_missing_configuration_and_its_journal() {
    for content in [Some("TELEGRAM_SESSION_NAME_WORK=replaced\n"), None] {
        let (dir, state) = fixture();
        let env = dir.path().join("selected.env");
        if let Some(content) = content {
            std::fs::write(&env, content).unwrap();
        }
        let (archive, _, after) = interrupted_removal(&state, &env, true);
        assert!(recover_rename(&state).is_err());
        assert!(!state.root().join("work.session.json").exists());
        assert!(
            state
                .root()
                .join(format!("{archive}.session.json"))
                .is_file()
        );
        assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), after);
        assert!(
            state
                .root()
                .join("account-rename-transaction.json")
                .is_file()
        );
        if let Some(content) = content {
            assert_eq!(std::fs::read_to_string(&env).unwrap(), content);
        }
    }
}

#[test]
fn configured_rename_commits_the_source_key_and_native_binding_without_rewriting_values() {
    use base64::Engine;
    for prefix in ["TELEGRAM_SESSION_NAME", "TELEGRAM_SESSION_STRING"] {
        let (dir, state) = fixture();
        let env = dir.path().join("selected.env");
        let key = format!("{prefix}_WoRk");
        let target = format!("{prefix}_OFFICE");
        let value = if prefix.ends_with("NAME") {
            r"C:\Fixtures\work.session".to_owned()
        } else {
            let mut bytes = vec![2, 149, 154, 167, 41, 1, 187];
            bytes.extend([7; 256]);
            format!(
                "1{}",
                base64::engine::general_purpose::URL_SAFE.encode(bytes)
            )
        };
        let original = format!(
            "\u{feff}# retained\r\nexport {key} = '{value}' # original spelling\r\nOTHER=\"first\r\nsecond\"\r\n"
        );
        std::fs::write(&env, &original).unwrap();
        let mut session = snapshot(42, 7);
        session["authorization_source"] = json!(format!("{key}:{}", config_digest(&value)));
        state.write_json("work.session.json", &session).unwrap();
        rename_configured_account(&state, "work", "office", &env, &key).unwrap();
        assert_eq!(
            std::fs::read_to_string(&env).unwrap(),
            original.replace(&key, &target)
        );
        assert!(!state.root().join("work.session.json").exists());
        let renamed = state.read_json("office.session.json", Value::Null).unwrap();
        assert_eq!(renamed["dc_options"], session["dc_options"]);
        assert_eq!(
            renamed["authorization_source"],
            format!("{target}:{}", config_digest(&value))
        );
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
    }
}

#[test]
fn configured_rename_preserves_active_references_and_literal_configuration_text() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    let key = "TELEGRAM_SESSION_NAME_WORK";
    let target = "TELEGRAM_SESSION_NAME_OFFICE";
    let original = concat!(
        "BEFORE=${TELEGRAM_SESSION_NAME_WORK:-before}\n",
        "TELEGRAM_SESSION_NAME_WORK=work\n",
        "TELEGRAM_STATE_DIR=/srv/${TELEGRAM_SESSION_NAME_WORK}\n",
        "FALLBACK=${TELEGRAM_SESSION_NAME_WORK:-fallback}\n",
        "QUOTED=\"escaped \\\" ${TELEGRAM_SESSION_NAME_WORK} \\${TELEGRAM_SESSION_NAME_WORK}\"\n",
        "SINGLE='${TELEGRAM_SESSION_NAME_WORK}'\n",
        "BARE=$TELEGRAM_SESSION_NAME_WORK\n",
        "COMMENT=value # ${TELEGRAM_SESSION_NAME_WORK}\n",
        "# ${TELEGRAM_SESSION_NAME_WORK}\n",
    );
    std::fs::write(&env, original).unwrap();
    state
        .write_json("work.session.json", &snapshot(42, 7))
        .unwrap();
    let before = read_config(&env).unwrap().values;
    rename_configured_account(&state, "work", "office", &env, key).unwrap();
    let after = read_config(&env).unwrap().values;
    for (name, value) in &before {
        assert_eq!(
            after.get(if name == key { target } else { name }),
            Some(value),
            "Resolved setting changed: {name}"
        );
    }
    let written = std::fs::read_to_string(&env).unwrap();
    assert!(written.contains("TELEGRAM_STATE_DIR=/srv/${TELEGRAM_SESSION_NAME_OFFICE}"));
    assert!(written.contains("BEFORE=${TELEGRAM_SESSION_NAME_OFFICE:-before}"));
    for literal in [
        "SINGLE='${TELEGRAM_SESSION_NAME_WORK}'",
        "BARE=$TELEGRAM_SESSION_NAME_WORK",
        "COMMENT=value # ${TELEGRAM_SESSION_NAME_WORK}",
        "# ${TELEGRAM_SESSION_NAME_WORK}",
    ] {
        assert!(written.contains(literal));
    }
}

#[test]
fn configured_rename_rejects_new_reference_meaning_before_moving_any_state() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    let original = "TELEGRAM_SESSION_NAME_WORK=work\nARCHIVE_DIR=/srv/${TELEGRAM_SESSION_NAME_OFFICE:-previous}\n";
    std::fs::write(&env, original).unwrap();
    let native = snapshot(42, 7);
    state.write_json("work.session.json", &native).unwrap();
    assert!(
        rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&env).unwrap(), original);
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        native
    );
    assert!(!state.root().join("office.session.json").exists());
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
}

fn interrupted_configured_rename(
    state: &State,
    env: &std::path::Path,
    moved: bool,
) -> (Value, Value) {
    let before = json!({"accounts":{"work":true}});
    let after = json!({"accounts":{"office":true}});
    state
        .write_json("work.session.json", &snapshot(42, 7))
        .unwrap();
    state
        .write_json("ghost.json", if moved { &after } else { &before })
        .unwrap();
    if moved {
        std::fs::rename(
            state.root().join("work.session.json"),
            state.root().join("office.session.json"),
        )
        .unwrap();
    }
    state.write_json("account-rename-transaction.json",&json!({
        "from":"work","to":"office","files":[["work.session.json","office.session.json"]],
        "rewrites":[{"name":"ghost.json","before":before,"after":after}],
        "configuration":{"env_file":env,"key":"TELEGRAM_SESSION_NAME_WORK","digest":config_digest("work"),"new_key":"TELEGRAM_SESSION_NAME_OFFICE"},
    })).unwrap();
    (before, after)
}

#[test]
fn configured_rename_recovery_rolls_back_state_before_the_key_commit_without_clobbering_env() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    let current = "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=concurrent\n";
    std::fs::write(&env, current).unwrap();
    let (before, _) = interrupted_configured_rename(&state, &env, true);
    recover_rename(&state).unwrap();
    assert!(state.root().join("work.session.json").is_file());
    assert!(!state.root().join("office.session.json").exists());
    assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), before);
    assert_eq!(std::fs::read_to_string(&env).unwrap(), current);
}

#[test]
fn configured_rename_recovery_finishes_state_after_the_exact_key_commit() {
    for moved in [false, true] {
        let (dir, state) = fixture();
        let env = dir.path().join("selected.env");
        let current = "TELEGRAM_SESSION_NAME_OFFICE=work\nOTHER=concurrent\n";
        std::fs::write(&env, current).unwrap();
        let (_, after) = interrupted_configured_rename(&state, &env, moved);
        recover_rename(&state).unwrap();
        assert!(!state.root().join("work.session.json").exists());
        assert!(state.root().join("office.session.json").is_file());
        assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), after);
        assert_eq!(std::fs::read_to_string(&env).unwrap(), current);
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
    }
}

#[test]
fn configured_rename_recovery_preserves_conflicting_or_ambiguous_configuration() {
    for current in [
        "TELEGRAM_SESSION_NAME_WORK=changed\n",
        "TELEGRAM_SESSION_NAME_OFFICE=changed\n",
        "OTHER=kept\n",
        "TELEGRAM_SESSION_NAME_WORK=work\nTELEGRAM_SESSION_NAME_OFFICE=work\n",
    ] {
        let (dir, state) = fixture();
        let env = dir.path().join("selected.env");
        std::fs::write(&env, current).unwrap();
        let (_, after) = interrupted_configured_rename(&state, &env, true);
        assert!(recover_rename(&state).is_err());
        assert!(state.root().join("office.session.json").is_file());
        assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), after);
        assert_eq!(std::fs::read_to_string(&env).unwrap(), current);
        assert!(
            state
                .root()
                .join("account-rename-transaction.json")
                .is_file()
        );
    }
}

#[test]
fn configured_rename_refuses_a_destination_source_before_publishing_any_state() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    let current = "TELEGRAM_SESSION_NAME_WORK=work\nTELEGRAM_SESSION_NAME_OFFICE=other\n";
    std::fs::write(&env, current).unwrap();
    state
        .write_json("work.session.json", &snapshot(42, 7))
        .unwrap();
    assert!(
        rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
            .is_err()
    );
    assert!(state.root().join("work.session.json").is_file());
    assert!(!state.root().join("office.session.json").exists());
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
    assert_eq!(std::fs::read_to_string(&env).unwrap(), current);
}

#[test]
fn configured_rename_preserves_a_native_snapshot_bound_to_an_unrelated_account() {
    let (dir, state) = fixture();
    let env = dir.path().join("selected.env");
    let current = "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=kept\n";
    std::fs::write(&env, current).unwrap();
    let mut original = snapshot(42, 7);
    original["authorization_source"] = json!(format!(
        "TELEGRAM_SESSION_NAME_PERSONAL:{}",
        config_digest("work")
    ));
    state.write_json("work.session.json", &original).unwrap();
    assert!(
        rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
            .is_err()
    );
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        original
    );
    assert!(!state.root().join("office.session.json").exists());
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
    assert_eq!(std::fs::read_to_string(&env).unwrap(), current);
}

#[test]
fn dotenv_legacy_windows_paths_and_documented_quote_escapes_are_lossless() {
    let values = parse_config(
        r#"TELEGRAM_STATE_DIR=C:\Fixtures\Demo\state
OTHER="C:\Fixtures\Demo\state"
SINGLE='C:\Fixtures\Demo\state'
APOSTROPHE=C:\Fixtures\O'Neil\state
ESCAPED='it\'s \\ safe'
CONTROLS="\a\b\f\n\r\t\v\'\"\\\z"
"#,
        &BTreeMap::new(),
    )
    .unwrap();
    for key in ["TELEGRAM_STATE_DIR", "OTHER", "SINGLE"] {
        assert_eq!(values[key], r"C:\Fixtures\Demo\state");
    }
    assert_eq!(values["APOSTROPHE"], r"C:\Fixtures\O'Neil\state");
    assert_eq!(values["ESCAPED"], "it's \\ safe");
    assert_eq!(values["CONTROLS"], "\u{7}\u{8}\u{c}\n\r\t\u{b}'\"\\\\z");
    let (dir, _state) = fixture();
    let path = dir.path().join(".env");
    let original = r#"C:\Fixtures\O'Neil\$unexpanded\"literal"#;
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", original, None).unwrap();
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        original
    );
}

#[test]
fn writing_an_accepted_bare_account_key_replaces_it_instead_of_creating_duplicates() {
    for text in [
        "\u{feff}TELEGRAM_SESSION_NAME_WORK\nOTHER=kept\n",
        "export TELEGRAM_SESSION_NAME_WORK\nOTHER=kept\n",
    ] {
        let (dir, _state) = fixture();
        let path = dir.path().join(".env");
        std::fs::write(&path, text).unwrap();
        write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "work", None).unwrap();
        let parsed = read_config(&path).unwrap();
        assert_eq!(parsed.values["TELEGRAM_SESSION_NAME_WORK"], "work");
        assert_eq!(parsed.values["OTHER"], "kept");
        assert_eq!(
            std::fs::read_to_string(path)
                .unwrap()
                .matches("TELEGRAM_SESSION_NAME_WORK")
                .count(),
            1
        );
    }
}

#[test]
fn accepted_source_removal_never_becomes_a_successful_empty_snapshot() {
    let (dir, _state) = fixture();
    let path = dir.path().join("selected.env");
    std::fs::write(&path, "TELEGRAM_API_ID=12345\n").unwrap();
    telegram_mcp::account_manager::select_source(&path).unwrap();
    telegram_mcp::account_manager::capture_process_config();
    assert_eq!(
        telegram_mcp::account_manager::current_config()
            .unwrap()
            .values["TELEGRAM_API_ID"],
        "12345"
    );
    std::fs::remove_file(&path).unwrap();
    assert!(telegram_mcp::account_manager::current_config().is_err());
    std::fs::write(&path, "OTHER=deliberately-empty-accounts\n").unwrap();
    assert_eq!(
        telegram_mcp::account_manager::current_config()
            .unwrap()
            .values["OTHER"],
        "deliberately-empty-accounts"
    );
}

#[test]
fn concurrent_new_logins_cannot_replace_the_first_completed_authorization() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    let first = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
    let second = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
    state
        .write_json(
            &format!("{}.session.json", first.stage_label()),
            &snapshot(42, 7),
        )
        .unwrap();
    state
        .write_json(
            &format!("{}.session.json", second.stage_label()),
            &snapshot(99, 8),
        )
        .unwrap();
    assert_eq!(first.commit(42).unwrap()["saved"], true);
    let installed = state.read_json("work.session.json", Value::Null).unwrap();
    assert!(second.commit(99).is_err());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        installed
    );
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
    assert!(!state.root().join("work.native-backup.json").exists());
}

#[test]
fn interrupted_login_without_config_rolls_back_only_its_exact_orphan_session() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "OTHER=unrelated-owner-write\n").unwrap();
    let installed = bound();
    let marker = login_marker(&state, &env, installed.clone());
    state.write_json("work.session.json", &installed).unwrap();
    recover_logins(&state).unwrap();
    assert!(!state.root().join("work.session.json").exists());
    assert!(!state.root().join(marker).exists());
    assert_eq!(
        std::fs::read_to_string(&env).unwrap(),
        "OTHER=unrelated-owner-write\n"
    );
    assert!(
        !state
            .root()
            .join("__login_recovery_fixture.session.json")
            .exists()
    );
}

#[test]
fn completed_login_recovery_preserves_later_session_cache_and_unrelated_env_writes() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=later\n").unwrap();
    let installed = bound();
    let marker = login_marker(&state, &env, installed.clone());
    let mut evolved = installed.clone();
    evolved["updates_state"]["pts"] = json!(25);
    state.write_json("work.session.json", &evolved).unwrap();
    recover_logins(&state).unwrap();
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        evolved
    );
    assert!(
        std::fs::read_to_string(&env)
            .unwrap()
            .contains("OTHER=later")
    );
    assert!(!state.root().join(marker).exists());
}

#[test]
fn committed_config_with_missing_session_recovers_the_verified_installed_snapshot() {
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    let installed = bound();
    let marker = login_marker(&state, &env, installed.clone());
    recover_logins(&state).unwrap();
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        installed
    );
    assert!(!state.root().join(marker).exists());
}

#[test]
fn login_recovery_never_overwrites_a_different_authorization() {
    for committed in [false, true] {
        let (dir, state) = fixture();
        let env = dir.path().join(".env");
        std::fs::write(
            &env,
            if committed {
                "TELEGRAM_SESSION_NAME_WORK=work\n"
            } else {
                "OTHER=kept\n"
            },
        )
        .unwrap();
        let installed = bound();
        let marker = login_marker(&state, &env, installed);
        let changed = snapshot(99, 9);
        state.write_json("work.session.json", &changed).unwrap();
        let before = std::fs::read(&env).unwrap();
        assert!(recover_logins(&state).is_err());
        assert_eq!(
            state.read_json("work.session.json", Value::Null).unwrap(),
            changed
        );
        assert_eq!(std::fs::read(&env).unwrap(), before);
        assert!(state.root().join(marker).exists());
    }
}

#[test]
fn rename_recovery_refuses_unrelated_record_changes_before_any_rollback() {
    let (_dir, state) = fixture();
    let before = json!({"accounts":{"work":false}});
    let after = json!({"accounts":{"new":false}});
    state.write_json("ghost.json", &after).unwrap();
    state
        .write_json(
            "account-rename-transaction.json",
            &json!({"files":[],"rewrites":[{"name":"ghost.json","before":before,"after":after}]}),
        )
        .unwrap();
    assert!(
        state
            .write_json("unrelated.json", &json!({"new":true}))
            .is_err()
    );
    let unrelated = json!({"accounts":{"new":false,"personal":true}});
    // Model an external/manual edit, deliberately outside normal fenced writers.
    std::fs::write(
        state.root().join("ghost.json"),
        serde_json::to_vec(&unrelated).unwrap(),
    )
    .unwrap();
    assert!(recover_rename(&state).is_err());
    assert_eq!(
        state.read_json("ghost.json", Value::Null).unwrap(),
        unrelated
    );
    assert!(
        state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
}

#[test]
fn backup_retention_is_per_selected_source_and_unrelated_backup_directories_are_ignored() {
    let (dir, _state) = fixture();
    let first = dir.path().join("first.env");
    let second = dir.path().join("second.env");
    write_env_value(&first, "TELEGRAM_SESSION_NAME_WORK", "one", None).unwrap();
    write_env_value(&second, "TELEGRAM_SESSION_NAME_WORK", "other", None).unwrap();
    let trap = dir.path().join("first.env.backup-000000");
    std::fs::create_dir(&trap).unwrap();
    std::fs::write(trap.join("preserve"), "directory").unwrap();
    write_env_value(&first, "TELEGRAM_SESSION_NAME_WORK", "two", None).unwrap();
    write_env_value(&second, "TELEGRAM_SESSION_NAME_WORK", "different", None).unwrap();
    write_env_value(&first, "TELEGRAM_SESSION_NAME_WORK", "three", None).unwrap();
    for prefix in ["first.env.backup-", "second.env.backup-"] {
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().unwrap().is_file()
                    && entry.file_name().to_string_lossy().starts_with(prefix))
                .count(),
            1
        );
    }
    assert_eq!(
        read_config(&first).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "three"
    );
    assert!(trap.join("preserve").exists());
}

#[tokio::test]
async fn native_configuration_writer_respects_the_launchers_shared_byte_lock() {
    use std::{process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    #[cfg(unix)]
    if let Ok(path) = std::env::var("ACCOUNT_REVIEW_LOCK_CHILD") {
        use std::{
            io::{Read, Write},
            os::fd::AsRawFd,
        };
        if std::env::var("ACCOUNT_REVIEW_HOLDER_PROBE").as_deref() == Ok("noisy") {
            std::io::stderr().write_all(&vec![b'n'; 131072]).unwrap();
            std::io::stderr().flush().unwrap();
        }
        println!("READY");
        std::io::stdout().flush().unwrap();
        if std::env::var("ACCOUNT_REVIEW_HOLDER_PROBE").as_deref() == Ok("exit") {
            eprintln!("Synthetic holder exited before acquiring the lock.");
            std::process::exit(19);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        // POSIX record locks are the FileStream.Lock protocol, unlike flock.
        let mut region: libc::flock = unsafe { std::mem::zeroed() };
        region.l_type = libc::F_WRLCK as _;
        region.l_whence = libc::SEEK_SET as _;
        region.l_start = 0;
        region.l_len = 1;
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &region) },
            0
        );
        println!("LOCKED");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        std::io::stdin().read_exact(&mut byte).unwrap();
        return;
    }
    let (dir, _state) = fixture();
    let path = dir.path().join("selected.env");
    std::fs::write(&path, "TELEGRAM_SESSION_NAME_WORK=original\n").unwrap();
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    let lock = std::path::PathBuf::from(lock);
    #[cfg(unix)]
    let mut command = {
        let mut c = tokio::process::Command::new(std::env::current_exe().unwrap());
        c.args([
            "--exact",
            "native_configuration_writer_respects_the_launchers_shared_byte_lock",
            "--nocapture",
        ])
        .env("ACCOUNT_REVIEW_LOCK_CHILD", &lock);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let script = dir.path().join("hold-lock.ps1");
        std::fs::write(&script,r#"param([string] $Path, [string] $Probe)
if ($Probe -eq 'noisy') { [Console]::Error.Write(('n' * 131072)); [Console]::Error.Flush() }
[Console]::Out.WriteLine('READY'); [Console]::Out.Flush()
if ($Probe -eq 'exit') { [Console]::Error.WriteLine('Synthetic holder exited before acquiring the lock.'); [Console]::Error.Flush(); exit 19 }
$stream = [IO.File]::Open($Path, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
try { $stream.Lock(0, 1); [Console]::Out.WriteLine('LOCKED'); [Console]::Out.Flush(); [void] [Console]::ReadLine() }
finally { $stream.Dispose() }
"#).unwrap();
        let mut c = tokio::process::Command::new("pwsh");
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
            .arg(script)
            .arg("-Path")
            .arg(&lock)
            .arg("-Probe")
            .arg(std::env::var("ACCOUNT_REVIEW_HOLDER_PROBE").unwrap_or_default());
        c
    };
    command
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let diagnostics = tokio::spawn(async move {
        let mut retained = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let keep = count.min(4096 - retained.len());
                    retained.extend_from_slice(&chunk[..keep]);
                    // Continue draining after the diagnostic limit: an unread
                    // stderr pipe must not prevent the holder announcing READY.
                }
            }
        }
        retained
    });
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let handshake = async {
        // The latency under test is acquiring the shared byte, not starting a
        // cold PowerShell/.NET runtime while instrumented cases share the CPU.
        for (phase, budget) in [("READY", 30), ("LOCKED", 10)] {
            tokio::time::timeout(Duration::from_secs(budget), async {
                loop {
                    let line = lines
                        .next_line()
                        .await
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("Lock holder exited before {phase}"))?;
                    if line == phase {
                        return Ok::<(), String>(());
                    }
                }
            })
            .await
            .map_err(|_| format!("Lock holder did not announce {phase} within {budget}s"))??;
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(error) = handshake {
        let mut status = child.try_wait().unwrap();
        if status.is_none() {
            let _ = child.start_kill();
            if let Ok(Ok(exited)) = tokio::time::timeout(Duration::from_secs(5), child.wait()).await
            {
                status = Some(exited);
            }
        }
        let stderr = tokio::time::timeout(Duration::from_secs(2), diagnostics)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        panic!(
            "{error}; child status: {status:?}; bounded holder stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
    }
    assert!(write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "replacement", None).is_err());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("=original")
    );
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"\n")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    diagnostics.await.unwrap();
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "replacement", None).unwrap();
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "replacement"
    );
}

#[cfg(windows)]
#[test]
fn a_readonly_old_backup_cannot_fail_an_already_committed_new_login() {
    use std::os::windows::fs::OpenOptionsExt;
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Ok(metadata) = std::fs::metadata(&self.0) {
                let mut permissions = metadata.permissions();
                permissions.set_readonly(false);
                let _ = std::fs::set_permissions(&self.0, permissions);
            }
        }
    }
    let (dir, state) = fixture();
    let env = dir.path().join(".env");
    std::fs::write(&env, "OTHER=original\n").unwrap();
    write_env_value(&env, "TELEGRAM_SESSION_NAME_PERSONAL", "personal", None).unwrap();
    let backup = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".env.backup-")
        })
        .unwrap()
        .path();
    let restore = Restore(backup);
    let mut permissions = std::fs::metadata(&restore.0).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&restore.0, permissions).unwrap();
    // Rust 1.100 can delete readonly Windows files using POSIX deletion. Keep
    // a real open handle that denies delete sharing to force cleanup failure.
    let held_backup = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x00000001 | 0x00000002)
        .open(&restore.0)
        .unwrap();
    assert!(
        std::fs::remove_file(&restore.0).is_err(),
        "The fixture must create a real cleanup failure"
    );
    let transaction = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
    state
        .write_json(
            &format!("{}.session.json", transaction.stage_label()),
            &snapshot(42, 7),
        )
        .unwrap();
    assert_eq!(transaction.commit(42).unwrap()["saved"], true);
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
    assert!(state.root().join("work.session.json").is_file());
    assert!(restore.0.exists());
    write_env_value(&env, "TELEGRAM_SESSION_NAME_PERSONAL", "updated", None).unwrap();
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_PERSONAL"],
        "updated"
    );
    assert_eq!(
        read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
    assert!(restore.0.exists());
    drop(held_backup);
}
