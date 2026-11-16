use super::*;
use std::{fs, panic::AssertUnwindSafe};

fn fixture() -> (tempfile::TempDir, Arc<State>) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let state = State::at(root.path().join("state"), vec![files]).unwrap();
    (root, Arc::new(state))
}

fn authorized_account_snapshot() -> Value {
    let mut data = grammers_session::SessionData::default();
    data.dc_options.get_mut(&data.home_dc).unwrap().auth_key = Some([7; 256]);
    json!({"home_dc":data.home_dc,"dc_options":data.dc_options.values().collect::<Vec<_>>(),"peer_infos":[grammers_session::types::PeerInfo::User{id:42,auth:None,bot:Some(false),is_self:Some(true)}],"updates_state":data.updates_state})
}

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
    rename_configured_account(&state, "work", "office", &env, "TELEGRAM_SESSION_NAME_WORK")
        .unwrap();
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
    let removed = remove_account(&state, "work", &env, "TELEGRAM_SESSION_NAME_WORK").unwrap();
    let archived = format!("{}.session.json", removed["archived"].as_str().unwrap());
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
    assert_eq!(login.commit(42).unwrap()["saved"], true);
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
fn malformed_alias_account_buckets_return_errors_without_panicking_or_replacing_bytes() {
    let (_root, state) = fixture();
    for bucket in [
        json!("invalid"),
        json!([]),
        json!(false),
        json!(7),
        Value::Null,
    ] {
        state
            .write_json(
                "aliases-v2.json",
                &json!({"accounts":{"work":bucket},"owner_note":"preserve"}),
            )
            .unwrap();
        let path = state.root().join("aliases-v2.json");
        let before = fs::read(&path).unwrap();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            state.alias_set(
                "work",
                "close friend",
                json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"}),
                false,
            )
        }));
        assert!(result.is_ok(), "A malformed alias account bucket panicked");
        assert!(result.unwrap().is_err());
        assert!(state.alias_remove("work", "close friend").is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn malformed_approval_store_mutations_return_errors_and_preserve_original_bytes() {
    let (_root, state) = fixture();
    for value in [
        json!([]),
        json!("invalid"),
        json!(false),
        json!(7),
        Value::Null,
    ] {
        state.write_json("always-approvals.json", &value).unwrap();
        let path = state.root().join("always-approvals.json");
        let before = fs::read(&path).unwrap();
        for operation in 0..3 {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| match operation {
                0 => state
                    .grant_approval("work", "send_message", &json!(42))
                    .map(|_| ()),
                1 => state
                    .revoke_approval("work", "send_message", &json!(42))
                    .map(|_| ()),
                _ => state
                    .revoke_folder(state.file_roots()[0].to_str().unwrap())
                    .map(|_| ()),
            }));
            assert!(result.is_ok(), "Malformed approval storage panicked");
            assert!(result.unwrap().is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        assert!(!state.approval_is_granted("work", "send_message", &json!(42)));
    }
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

fn incoming(chat: &str) -> IncomingEvent {
    IncomingEvent {
        account: "work".into(),
        chat_id: chat.into(),
        message_id: "7".into(),
        name: format!("Contact {chat}"),
        username: None,
        bot: false,
    }
}

#[tokio::test]
async fn already_settled_feed_backlog_drains_without_a_second_quiet_period() {
    let (_root, state) = fixture();
    for chat in 1..=32 {
        state.record_incoming(incoming(&chat.to_string())).unwrap();
    }
    {
        let mut journal = state.events.lock().unwrap();
        let old = Instant::now() - Duration::from_secs(60);
        for burst in journal.bursts.values_mut() {
            burst.first = old;
            burst.last = old;
        }
        let flushed = journal
            .flush_settled(Duration::from_secs(50), |value| {
                state.append_event_line(value)
            })
            .unwrap();
        assert!(
            flushed > 0 && flushed < 32,
            "One batch must leave other work a chance to run"
        );
        assert_eq!(journal.bursts.len(), 32 - flushed);
    }
    state.enable_incoming_feed(50_000).unwrap();
    let drained = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state.incoming_feed_status().unwrap()["pending_chats"] == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    state.disable_incoming_feed().unwrap();
    assert!(
        drained.is_ok(),
        "Already-settled bursts were delayed by another quiet period"
    );
    let contents = fs::read_to_string(state.root().join("incoming_feed.jsonl")).unwrap();
    assert_eq!(contents.lines().count(), 32);
}

#[tokio::test]
async fn explicit_disable_wins_over_an_already_claimed_autostart() {
    let (_root, state) = fixture();
    state
        .feed_autostart
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let generation = state.journal_incoming(incoming("42")).unwrap().unwrap();
    state.disable_incoming_feed().unwrap();
    let status = state
        .enable_incoming_feed_at_generation(6000, Some(generation))
        .unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["autostart_pending"], false);
    assert_eq!(status["pending_chats"], 1);
    assert!(!state.root().join("incoming_feed.jsonl").exists());
}

#[tokio::test]
async fn stale_autostart_cannot_override_a_later_manual_settle_period() {
    let (_root, state) = fixture();
    state
        .feed_autostart
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let generation = state.journal_incoming(incoming("42")).unwrap().unwrap();
    state.enable_incoming_feed(50000).unwrap();
    let status = state
        .enable_incoming_feed_at_generation(6000, Some(generation))
        .unwrap();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["settle_ms"], 50000);
    state.disable_incoming_feed().unwrap();
}

