use super::*;
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
