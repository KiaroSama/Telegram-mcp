fn retained_rename_receipt(state: &State, configuration: Option<&std::path::Path>) -> Value {
    use crate::account_manager::{
        fail_configuration_sync_after_for_test, rename_account, rename_configured_account,
    };
    let fault = fail_configuration_sync_after_for_test(state.root(), 1);
    let moved = match configuration {
        Some(path) => {
            rename_configured_account(state, "work", "office", path, "TELEGRAM_SESSION_NAME_WORK")
        }
        None => rename_account(state, "work", "office"),
    }
    .unwrap();
    assert!(moved.iter().any(|name| name == "work.session.json"));
    drop(fault);
    let receipt = state
        .read_json("account-rename-transaction.json", Value::Null)
        .unwrap();
    assert_eq!(receipt["version"], 2);
    assert_eq!(receipt["phase"], "completed");
    assert!(receipt.get("files").is_none() && receipt.get("rewrites").is_none());
    receipt
}

#[test]
fn acknowledged_state_only_receipt_resurrection_preserves_new_accounts_and_cache_writes() {
    use crate::account_manager::recover_rename;
    use fs2::FileExt;
    let (_root, state) = fixture();
    state
        .write_json("work.session.json", &authorized_account_snapshot())
        .unwrap();
    state.set_ghost_mode(false, Some("work"), None).unwrap();
    let receipt = retained_rename_receipt(&state, None);
    let journal = state.root().join("account-rename-transaction.json");
    fs::remove_file(&journal).unwrap();
    let mut newer = authorized_account_snapshot();
    newer["peer_infos"].as_array_mut().unwrap().push(json!(
        grammers_session::types::PeerInfo::User {
            id: 99,
            auth: None,
            bot: Some(false),
            is_self: Some(false)
        }
    ));
    state.write_json("office.session.json", &newer).unwrap();
    state
        .write_json("work.session.json", &account_snapshot(88, 9))
        .unwrap();
    state.set_ghost_mode(true, Some("office"), None).unwrap();
    let before = ["office.session.json", "work.session.json", "ghost.json"]
        .map(|name| (name, fs::read(state.root().join(name)).unwrap()));
    state
        .write_json("account-rename-transaction.json", &receipt)
        .unwrap();
    let leases = ["work", "office"].map(|label| {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.root().join(format!("{label}.session.lock")))
            .unwrap();
        FileExt::try_lock_exclusive(&file).unwrap();
        file
    });
    recover_rename(&state).unwrap();
    assert!(!journal.exists());
    for (name, bytes) in before {
        assert_eq!(fs::read(state.root().join(name)).unwrap(), bytes);
    }
    drop(leases);
}

#[test]
fn completed_configured_receipt_cleanup_ignores_removed_configuration_and_active_leases() {
    use crate::account_manager::recover_rename;
    use fs2::FileExt;
    let (root, state) = fixture();
    let env = root.path().join("selected.env");
    fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
    state
        .write_json("work.session.json", &authorized_account_snapshot())
        .unwrap();
    let receipt = retained_rename_receipt(&state, Some(&env));
    let journal = state.root().join("account-rename-transaction.json");
    fs::remove_file(&journal).unwrap();
    fs::remove_file(&env).unwrap();
    let newer = account_snapshot(77, 11);
    state.write_json("office.session.json", &newer).unwrap();
    state
        .write_json("account-rename-transaction.json", &receipt)
        .unwrap();
    let lease = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.root().join("office.session.lock"))
        .unwrap();
    FileExt::try_lock_exclusive(&lease).unwrap();
    recover_rename(&state).unwrap();
    assert!(!journal.exists());
    assert!(!env.exists());
    assert_eq!(
        state.read_json("office.session.json", Value::Null).unwrap(),
        newer
    );
}

