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
