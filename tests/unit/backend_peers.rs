use super::*;
#[cfg(unix)]
use grammers_session::types::UpdateState;
use grammers_tl_types::Deserializable;

#[test]
fn direct_user_and_get_users_vector_form_lossless_cache_records() {
    let user =
        json!({"_":"user","id":"42","access_hash":"-9223372036854775808","bot":true,"self":true});
    let expected = PeerInfo::User {
        id: 42,
        auth: Some(PeerAuth::from_hash(i64::MIN)),
        bot: Some(true),
        is_self: Some(true),
    };
    assert_eq!(peer_infos(&user), vec![expected.clone()]);
    assert_eq!(
        peer_infos(&json!([user,{"_":"userEmpty","id":43}])),
        vec![
            expected,
            PeerInfo::User {
                id: 43,
                auth: None,
                bot: None,
                is_self: None
            }
        ]
    );
}

#[test]
fn wrapper_peer_arrays_keep_authority_and_channel_classification() {
    let wrapper = json!({"users":[{"_":"user","id":42,"access_hash":"77"}],
        "chats":[{"_":"chatForbidden","id":5},{"_":"channelForbidden","id":9,"access_hash":"88","broadcast":true},
            {"_":"channel","id":10,"access_hash":"99","megagroup":true},{"_":"chatEmpty","id":6},
            {"_":"community","id":11,"access_hash":"101"}]});
    assert_eq!(
        peer_infos(&wrapper),
        vec![
            PeerInfo::User {
                id: 42,
                auth: Some(PeerAuth::from_hash(77)),
                bot: None,
                is_self: None
            },
            PeerInfo::Chat { id: 5 },
            PeerInfo::Channel {
                id: 9,
                auth: Some(PeerAuth::from_hash(88)),
                kind: Some(ChannelKind::Broadcast)
            },
            PeerInfo::Channel {
                id: 10,
                auth: Some(PeerAuth::from_hash(99)),
                kind: Some(ChannelKind::Megagroup)
            },
            PeerInfo::Chat { id: 6 },
            PeerInfo::Channel {
                id: 11,
                auth: Some(PeerAuth::from_hash(101)),
                kind: Some(ChannelKind::Community)
            }
        ]
    );
}

#[test]
fn min_peers_do_not_promote_context_specific_access_hashes() {
    let peers = peer_infos(&json!([
        {"_":"user","id":42,"access_hash":"777","min":true},
        {"_":"channel","id":9,"access_hash":"888","min":true,"megagroup":true},
        {"_":"community","id":10,"access_hash":"999","min":true},
        {"_":"userEmpty","id":43,"access_hash":"123"}]));
    assert_eq!(peers.len(), 4);
    assert!(peers.iter().all(|peer| peer.auth().is_none()));
    let nested = json!({"message":{"_":"user","id":50,"access_hash":"1"},
        "users":[{"_":"message","id":3},{"_":"user","id":true},{"_":"user","id":0},{"_":"user","id":-5},
            {"_":"user","id":"9223372036854775808"}]});
    assert!(peer_infos(&nested).is_empty());
}

#[tokio::test]
async fn min_cache_updates_preserve_existing_full_authority_in_durable_session() {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    let session = PersistentSession::open(Arc::clone(&state), "cache").unwrap();
    session
        .cache_peers(peer_infos(&json!([
        {"_":"user","id":42,"access_hash":"77","self":true},
        {"_":"channel","id":9,"access_hash":"88","megagroup":true}])))
        .await
        .unwrap();
    session
        .cache_peers(peer_infos(
            &json!({"users":[{"_":"user","id":42,"access_hash":"999","min":true}],
        "chats":[{"_":"channel","id":9,"access_hash":"999","min":true}]}),
        ))
        .await
        .unwrap();
    drop(session);
    let session = PersistentSession::open(state, "cache").unwrap();
    let user = session
        .peer(PeerId::user(42).unwrap())
        .await
        .unwrap()
        .unwrap();
    let channel = session
        .peer(PeerId::channel(9).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.auth().unwrap().hash(), 77);
    assert_eq!(channel.auth().unwrap().hash(), 88);
    assert!(matches!(
        user,
        PeerInfo::User {
            is_self: Some(true),
            ..
        }
    ));
    assert!(matches!(
        channel,
        PeerInfo::Channel {
            kind: Some(ChannelKind::Megagroup),
            ..
        }
    ));
}

#[test]
fn cache_formation_matches_official_grammers_typed_peer_conversions() {
    for value in [
        json!({"_":"user","id":"42","access_hash":"-99","bot":true,"bot_info_version":1,"self":true}),
        json!({"_":"user","id":"43","access_hash":"-98","min":true}),
        json!({"_":"userEmpty","id":"44"}),
    ] {
        let bytes = Schema::global().encode_object(&value).unwrap();
        let typed = tl::enums::User::from_bytes(&bytes).unwrap();
        let decoded = Schema::global().decode("User", &bytes).unwrap();
        assert_eq!(peer_infos(&decoded), vec![PeerInfo::from(typed)]);
    }
    for value in [
        json!({"_":"channelForbidden","id":"9","access_hash":"-999","broadcast":true,"title":"Channel"}),
        json!({"_":"channel","id":"10","access_hash":"-998","min":true,"megagroup":true,"title":"Group","date":1,"photo":{"_":"chatPhotoEmpty"}}),
    ] {
        let bytes = Schema::global().encode_object(&value).unwrap();
        let typed = tl::enums::Chat::from_bytes(&bytes).unwrap();
        let decoded = Schema::global().decode("Chat", &bytes).unwrap();
        assert_eq!(peer_infos(&decoded), vec![PeerInfo::from(typed)]);
    }
}