#[test]
fn ordinary_cache_writes_retire_confirmed_receipts_without_configuration_or_account_leases() {
    use fs2::FileExt;
    for configured in [false, true] {
        let (root, state) = fixture();
        let env = root.path().join("selected.env");
        fs::write(&env, "TELEGRAM_SESSION_NAME_WORK=work\n").unwrap();
        state
            .write_json("work.session.json", &authorized_account_snapshot())
            .unwrap();
        state.set_ghost_mode(false, Some("work"), None).unwrap();
        retained_rename_receipt(&state, configured.then_some(env.as_path()));
        fs::remove_file(&env).unwrap();
        let leases = ["work", "office"].map(|label| {
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(state.root().join(format!("{label}.session.lock")))
                .unwrap();
            FileExt::try_lock_exclusive(&file).unwrap();
            file
        });
        state
            .update_json("office.session.json", Value::Null, |snapshot| {
                snapshot["peer_infos"].as_array_mut().unwrap().push(json!(
                    grammers_session::types::PeerInfo::User {
                        id: 99,
                        auth: None,
                        bot: Some(false),
                        is_self: Some(false)
                    }
                ));
                Ok(())
            })
            .unwrap();
        state
            .write_json("unrelated-cache.json", &json!({"healthy":true}))
            .unwrap();
        assert!(
            !state
                .root()
                .join("account-rename-transaction.json")
                .exists()
        );
        assert!(!env.exists());
        assert!(!state.root().join("work.session.json").exists());
        let cached = state.read_json("office.session.json", Value::Null).unwrap();
        let peers: Vec<grammers_session::types::PeerInfo> =
            serde_json::from_value(cached["peer_infos"].clone()).unwrap();
        assert_eq!(peers.len(), 2);
        assert!(matches!(
            &peers[1],
            grammers_session::types::PeerInfo::User { id: 99, .. }
        ));
        assert_eq!(state.ghost_mode(Some("office"), None)["effective"], false);
        drop(leases);
    }
}

#[test]
fn ordinary_writers_keep_prepared_legacy_and_nonregular_rename_intents_fenced() {
    let (_root, state) = fixture();
    state
        .write_json("work.session.json", &authorized_account_snapshot())
        .unwrap();
    state
        .write_json("sample.json", &json!({"preserve":true}))
        .unwrap();
    let mut prepared = retained_rename_receipt(&state, None);
    prepared["phase"] = json!("prepared");
    prepared["files"] = json!([["work.session.json", "office.session.json"]]);
    prepared["rewrites"] = json!([]);
    let journal = state.root().join("account-rename-transaction.json");
    let before = fs::read(state.root().join("sample.json")).unwrap();
    for marker in [prepared, json!({"files":[],"rewrites":[]})] {
        let bytes = serde_json::to_vec(&marker).unwrap();
        fs::write(&journal, &bytes).unwrap();
        assert!(
            state
                .write_json("sample.json", &json!({"bad":true}))
                .is_err()
        );
        assert_eq!(fs::read(&journal).unwrap(), bytes);
        assert_eq!(fs::read(state.root().join("sample.json")).unwrap(), before);
        assert!(state.root().join("office.session.json").exists());
        assert!(!state.root().join("work.session.json").exists());
    }
    fs::remove_file(&journal).unwrap();
    fs::create_dir(&journal).unwrap();
    assert!(
        state
            .write_json("sample.json", &json!({"bad":true}))
            .is_err()
    );
    assert!(journal.is_dir());
    assert_eq!(fs::read(state.root().join("sample.json")).unwrap(), before);
    #[cfg(unix)]
    {
        fs::remove_dir(&journal).unwrap();
        let outside = state.root().parent().unwrap().join("outside-receipt.json");
        fs::write(&outside, b"private-outside-canary").unwrap();
        std::os::unix::fs::symlink(&outside, &journal).unwrap();
        assert!(
            state
                .write_json("sample.json", &json!({"bad":true}))
                .is_err()
        );
        assert!(journal.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(outside).unwrap(), b"private-outside-canary");
        assert_eq!(fs::read(state.root().join("sample.json")).unwrap(), before);
    }
}

