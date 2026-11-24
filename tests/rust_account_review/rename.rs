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
fn rename_moves_native_secret_chat_state_without_rewriting_it() {
    let (_dir, state) = fixture();
    let custody = state.root().join("secret-chats");
    std::fs::create_dir(&custody).unwrap();
    let suffixes = [
        ".v1.enc",
        ".store.key",
        ".migration.json",
        ".history.v1.json",
        ".media.v1.json",
        ".archive.v1.json",
    ];
    for suffix in suffixes {
        std::fs::write(custody.join(format!("work{suffix}")), suffix).unwrap();
    }

    telegram_mcp::account_manager::rename_account(&state, "work", "office").unwrap();

    for suffix in suffixes {
        assert!(!custody.join(format!("work{suffix}")).exists(), "{suffix}");
        assert_eq!(
            std::fs::read(custody.join(format!("office{suffix}"))).unwrap(),
            suffix.as_bytes(),
            "{suffix}"
        );
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