#[tokio::test]
async fn restricted_pending_counts_and_consumption_only_include_permitted_chats() {
    let (_root, state) = fixture();
    state.record_incoming(incoming("41")).unwrap();
    state.record_incoming(incoming("42")).unwrap();
    {
        let mut journal = state.events.lock().unwrap();
        journal.note_drop(&("secret-account".into(), "99".into()), "overflow");
    }
    let pending = state
        .wait_incoming_permitted(
            Some("work"),
            None,
            Duration::from_millis(10),
            None,
            1,
            |event| event.chat_id == "42",
        )
        .await
        .unwrap();
    assert_eq!(pending["total"], 1);
    assert_eq!(pending["has_more"], false);
    assert!(pending.get("dropped_total").is_none());
    assert_eq!(pending["pending_chats"][0]["chat_id"], "42");
    let allowed = state
        .wait_incoming_permitted(
            Some("work"),
            None,
            Duration::from_millis(10),
            Some(Duration::ZERO),
            1,
            |event| event.chat_id == "42",
        )
        .await
        .unwrap();
    assert_eq!(allowed["chat_id"], "42");
    let denied = state
        .wait_incoming_permitted(
            Some("work"),
            Some("41"),
            Duration::from_millis(1),
            Some(Duration::ZERO),
            1,
            |_| false,
        )
        .await
        .unwrap();
    assert_eq!(denied["event"], false);
    let retained = state
        .wait_incoming(
            Some("work"),
            Some("41"),
            Duration::from_millis(10),
            Some(Duration::ZERO),
            1,
        )
        .await
        .unwrap();
    assert_eq!(retained["chat_id"], "41");
}

#[test]
fn partial_first_feed_record_is_recovered_before_a_healthy_append() {
    let (_root, state) = fixture();
    let path = state.root().join("incoming_feed.jsonl");
    fs::write(&path, b"{\"account\":\"interrupted").unwrap();
    state
        .append_event_line(&json!({"account":"work","message_count":1}))
        .unwrap();
    let rows = fs::read_to_string(path).unwrap();
    let row: Value = serde_json::from_str(rows.trim_end()).unwrap();
    assert_eq!(row["account"], "work");
    assert!(rows.ends_with('\n'));
}

#[test]
fn partial_last_feed_record_is_removed_without_losing_committed_records() {
    let (_root, state) = fixture();
    state
        .append_event_line(&json!({"account":"first"}))
        .unwrap();
    let path = state.root().join("incoming_feed.jsonl");
    let prefix = fs::read(&path).unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"account\":")
        .unwrap();
    state
        .append_event_line(&json!({"account":"second"}))
        .unwrap();
    let result = fs::read(&path).unwrap();
    assert!(result.starts_with(&prefix));
    let rows = std::str::from_utf8(&result)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["account"], "second");
}

