#[test]
fn unconfirmed_state_move_rolls_back_before_configuration_and_retains_intent() {
    use crate::account_manager::{
        fail_configuration_sync_for_test, recover_rename, rename_configured_account,
    };
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    let original = "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=keep\n";
    fs::write(&env, original).unwrap();
    let native = authorized_account_snapshot();
    state.write_json("work.session.json", &native).unwrap();
    let secret_parent = state.root().join("secret-chats");
    fs::create_dir(&secret_parent).unwrap();
    fs::create_dir(secret_parent.join("work")).unwrap();
    fs::write(
        secret_parent.join("work").join("preserved.json"),
        b"{\"deferred\":true}",
    )
    .unwrap();
    let fault = fail_configuration_sync_for_test(&secret_parent);
    assert!(
        rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
            .is_err()
    );
    let journal = state.root().join("account-rename-transaction.json");
    assert!(journal.exists(), "Unconfirmed rollback lost its intent");
    assert_eq!(fs::read_to_string(&env).unwrap(), original);
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        native
    );
    assert!(secret_parent.join("work").join("preserved.json").exists());
    assert!(!secret_parent.join("office").exists());
    drop(fault);
    recover_rename(&state).unwrap();
    assert!(!journal.exists());
    assert_eq!(fs::read_to_string(env).unwrap(), original);
}

#[test]
fn login_rollback_confirms_state_file_removals_before_discarding_recovery_intent() {
    use crate::account_manager::{config_digest, fail_configuration_sync_for_test, recover_logins};
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    fs::write(&env, "OTHER=keep\n").unwrap();
    let stage = "__login_rollback_fixture";
    let staged = authorized_account_snapshot();
    let mut installed = staged.clone();
    installed["authorization_source"] = json!(format!(
        "TELEGRAM_SESSION_NAME_WORK:{}",
        config_digest("work")
    ));
    state
        .write_json(&format!("{stage}.session.json"), &staged)
        .unwrap();
    state.write_json("work.session.json", &installed).unwrap();
    let name = "account-login-work-transaction.json";
    state.write_json(name,&json!({"account":"work","stage":stage,"stage_digest":config_digest(&serde_json::to_string(&staged).unwrap()),"env_file":env,"config_key":"TELEGRAM_SESSION_NAME_WORK","config_digest":config_digest("work"),"installed":installed})).unwrap();
    let fault = fail_configuration_sync_for_test(state.root());
    assert!(recover_logins(&state).is_err());
    assert!(state.root().join(name).exists());
    assert!(!state.root().join("work.session.json").exists());
    assert!(!state.root().join(format!("{stage}.session.json")).exists());
    assert_eq!(fs::read_to_string(&env).unwrap(), "OTHER=keep\n");
    drop(fault);
    recover_logins(&state).unwrap();
    assert!(!state.root().join(name).exists());
    assert!(!state.root().join("work.session.json").exists());
}

#[test]
fn configured_rename_retains_intent_until_configuration_directory_is_durable() {
    use crate::account_manager::{
        fail_configuration_sync_for_test, recover_rename, rename_configured_account,
    };
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=keep\n").unwrap();
    state
        .write_json("work.session.json", &authorized_account_snapshot())
        .unwrap();
    let secret_parent = state.root().join("secret-chats");
    fs::create_dir(&secret_parent).unwrap();
    fs::create_dir(secret_parent.join("work")).unwrap();
    fs::write(
        secret_parent.join("work").join("preserved.json"),
        b"{\"deferred\":true}",
    )
    .unwrap();
    let fault = fail_configuration_sync_for_test(&env);
    let error =
        rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
            .unwrap_err();
    assert!(error.to_string().contains("recovery pending"), "{error:#}");
    let journal = state.root().join("account-rename-transaction.json");
    assert!(
        journal.exists(),
        "Published rename lost its durability recovery intent"
    );
    assert!(state.root().join("office.session.json").exists());
    assert!(!state.root().join("work.session.json").exists());
    assert_eq!(
        fs::read(secret_parent.join("office").join("preserved.json")).unwrap(),
        b"{\"deferred\":true}"
    );
    let published = fs::read(&env).unwrap();
    assert!(recover_rename(&state).is_err());
    assert_eq!(fs::read(&env).unwrap(), published);
    assert!(journal.exists());
    drop(fault);
    recover_rename(&state).unwrap();
    assert!(!journal.exists());
    assert_eq!(fs::read(&env).unwrap(), published);
    assert!(state.root().join("office.session.json").exists());
    assert!(!secret_parent.join("work").exists());
}

#[test]
fn legacy_removal_rollback_retains_intent_until_its_configuration_is_durable() {
    use crate::account_manager::{config_digest, fail_configuration_sync_for_test, recover_rename};
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    let original = "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=keep\n";
    fs::write(&env, original).unwrap();
    let archived = "__removed_work_fixture";
    let native = authorized_account_snapshot();
    state
        .write_json(&format!("{archived}.session.json"), &native)
        .unwrap();
    let secret_parent = state.root().join("secret-chats");
    fs::create_dir(&secret_parent).unwrap();
    fs::create_dir(secret_parent.join(archived)).unwrap();
    fs::write(
        secret_parent.join(archived).join("preserved.json"),
        b"{\"deferred\":true}",
    )
    .unwrap();
    state.write_json("account-rename-transaction.json",&json!({"from":"work","to":archived,"files":[["work.session.json",format!("{archived}.session.json")],["secret-chats/work",format!("secret-chats/{archived}")]],"rewrites":[],"removal":{"env_file":env,"key":"TELEGRAM_SESSION_NAME_WORK","digest":config_digest("work")}})).unwrap();
    let journal = state.root().join("account-rename-transaction.json");
    let fault = fail_configuration_sync_for_test(&env);
    assert!(recover_rename(&state).is_err());
    assert!(journal.exists());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        native
    );
    assert_eq!(fs::read_to_string(&env).unwrap(), original);
    assert_eq!(
        fs::read(secret_parent.join("work").join("preserved.json")).unwrap(),
        b"{\"deferred\":true}"
    );
    drop(fault);
    recover_rename(&state).unwrap();
    assert!(!journal.exists());
    assert_eq!(fs::read_to_string(env).unwrap(), original);
    assert!(!secret_parent.join(archived).exists());
}

