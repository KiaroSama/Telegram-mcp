use super::*;

#[tokio::test]
async fn rejected_login_input_retries_only_up_to_three_attempts() {
    let mut attempts = 0;
    let result = retry_auth(
        async || {
            attempts += 1;
            Ok((attempts == 2).then_some(7))
        },
        "exhausted",
    )
    .await
    .unwrap();
    assert_eq!((result, attempts), (7, 2));

    let mut attempts = 0;
    let failure = retry_auth::<()>(
        async || {
            attempts += 1;
            Ok(None)
        },
        "exhausted",
    )
    .await
    .unwrap_err();
    assert_eq!(failure.to_string(), "exhausted");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn failed_login_transport_is_not_retried_as_rejected_input() {
    let mut attempts = 0;
    let failure = retry_auth::<()>(
        async || {
            attempts += 1;
            bail!("transport failure");
        },
        "exhausted",
    )
    .await
    .unwrap_err();
    assert_eq!(failure.to_string(), "transport failure");
    assert_eq!(attempts, 1);
}

#[cfg(unix)]
#[tokio::test]
async fn disconnected_native_refuses_recovery_after_an_indeterminate_session_commit() {
    for indeterminate in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let files = temporary.path().join("files");
        std::fs::create_dir(&files).unwrap();
        let state = Arc::new(
            State::at_with_installation(
                temporary.path().join("state"),
                vec![files],
                temporary.path().to_owned(),
            )
            .unwrap(),
        );
        let session = Arc::new(PersistentSession::open(Arc::clone(&state), "closed").unwrap());
        session.set_home_dc_id(2).await.unwrap();
        if indeterminate {
            state.fail_next_directory_sync_for_test();
            assert!(session.set_home_dc_id(3).await.is_err());
            assert!(session.check_health().is_err());
        }
        let snapshot = crate::account_manager::ConfigSnapshot {
            values: Default::default(),
            digest: String::new(),
            path: temporary.path().join("configuration.env"),
            process_keys: Default::default(),
        };
        let route = crate::proxy::route(Arc::clone(&state), &snapshot, "closed")
            .await
            .unwrap();
        let SenderPool {
            runner,
            handle,
            updates,
        } = SenderPool::with_configuration(Arc::clone(&session), 1, ConnectionParams::default());
        // Drop the real runner before any network dispatch: this fixture is a
        // disconnected native owner with no live Telegram authorization.
        drop(runner);
        drop(updates);
        let native = Native(Arc::new(Inner {
            client: Client::new(handle.clone()),
            handle,
            session: Arc::clone(&session),
            state,
            label: "closed".into(),
            stats_dc: AtomicI32::new(0),
            tasks: std::sync::Mutex::new(Vec::new()),
            source: None,
            nonce: 1,
            authorized_dcs: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            route,
            secret: None,
            closed: Arc::new(AtomicBool::new(true)),
        }));
        let result = native.close_connection().await;
        assert_eq!(
            result.is_err(),
            indeterminate,
            "Recovery must distinguish a disconnected transport from uncertain session durability"
        );
        if indeterminate {
            assert!(session.check_health().is_err());
            assert!(session.set_home_dc_id(4).await.is_err());
        }
    }
}
