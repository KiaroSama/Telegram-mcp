use super::*;
use std::{fs, panic::AssertUnwindSafe};

fn fixture() -> (tempfile::TempDir, Arc<State>) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let state = State::at(root.path().join("state"), vec![files]).unwrap();
    (root, Arc::new(state))
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