#[test]
fn account_removal_retains_archive_and_intent_until_configuration_is_durable() {
    use crate::account_manager::{
        fail_configuration_sync_for_test, recover_rename, remove_account,
    };
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\nOTHER=keep\n").unwrap();
    let original = authorized_account_snapshot();
    state.write_json("work.session.json", &original).unwrap();
    let fault = fail_configuration_sync_for_test(&env);
    let error = remove_account(&state, "work", &env, "TELEGRAM_SESSION_NAME_WORK").unwrap_err();
    assert!(error.to_string().contains("recovery pending"), "{error:#}");
    let intent = state
        .read_json("account-rename-transaction.json", Value::Null)
        .unwrap();
    let archived = format!("{}.session.json", intent["to"].as_str().unwrap());
    assert_eq!(state.read_json(&archived, Value::Null).unwrap(), original);
    let journal = state.root().join("account-rename-transaction.json");
    assert!(
        journal.exists(),
        "Published removal lost its durability recovery intent"
    );
    assert!(recover_rename(&state).is_err());
    assert!(
        !fs::read_to_string(&env)
            .unwrap()
            .contains("TELEGRAM_SESSION_NAME_WORK")
    );
    assert!(journal.exists());
    drop(fault);
    recover_rename(&state).unwrap();
    assert!(!journal.exists());
    assert_eq!(state.read_json(&archived, Value::Null).unwrap(), original);
    assert!(fs::read_to_string(&env).unwrap().contains("OTHER=keep"));
}

#[test]
fn new_login_retains_published_authorization_until_configuration_is_durable() {
    use crate::account_manager::{begin_login, fail_configuration_sync_for_test, recover_logins};
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    fs::write(&env, "OTHER=keep\n").unwrap();
    let login = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
    state
        .write_json(
            &format!("{}.session.json", login.stage_label()),
            &authorized_account_snapshot(),
        )
        .unwrap();
    let fault = fail_configuration_sync_for_test(&env);
    let error = login.commit(42).unwrap_err();
    assert!(error.to_string().contains("recovery pending"), "{error:#}");
    let journal = state.root().join("account-login-work-transaction.json");
    assert!(
        journal.exists(),
        "Published login lost its durability recovery intent"
    );
    let installed = fs::read(state.root().join("work.session.json")).unwrap();
    assert!(recover_logins(&state).is_err());
    assert_eq!(
        fs::read(state.root().join("work.session.json")).unwrap(),
        installed
    );
    assert!(journal.exists());
    drop(fault);
    recover_logins(&state).unwrap();
    assert!(!journal.exists());
    assert_eq!(
        fs::read(state.root().join("work.session.json")).unwrap(),
        installed
    );
    assert!(fs::read_to_string(env).unwrap().contains("OTHER=keep"));
}

#[test]
fn confirmed_login_retains_its_actual_cleanup_only_receipt_when_stage_cleanup_is_unconfirmed() {
    use crate::account_manager::{begin_login, fail_configuration_sync_for_test, recover_logins};
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    fs::write(&env, "OTHER=keep\n").unwrap();
    let login = begin_login(state.clone(), "work", &env, None, None, None).unwrap();
    let stage = format!("{}.session.json", login.stage_label());
    state
        .write_json(&stage, &authorized_account_snapshot())
        .unwrap();
    // State publication uses the real storage barrier. This account-manager
    // barrier fails only after the durable completion receipt has been written.
    let fault = fail_configuration_sync_for_test(state.root());
    assert_eq!(login.commit(42).unwrap()["saved"], true);
    let name = "account-login-work-transaction.json";
    let receipt = state.read_json(name, Value::Null).unwrap();
    assert_eq!(receipt["version"], 2);
    assert_eq!(receipt["phase"], "completed");
    assert_eq!(receipt["account"], "work");
    assert_eq!(receipt["committed"], true);
    assert_eq!(receipt.as_object().unwrap().len(), 7);
    for field in ["installed", "env_file", "config_key", "config_digest"] {
        assert!(receipt.get(field).is_none(), "obsolete {field} retained");
    }
    assert!(!state.root().join(stage).exists());
    let installed = fs::read(state.root().join("work.session.json")).unwrap();
    let published_config = fs::read(&env).unwrap();
    assert_eq!(
        crate::account_manager::read_config(&env).unwrap().values["TELEGRAM_SESSION_NAME_WORK"],
        "work"
    );
    drop(fault);
    recover_logins(&state).unwrap();
    assert!(!state.root().join(name).exists());
    assert_eq!(
        fs::read(state.root().join("work.session.json")).unwrap(),
        installed
    );
    assert_eq!(fs::read(env).unwrap(), published_config);
}
