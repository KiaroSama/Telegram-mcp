#[tokio::test]
async fn recovered_difference_batch_never_checkpoints_a_partially_delivered_shared_pts() {
    let (_temp, state) = fixture();
    let session = Arc::new(PersistentSession::open(Arc::clone(&state), "batch").unwrap());
    session
        .set_update_state(UpdateState::All(current()))
        .await
        .unwrap();
    let mut second = raw_message();
    second["id"] = json!(8);
    let bytes=Schema::global().encode_object(&json!({"_":"updates.difference",
        "new_messages":[raw_message(),second],"new_encrypted_messages":[],"other_updates":[],"chats":[],"users":[],"topics":null,
        "state":{"_":"updates.state","pts":12,"qts":71,"date":200,"seq":6,"unread_count":0}})).unwrap();
    let difference = tl::enums::updates::Difference::from_bytes(&bytes).unwrap();
    let mut boxes = grammers_session::updates::MessageBoxes::load(current());
    let (batch, _, _) = boxes.apply_difference(difference);
    assert_eq!(batch.len(), 2);
    assert!(
        batch
            .iter()
            .all(|(_, state)| state.message_box == Some(MessageBox::Common { pts: 12 }))
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let task_session = Arc::clone(&session);
    let task_state = Arc::clone(&state);
    let pending = Arc::clone(&entered);
    let task_batch = batch.clone();
    let task = tokio::spawn(async move {
        let mut index = 0;
        process_batch(&task_session, task_batch, |update| {
            index += 1;
            let second = index == 2;
            let state = Arc::clone(&task_state);
            let pending = Arc::clone(&pending);
            async move {
                if second {
                    pending.notify_one();
                    std::future::pending::<()>().await;
                }
                let tl::enums::Update::NewMessage(message) = update else {
                    panic!("Expected recovered message")
                };
                publish(
                    &state,
                    &AccessPolicy::default(),
                    "batch",
                    &message_fields(&message.message).unwrap(),
                    &peer(),
                )?;
                Ok(true)
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(events(&state).await["events"].as_array().unwrap().len(), 1);
    assert_eq!(session.updates_state().await.unwrap(), current());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // Restart/replay can still deliver the complete shared-pts batch.
    process_batch(&session, batch, |update| {
        let state = Arc::clone(&state);
        async move {
            let tl::enums::Update::NewMessage(message) = update else {
                panic!("Expected recovered message")
            };
            publish(
                &state,
                &AccessPolicy::default(),
                "batch",
                &message_fields(&message.message).unwrap(),
                &peer(),
            )?;
            Ok(true)
        }
    })
    .await
    .unwrap();
    let final_state = session.updates_state().await.unwrap();
    assert_eq!(final_state.pts, 12);
    assert_eq!(final_state.qts, 71);
    assert_eq!(final_state.seq, current().seq);
    assert_eq!(final_state.date, current().date);
    let recorded = events(&state).await;
    assert!(
        recorded["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["message_id"] == 8)
    );
}

#[tokio::test(start_paused = true)]
async fn update_read_retry_policy_uses_typed_errors_bounded_backoff_and_server_flood_waits() {
    let io = InvocationError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
    assert_eq!(retry_delay(&io, 0), Some(Duration::from_millis(250)));
    assert_eq!(retry_delay(&io, 1), Some(Duration::from_millis(500)));
    assert_eq!(retry_delay(&io, u32::MAX), Some(Duration::from_secs(30)));
    let rpc = |code, name: &str, value| {
        InvocationError::Rpc(grammers_mtsender::RpcError {
            code,
            name: String::from(name),
            value,
            caused_by: None,
        })
    };
    assert_eq!(
        retry_delay(&rpc(500, "RPC_CALL_FAIL", None), 0),
        Some(Duration::from_millis(250))
    );
    assert_eq!(
        retry_delay(&rpc(420, "FLOOD_WAIT", Some(60)), 0),
        Some(Duration::from_secs(60))
    );
    assert_eq!(retry_delay(&rpc(420, "FLOOD_WAIT", Some(86_401)), 0), None);
    for error in [
        rpc(401, "AUTH_KEY_UNREGISTERED", None),
        rpc(400, "BAD_REQUEST", None),
        InvocationError::Dropped("updates closed"),
        InvocationError::InvalidDc,
    ] {
        assert_eq!(retry_delay(&error, 0), None);
    }
    struct Reader {
        results: std::collections::VecDeque<std::result::Result<Batch, InvocationError>>,
        calls: usize,
    }
    impl BatchReader for Reader {
        async fn read(&mut self) -> std::result::Result<Batch, InvocationError> {
            self.calls += 1;
            self.results.pop_front().expect("Unexpected update read")
        }
    }
    let (_temp, state) = fixture();
    let session = PersistentSession::open(state, "retry").unwrap();
    let mut reader = Reader {
        results: vec![
            Err(io),
            Err(rpc(500, "RPC_CALL_FAIL", None)),
            Ok((vec![], vec![], vec![])),
            Err(InvocationError::Dropped("closed")),
        ]
        .into(),
        calls: 0,
    };
    let start = tokio::time::Instant::now();
    assert!(next_batch(&mut reader, &session).await.is_some());
    assert_eq!(reader.calls, 3);
    assert_eq!(start.elapsed(), Duration::from_millis(750));
    let stopped = tokio::time::Instant::now();
    assert!(next_batch(&mut reader, &session).await.is_none());
    assert_eq!(reader.calls, 4);
    assert_eq!(stopped.elapsed(), Duration::ZERO);
    assert_eq!(session.updates_state().await.unwrap().pts, 0);
}

#[test]
fn primary_cursor_uses_delivered_pts_and_preserves_container_seq_date() {
    let updated =
        cloud_cursor(&current(), &delivered(Some(MessageBox::Common { pts: 11 }))).unwrap();
    assert!(matches!(
        updated,
        UpdateState::Primary {
            pts: 11,
            date: 100,
            seq: 5
        }
    ));
}

#[test]
fn channel_cursor_is_exact_and_does_not_advance_another_channel() {
    let updated = cloud_cursor(
        &current(),
        &delivered(Some(MessageBox::Channel {
            channel_id: 9,
            pts: 31,
        })),
    )
    .unwrap();
    assert!(matches!(updated, UpdateState::Channel { id: 9, pts: 31 }));
    let new = cloud_cursor(
        &current(),
        &delivered(Some(MessageBox::Channel {
            channel_id: 10,
            pts: 1,
        })),
    )
    .unwrap();
    assert!(matches!(new, UpdateState::Channel { id: 10, pts: 1 }));
}

#[test]
fn secondary_and_absent_update_cursors_are_never_checkpointed() {
    assert!(
        cloud_cursor(
            &current(),
            &delivered(Some(MessageBox::Secondary { qts: 999 }))
        )
        .is_none()
    );
    assert!(cloud_cursor(&current(), &delivered(None)).is_none());
}

#[test]
fn zero_stale_or_invalid_cloud_cursors_do_not_regress_session_state() {
    for message_box in [
        MessageBox::Common { pts: 0 },
        MessageBox::Common { pts: 9 },
        MessageBox::Common { pts: 10 },
        MessageBox::Channel {
            channel_id: 9,
            pts: 29,
        },
        MessageBox::Channel {
            channel_id: 9,
            pts: 30,
        },
        MessageBox::Channel {
            channel_id: 10,
            pts: 0,
        },
        MessageBox::Channel {
            channel_id: -1,
            pts: 40,
        },
    ] {
        assert!(cloud_cursor(&current(), &delivered(Some(message_box))).is_none());
    }
}

#[tokio::test]
async fn durable_cloud_checkpoints_leave_secondary_custody_and_other_channels_untouched() {
    let (_temp, state) = fixture();
    let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    session
        .set_update_state(UpdateState::All(current()))
        .await
        .unwrap();
    checkpoint(&session, &delivered(Some(MessageBox::Common { pts: 11 })))
        .await
        .unwrap();
    checkpoint(
        &session,
        &delivered(Some(MessageBox::Channel {
            channel_id: 10,
            pts: 4,
        })),
    )
    .await
    .unwrap();
    checkpoint(
        &session,
        &delivered(Some(MessageBox::Secondary { qts: 999 })),
    )
    .await
    .unwrap();
    drop(session);
    let reopened = PersistentSession::open(state, "work").unwrap();
    let stored = reopened.updates_state().await.unwrap();
    assert_eq!(
        (stored.pts, stored.qts, stored.date, stored.seq),
        (11, 71, 100, 5)
    );
    assert!(stored.channels.contains(&ChannelState { id: 9, pts: 30 }));
    assert!(stored.channels.contains(&ChannelState { id: 10, pts: 4 }));
}

#[tokio::test]
async fn native_callback_loop_records_then_checkpoints_without_running_a_network_sender() {
    let (_temp, state) = fixture();
    let session = Arc::new(PersistentSession::open(Arc::clone(&state), "callback").unwrap());
    let pool = grammers_mtsender::SenderPool::new(Arc::clone(&session), 1);
    let handle = pool.handle.clone();
    let client = Client::new(pool.handle);
    let (send, receive) = tokio::sync::mpsc::channel(1);
    let mut source = raw_message();
    source["reply_to"] = Value::Null;
    send.send(packet(source, json!([]), json!([]), false))
        .await
        .unwrap();
    drop(send);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run(
            client,
            handle,
            receive,
            Arc::clone(&state),
            Arc::clone(&session),
            "callback".into(),
        ),
    )
    .await
    .unwrap();
    let recorded = events(&state).await;
    assert_eq!(recorded["events"].as_array().unwrap().len(), 1);
    assert_eq!(recorded["events"][0]["message_id"], 7);
    assert_eq!(session.updates_state().await.unwrap().pts, 1);
    assert_eq!(state.incoming_feed_status().unwrap()["pending_chats"], 1);
}

#[tokio::test]
async fn callback_validation_failure_stops_before_checkpointing_the_event() {
    let (_temp, state) = fixture();
    let session = Arc::new(PersistentSession::open(Arc::clone(&state), "callback").unwrap());
    let pool = grammers_mtsender::SenderPool::new(Arc::clone(&session), 1);
    let handle = pool.handle.clone();
    let client = Client::new(pool.handle);
    let (send, receive) = tokio::sync::mpsc::channel(1);
    send.send(packet(raw_message(), json!([]), json!([]), false))
        .await
        .unwrap();
    drop(send);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run(
            client,
            handle,
            receive,
            Arc::clone(&state),
            Arc::clone(&session),
            "invalid/account".into(),
        ),
    )
    .await
    .unwrap();
    assert!(
        events(&state).await["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(session.updates_state().await.unwrap().pts, 0);
    assert_eq!(state.incoming_feed_status().unwrap()["pending_chats"], 0);
}

#[tokio::test]
async fn checkpoint_storage_failure_preserves_the_old_in_memory_and_on_disk_cursor() {
    let (_temp, state) = fixture();
    let session = PersistentSession::open(Arc::clone(&state), "callback").unwrap();
    let destination = state.root().join("callback.session.json");
    std::fs::create_dir(&destination).unwrap();
    assert!(
        checkpoint(&session, &delivered(Some(MessageBox::Common { pts: 1 })))
            .await
            .is_err()
    );
    assert_eq!(
        session.updates_state().await.unwrap(),
        UpdatesState::default()
    );
    assert!(destination.is_dir());
}