#[test]
fn a_complete_unterminated_feed_record_is_preserved_with_a_separator() {
    let (_root, state) = fixture();
    let path = state.root().join("incoming_feed.jsonl");
    fs::write(&path, br#"{"account":"existing","ts":4102444800}"#).unwrap();
    state.append_event_line(&json!({"account":"next"})).unwrap();
    let result = fs::read_to_string(path).unwrap();
    let rows = result
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["account"], "existing");
    assert_eq!(rows[1]["account"], "next");
}

#[test]
fn committed_corruption_and_an_oversized_partial_feed_are_not_silently_rewritten() {
    let (_root, state) = fixture();
    let path = state.root().join("incoming_feed.jsonl");
    for original in [b"broken committed record\n".to_vec(), vec![b'x'; 65538]] {
        fs::write(&path, &original).unwrap();
        assert!(state.append_event_line(&json!({"account":"next"})).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn failed_feed_prefix_and_final_sync_roll_back_to_the_last_committed_length() {
    use std::io::Write;
    let (_root, state) = fixture();
    state
        .append_event_line(&json!({"account":"committed"}))
        .unwrap();
    let path = state.root().join("incoming_feed.jsonl");
    let before = fs::read(&path).unwrap();
    for fragment in [
        b"{\"account\":".as_slice(),
        b"{\"account\":\"uncommitted\"}\n".as_slice(),
    ] {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let result = super::super::storage::append_transaction(&mut file, |file| {
            file.write_all(fragment)?;
            Err(std::io::Error::other(
                "simulated storage exhaustion or sync failure",
            ))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    state
        .append_event_line(&json!({"account":"healthy-retry"}))
        .unwrap();
    let rows = fs::read_to_string(path).unwrap();
    assert_eq!(rows.lines().count(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(rows.lines().last().unwrap()).unwrap()["account"],
        "healthy-retry"
    );
}

#[tokio::test]
async fn a_complete_append_with_failed_rollback_reports_an_unknown_outcome() {
    use std::io::Write;
    let (_root, state) = fixture();
    let path = state.root().join("incoming_feed.jsonl");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    state.record_incoming(incoming("42")).unwrap();
    {
        let mut journal = state.events.lock().unwrap();
        journal.feed_enabled = true;
        let error = journal
            .flush_settled(Duration::ZERO, |summary| {
                super::super::storage::append_transaction(&mut file, |file| {
                    file.write_all(&serde_json::to_vec(summary)?)?;
                    file.write_all(b"\n")?;
                    // Rollback fails deterministically even for root after a
                    // complete record was written, by changing descriptor access.
                    *file = fs::File::open(&path)?;
                    Err(std::io::Error::other(
                        "injected append synchronization failure",
                    ))
                })
            })
            .unwrap_err();
        assert!(error.is::<super::super::storage::AppendUncertain>());
        assert!(!journal.feed_enabled);
        assert_eq!(journal.bursts.len(), 1);
        assert!(
            journal
                .error
                .as_deref()
                .unwrap()
                .contains("uncertain append")
        );
    }
    let before = fs::read(&path).unwrap();
    tokio::time::timeout(Duration::from_secs(1), state.feed_loop(0, Duration::ZERO))
        .await
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(String::from_utf8(before).unwrap().lines().count(), 1);
}

#[test]
fn feed_keeps_confirmed_rollback_retryable_and_consumes_only_a_durable_success() {
    use std::io::Write;
    let (_root, state) = fixture();
    state.record_incoming(incoming("42")).unwrap();
    let path = state.root().join("incoming_feed.jsonl");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let mut journal = state.events.lock().unwrap();
    journal.feed_enabled = true;
    let error = journal
        .flush_settled(Duration::ZERO, |_| {
            super::super::storage::append_transaction(&mut file, |file| {
                file.write_all(b"{\"partial\":")?;
                Err(std::io::Error::other("injected append failure"))
            })
        })
        .unwrap_err();
    assert!(!error.is::<super::super::storage::AppendUncertain>());
    assert!(journal.feed_enabled);
    assert_eq!(journal.bursts.len(), 1);
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    drop(file);
    assert_eq!(
        journal
            .flush_settled(Duration::ZERO, |value| state.append_event_line(value))
            .unwrap(),
        1
    );
    assert!(journal.bursts.is_empty());
    assert!(journal.error.is_none());
    assert_eq!(fs::read_to_string(path).unwrap().lines().count(), 1);
}

#[test]
fn feed_maintenance_recovers_an_interrupted_tail_after_restarting_state() {
    let (_root, state) = fixture();
    let path = state.root().join("incoming_feed.jsonl");
    state
        .append_event_line(&json!({"account":"committed"}))
        .unwrap();
    let before = fs::read(&path).unwrap();
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"interrupted\":")
        .unwrap();
    let restarted = State::at(state.root().to_path_buf(), state.file_roots().to_vec()).unwrap();
    restarted.maintain_event_feed().unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    restarted
        .append_event_line(&json!({"account":"after-restart"}))
        .unwrap();
    let rows = fs::read_to_string(path).unwrap();
    assert_eq!(rows.lines().count(), 2);
}

#[test]
fn malformed_nested_ghost_maps_refuse_mutations_without_panics_or_replacement() {
    let (_root, state) = fixture();
    for value in [
        json!({"accounts":[]}),
        json!({"accounts":"invalid"}),
        json!({"chats":[]}),
        json!({"chats":{"work":false}}),
    ] {
        state.write_json("ghost.json", &value).unwrap();
        let path = state.root().join("ghost.json");
        let before = fs::read(&path).unwrap();
        for scope in [1, 2, 0] {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| match scope {
                0 => state.set_ghost_mode(false, None, None),
                1 => state.set_ghost_mode(false, Some("work"), None),
                _ => state.set_ghost_mode(false, Some("work"), Some(&json!(42))),
            }));
            assert!(result.is_ok(), "Malformed nested ghost settings panicked");
            assert!(result.unwrap().is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }
}

#[test]
fn state_json_byte_budget_includes_its_newline_and_preserves_old_bytes_on_refusal() {
    let (_root, state) = fixture();
    let limit = 16 * 1024 * 1024;
    state
        .write_json("boundary.json", &json!({"before":true}))
        .unwrap();
    let path = state.root().join("boundary.json");
    let before = fs::read(&path).unwrap();
    let rejected = Value::String("x".repeat(limit - 2));
    assert_eq!(serde_json::to_vec_pretty(&rejected).unwrap().len(), limit);
    assert!(state.write_json("boundary.json", &rejected).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let accepted = Value::String("x".repeat(limit - 3));
    state.write_json("boundary.json", &accepted).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), limit as u64);
    assert_eq!(
        state.read_json("boundary.json", Value::Null).unwrap(),
        accepted
    );
}

#[test]
fn state_writer_waits_for_a_real_contended_lock_then_commits() {
    let (_root, state) = fixture();
    let held = state.state_lock().unwrap();
    let writer = Arc::clone(&state);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let probe = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(writer.root().join("state.lock"))
            .unwrap();
        let contention = fs2::FileExt::try_lock_exclusive(&probe).unwrap_err();
        assert_eq!(
            contention.raw_os_error(),
            fs2::lock_contended_error().raw_os_error()
        );
        ready_tx.send(()).unwrap();
        done_tx
            .send(writer.write_json("after-contention.json", &json!({"committed":true})))
            .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        done_rx.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    drop(held);
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    thread.join().unwrap();
    assert_eq!(
        state
            .read_json("after-contention.json", Value::Null)
            .unwrap()["committed"],
        true
    );
}

#[test]
fn a_post_replace_sync_failure_is_distinct_from_an_unchanged_precommit_failure() {
    use super::super::storage::{DurabilityUncertain, replace_and_sync};
    use anyhow::Context;
    let (root, _state) = fixture();
    let destination = root.path().join("committed.json");
    let temporary = root.path().join("pending.json");
    fs::write(&destination, b"previous").unwrap();
    fs::write(&temporary, b"replacement").unwrap();
    let error = replace_and_sync(&temporary, &destination, || {
        Err(std::io::Error::other("simulated directory sync failure"))
    })
    .context("session persistence failed")
    .unwrap_err();
    assert!(error.downcast_ref::<DurabilityUncertain>().is_some());
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!temporary.exists());
    let before = fs::read(&destination).unwrap();
    let error = replace_and_sync(&temporary, &destination, || Ok(())).unwrap_err();
    assert!(error.downcast_ref::<DurabilityUncertain>().is_none());
    assert_eq!(fs::read(destination).unwrap(), before);
}
