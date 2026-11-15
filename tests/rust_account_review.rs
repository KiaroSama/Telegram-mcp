//! Account publication, legacy configuration and cross-writer regressions.
mod local_support;
use local_support::fixture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use telegram_mcp::{
    account_manager::{
        begin_login, config_digest, parse_config, read_config, recover_logins, recover_rename,
        write_env_value,
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
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    #[cfg(unix)]
    if let Ok(path) = std::env::var("ACCOUNT_REVIEW_LOCK_CHILD") {
        use std::{
            io::{Read, Write},
            os::fd::AsRawFd,
        };
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
        std::fs::write(&script,r#"param([string] $Path)
$stream = [IO.File]::Open($Path, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
try { $stream.Lock(0, 1); [Console]::Out.WriteLine('LOCKED'); [Console]::Out.Flush(); [void] [Console]::ReadLine() }
finally { $stream.Dispose() }
"#).unwrap();
        let mut c = tokio::process::Command::new("pwsh");
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
            .arg(script)
            .arg("-Path")
            .arg(&lock);
        c
    };
    command
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("Lock holder exited before acquiring the shared byte");
            if line == "LOCKED" {
                break;
            }
        }
    })
    .await
    .unwrap();
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
    write_env_value(&path, "TELEGRAM_SESSION_NAME_WORK", "replacement", None).unwrap();
    assert_eq!(
        read_config(&path).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "replacement"
    );
}

#[cfg(windows)]
#[test]
fn a_readonly_old_backup_cannot_fail_an_already_committed_new_login() {
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
}
