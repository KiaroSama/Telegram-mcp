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