#[test]
fn copied_or_malformed_completed_receipts_fail_closed_without_rewriting_records() {
    use crate::account_manager::recover_rename;
    let (_root, state) = fixture();
    state
        .write_json("work.session.json", &authorized_account_snapshot())
        .unwrap();
    let valid = retained_rename_receipt(&state, None);
    let journal = state.root().join("account-rename-transaction.json");
    let session = fs::read(state.root().join("office.session.json")).unwrap();
    let mut variants = Vec::new();
    for (field, value) in [
        ("version", json!(3)),
        ("version", json!("2")),
        ("phase", json!("future")),
        ("phase", Value::Null),
        ("owner", json!("0".repeat(64))),
        ("from", json!("WORK")),
        ("to", json!("work")),
        ("from", json!("../work")),
    ] {
        let mut marker = valid.clone();
        marker[field] = value;
        variants.push(marker);
    }
    for field in ["version", "from", "phase", "owner"] {
        let mut marker = valid.clone();
        marker.as_object_mut().unwrap().remove(field);
        variants.push(marker);
    }
    let mut marker = valid.clone();
    marker["unexpected"] = json!(true);
    variants.push(marker);
    // A malformed new marker must not masquerade as an old rollback journal.
    let mut marker = valid.clone();
    marker.as_object_mut().unwrap().remove("version");
    marker["files"] = json!([["work.session.json", "office.session.json"]]);
    marker["rewrites"] = json!([]);
    variants.push(marker);
    for marker in variants {
        let bytes = serde_json::to_vec(&marker).unwrap();
        fs::write(&journal, &bytes).unwrap();
        assert!(recover_rename(&state).is_err());
        assert!(
            state
                .write_json("unrelated-cache.json", &json!({"must":"wait"}))
                .is_err()
        );
        assert!(!state.root().join("unrelated-cache.json").exists());
        assert_eq!(fs::read(&journal).unwrap(), bytes);
        assert_eq!(
            fs::read(state.root().join("office.session.json")).unwrap(),
            session
        );
        assert!(!state.root().join("work.session.json").exists());
    }
    let (_other_root, other) = fixture();
    other
        .write_json("office.session.json", &account_snapshot(66, 13))
        .unwrap();
    other
        .write_json("account-rename-transaction.json", &valid)
        .unwrap();
    let before = fs::read(other.root().join("office.session.json")).unwrap();
    assert!(recover_rename(&other).is_err());
    assert!(
        other
            .write_json("unrelated-cache.json", &json!({"must":"wait"}))
            .is_err()
    );
    assert!(!other.root().join("unrelated-cache.json").exists());
    assert_eq!(
        fs::read(other.root().join("office.session.json")).unwrap(),
        before
    );
    assert!(
        other
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
}

#[test]
fn legacy_unversioned_recovery_without_labels_remains_rollback_compatible() {
    use crate::account_manager::recover_rename;
    let (_root, state) = fixture();
    let native = authorized_account_snapshot();
    state.write_json("office.session.json", &native).unwrap();
    let before = json!({"default":true,"accounts":{"work":false},"chats":{}});
    let after = json!({"default":true,"accounts":{"office":false},"chats":{}});
    state.write_json("ghost.json", &after).unwrap();
    state.write_json("account-rename-transaction.json", &json!({"files":[["work.session.json","office.session.json"]],"rewrites":[{"name":"ghost.json","before":before,"after":after}]})).unwrap();
    recover_rename(&state).unwrap();
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap(),
        native
    );
    assert_eq!(state.read_json("ghost.json", Value::Null).unwrap(), before);
    assert!(!state.root().join("office.session.json").exists());
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn uncertain_completion_publication_retains_fenced_receipt_until_a_real_retry_confirms_it() {
    use crate::account_manager::{fail_configuration_sync_for_test, recover_rename};
    let (_root, state) = fixture();
    state
        .write_json("work.session.json", &authorized_account_snapshot())
        .unwrap();
    let mut prepared = retained_rename_receipt(&state, None);
    prepared["phase"] = json!("prepared");
    prepared["files"] = json!([["work.session.json", "office.session.json"]]);
    prepared["rewrites"] = json!([]);
    fs::write(
        state.root().join("account-rename-transaction.json"),
        serde_json::to_vec(&prepared).unwrap(),
    )
    .unwrap();
    state.fail_next_directory_sync_for_test();
    assert!(recover_rename(&state).is_err());
    let receipt = state
        .read_json("account-rename-transaction.json", Value::Null)
        .unwrap();
    assert_eq!(receipt["phase"], "completed");
    assert!(state.root().join("work.session.json").exists());
    assert!(!state.root().join("office.session.json").exists());
    let fault = fail_configuration_sync_for_test(state.root());
    assert!(
        state
            .write_json("unrelated.json", &json!({"must":"wait"}))
            .is_err()
    );
    assert_eq!(
        state
            .read_json("account-rename-transaction.json", Value::Null)
            .unwrap(),
        receipt
    );
    drop(fault);
    state
        .write_json("unrelated.json", &json!({"resumed":true}))
        .unwrap();
    assert!(
        !state
            .root()
            .join("account-rename-transaction.json")
            .exists()
    );
    assert_eq!(
        state.read_json("unrelated.json", Value::Null).unwrap(),
        json!({"resumed":true})
    );
}

#[test]
fn pending_rename_fences_normal_writes_until_trusted_locked_recovery() {
    let (_root, state) = fixture();
    state
        .write_json("sample.json", &json!({"before":true}))
        .unwrap();
    state
        .write_json("account-rename-transaction.json", &json!({"pending":true}))
        .unwrap();
    let path = state.root().join("sample.json");
    let before = fs::read(&path).unwrap();
    assert!(
        state
            .write_json("sample.json", &json!({"unrelated":true}))
            .is_err()
    );
    assert!(
        state
            .update_json("sample.json", Value::Null, |value| {
                value["changed"] = json!(true);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    {
        let _lock = state.state_lock().unwrap();
        state
            .write_json_transaction_unlocked("sample.json", &json!({"trusted_recovery":true}))
            .unwrap();
    }
    assert_eq!(
        state.read_json("sample.json", Value::Null).unwrap(),
        json!({"trusted_recovery":true})
    );
}
