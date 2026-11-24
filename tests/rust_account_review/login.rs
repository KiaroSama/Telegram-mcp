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

#[cfg(windows)]
#[test]
fn a_readonly_old_backup_cannot_fail_an_already_committed_new_login() {
    use std::os::windows::fs::OpenOptionsExt;
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Ok(metadata) = std::fs::metadata(&self.0) {
                let mut permissions = metadata.permissions();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    permissions.set_mode(permissions.mode() | 0o200);
                }
                #[cfg(not(unix))]
                #[expect(
                    clippy::permissions_set_readonly_false,
                    reason = "Windows test fixture cleanup clears read-only attribute"
                )]
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