#[tokio::test]
async fn successful_rpc_result_is_preserved_when_peer_storage_cannot_commit() {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    let session = PersistentSession::open(state.clone(), "cache").unwrap();
    let path = state.root().join("cache.session.json");
    if path.exists() {
        std::fs::remove_file(&path).unwrap();
    }
    std::fs::create_dir(&path).unwrap();
    let response = json!({"_":"updates","updates":[{"_":"updateMessageID","id":77,"random_id":"17"}],"users":[{"_":"user","id":"42","access_hash":"77"}],"chats":[],"date":1,"seq":1});
    let actual = cache_rpc_outcome(&session, response.clone()).await.unwrap();
    assert_eq!(
        actual, response,
        "A successful remote operation cannot become a retryable storage failure"
    );
    assert!(
        session
            .peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .is_none(),
        "Failed cache snapshots must not publish in-memory authority"
    );
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(
        cache_rpc_outcome(&session, response.clone()).await.unwrap(),
        response
    );
    assert!(
        session
            .peer(PeerId::user(42).unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_indeterminate_commit_notifies_existing_and_later_pool_watchers() {
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(
        State::at_with_installation(
            temp.path().join("state"),
            vec![files],
            temp.path().to_owned(),
        )
        .unwrap(),
    );
    let session = Arc::new(PersistentSession::open(state.clone(), "watcher").unwrap());
    session.set_home_dc_id(2).await.unwrap();
    let watcher = tokio::spawn({
        let session = session.clone();
        async move { session.wait_for_write_uncertainty().await }
    });
    tokio::task::yield_now().await;
    assert!(!watcher.is_finished());
    state.fail_next_directory_sync_for_test();
    let failed = session.set_home_dc_id(3).await;
    assert!(failed.is_err());
    tokio::time::timeout(Duration::from_secs(1), watcher)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), session.wait_for_write_uncertainty())
        .await
        .unwrap();
    assert!(session.check_health().is_err());
    // The fault is per-State and consumed only once at the real durability
    // seam; it cannot poison another session's next successful commit.
    let healthy = PersistentSession::open(state, "unrelated").unwrap();
    healthy.set_home_dc_id(2).await.unwrap();
}

#[cfg(unix)]
fn private_session_state() -> (tempfile::TempDir, Arc<State>) {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(
        State::at_with_installation(
            temp.path().join("state"),
            vec![files],
            temp.path().to_owned(),
        )
        .unwrap(),
    );
    (temp, state)
}
#[cfg(unix)]
fn private_test_session_string() -> String {
    use base64::Engine;
    let mut bytes = vec![2, 149, 154, 167, 51];
    bytes.extend(443u16.to_be_bytes());
    bytes.extend([0x5a; 256]);
    format!(
        "1{}",
        base64::engine::general_purpose::URL_SAFE.encode(bytes)
    )
}

#[cfg(unix)]
#[tokio::test]
async fn an_indeterminate_session_commit_fences_later_writes_until_reopening_disk() {
    let (_temp, state) = private_session_state();
    let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    session
        .set_update_state(UpdateState::Primary {
            pts: 10,
            date: 10,
            seq: 1,
        })
        .await
        .unwrap();
    state.fail_next_directory_sync_for_test();
    let failed = session
        .set_update_state(UpdateState::Primary {
            pts: 20,
            date: 20,
            seq: 2,
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap()["updates_state"]["pts"],
        20
    );
    assert!(session.updates_state().await.is_err());
    assert!(session.set_home_dc_id(1).await.is_err());
    assert!(
        session
            .bind_source(Some("must-not-overwrite".into()))
            .is_err()
    );
    assert!(
        session
            .import_string_session(&private_test_session_string())
            .is_err()
    );
    assert_eq!(
        state.read_json("work.session.json", Value::Null).unwrap()["updates_state"]["pts"],
        20
    );
    drop(session);
    let reopened = PersistentSession::open(Arc::clone(&state), "work").unwrap();
    assert_eq!(reopened.updates_state().await.unwrap().pts, 20);
}

#[cfg(unix)]
#[tokio::test]
async fn indeterminate_source_and_import_commits_preserve_the_installed_snapshot_until_reopen() {
    for import in [false, true] {
        let (_temp, state) = private_session_state();
        let session = PersistentSession::open(Arc::clone(&state), "work").unwrap();
        state.fail_next_directory_sync_for_test();
        let result = if import {
            session.import_string_session(&private_test_session_string())
        } else {
            session.bind_source(Some("selected-revision".into()))
        };
        assert!(result.is_err());
        assert!(session.bind_source(None).is_err());
        assert!(session.set_home_dc_id(1).await.is_err());
        assert!(session.cache_peers(vec![]).await.is_err());
        let saved = state.read_json("work.session.json", Value::Null).unwrap();
        if import {
            assert_eq!(saved["authorization_origin"], "legacy");
        } else {
            assert_eq!(saved["authorization_source"], "selected-revision");
        }
        drop(session);
        let reopened = PersistentSession::open(Arc::clone(&state), "work").unwrap();
        if import {
            assert!(reopened.dc_option(2).unwrap().unwrap().auth_key.is_some());
        } else {
            assert!(
                reopened
                    .bind_source(Some("selected-revision".into()))
                    .is_ok()
            );
        }
        assert_eq!(
            state.read_json("work.session.json", Value::Null).unwrap(),
            saved
        );
    }
}
