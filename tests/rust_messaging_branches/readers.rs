use super::support::*;
use serde_json::{Value, json};

#[tokio::test]
async fn disappearing_listing_requires_a_positive_decoded_media_ttl() {
    let schema = telegram_mcp::tl::Schema::global();
    let mut rows = Vec::new();
    for (id, media) in [
        (
            1,
            json!({"_":"messageMediaPhoto","photo":{"_":"photoEmpty","id":"10"}}),
        ),
        (
            2,
            json!({"_":"messageMediaDocument","document":{"_":"documentEmpty","id":"20"}}),
        ),
        (
            3,
            json!({"_":"messageMediaPhoto","photo":{"_":"photoEmpty","id":"30"},"ttl_seconds":0}),
        ),
        (
            4,
            json!({"_":"messageMediaPhoto","photo":{"_":"photoEmpty","id":"40"},"ttl_seconds":30}),
        ),
        (
            5,
            json!({"_":"messageMediaDocument","document":{"_":"documentEmpty","id":"50"},"ttl_seconds":2147483647}),
        ),
    ] {
        let mut row = message(id, "");
        row["media"] = schema
            .decode("MessageMedia", &schema.encode_object(&media).unwrap())
            .unwrap();
        rows.push(row);
    }
    assert_eq!(rows[0]["media"].get("ttl_seconds"), Some(&Value::Null));
    let api = Api::new(vec![("messages.getHistory", messages(rows))]);
    let result = run(&api, "list_disappearing_media", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(
        result["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert_eq!(api.calls().len(), 1);
}

struct DelayedApi {
    api: Api,
    search_delay: u64,
    cursor_delay: u64,
    self_delay: u64,
}
#[async_trait::async_trait]
impl telegram_mcp::backend::Telegram for DelayedApi {
    async fn invoke(&self, method: &str, args: Value) -> anyhow::Result<Value> {
        let delay = match method {
            "messages.searchGlobal" => self.search_delay,
            "users.getUsers" => self.self_delay,
            _ => 0,
        };
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        self.api.invoke(method, args).await
    }
    async fn resolve_peer(&self, input: &Value) -> anyhow::Result<Value> {
        tokio::time::sleep(std::time::Duration::from_secs(self.cursor_delay)).await;
        self.api.resolve_peer(input).await
    }
}

#[tokio::test(start_paused = true)]
async fn global_cursor_timeout_preserves_fetched_page_within_search_budget() {
    let api = DelayedApi {
        api: Api::new(vec![(
            "messages.searchGlobal",
            messages(vec![message(8, "retained")]),
        )]),
        search_delay: 25,
        cursor_delay: 10,
        self_delay: 0,
    };
    let started = tokio::time::Instant::now();
    let result =
        telegram_mcp::tools::messaging::call(&api, "search_global", &json!({"query":"needle"}))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        tokio::time::Instant::now() - started,
        std::time::Duration::from_secs(30)
    );
    assert_eq!(result["messages"][0]["message"], "retained");
    assert_eq!(result["partial"], true);
    assert_eq!(result["exhausted"], false);
    assert!(result["next_cursor"].is_null());
    assert_eq!(api.api.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn legacy_global_cursor_timeout_keeps_fetched_batch_separate_from_requested_page() {
    let api = DelayedApi {
        api: Api::new(vec![
            (
                "messages.searchGlobal",
                messages(vec![message(8, "earlier page")]),
            ),
            ("messages.searchGlobal", messages(vec![])),
        ]),
        search_delay: 0,
        cursor_delay: 40,
        self_delay: 0,
    };
    let result = telegram_mcp::tools::messaging::call(
        &api,
        "search_global",
        &json!({"query":"needle","page":2,"page_size":1}),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["messages"], json!([]));
    assert_eq!(result["fetched_messages"][0]["message"], "earlier page");
    assert_eq!(result["legacy_page_reached"], false);
    assert_eq!(result["legacy_page_advance_remaining"], 1);
    assert_eq!(result["partial"], true);
    assert!(result["next_cursor"].is_null());
    assert_eq!(api.api.calls().len(), 1);
}
#[tokio::test]
async fn global_cursor_round_trip_preserves_query_account_filter_and_server_offsets() {
    let mut first = messages(vec![message(8, "match")]);
    first["next_rate"] = json!(33);
    let api = Api::new(vec![
        ("messages.searchGlobal", first),
        ("messages.searchGlobal", messages(vec![])),
    ]);
    let result = run(
        &api,
        "search_global",
        json!({"query":"needle","account":"work","page_size":3}),
    )
    .await
    .unwrap();
    let cursor = result["next_cursor"].as_str().unwrap();
    assert!(
        cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    );
    run(
        &api,
        "search_global",
        json!({"query":"needle","account":"work","page_size":3,"page":1,"cursor":cursor}),
    )
    .await
    .unwrap();
    let req = &api.calls()[1].1;
    assert_eq!(req["offset_rate"], 33);
    assert_eq!(req["offset_id"], 8);
    assert_eq!(req["offset_peer"]["user_id"], "7");
    assert_eq!(api.calls().len(), 2);
}
#[tokio::test]
async fn global_search_marks_user_group_and_channel_peers_before_resolving_cursor_capabilities() {
    for (peer, wanted) in [
        (json!({"_":"peerUser","user_id":7}), "7"),
        (json!({"_":"peerChat","chat_id":"9"}), "-9"),
        (
            json!({"_":"peerChannel","channel_id":"9"}),
            "-1000000000009",
        ),
    ] {
        let mut item = message(8, "match");
        let mut input = peer.clone();
        input["_"] = json!(match peer["_"].as_str().unwrap() {
            "peerUser" => "inputPeerUser",
            "peerChat" => "inputPeerChat",
            _ => "inputPeerChannel",
        });
        input["access_hash"] = json!("13");
        item["peer_id"] = peer;
        let api = Api::new(vec![("messages.searchGlobal", messages(vec![item]))])
            .identity(json!(wanted), input);
        let result = run(&api, "search_global", json!({"query":"needle"}))
            .await
            .unwrap();
        assert!(result["next_cursor"].is_string());
        assert_eq!(api.resolved.lock().unwrap().as_slice(), [json!(wanted)]);
    }
}
#[tokio::test]
async fn malformed_or_cross_scope_cursors_are_rejected_before_search() {
    for cursor in [
        "not-json".to_owned(),
        json!({"query":"other","filter":{"_":"inputMessagesFilterEmpty"},"account":"work"})
            .to_string(),
        json!({"query":"needle","filter":{"_":"inputMessagesFilterPhotos"},"account":"work"})
            .to_string(),
        json!({"query":"needle","filter":{"_":"inputMessagesFilterEmpty"},"account":"other"})
            .to_string(),
    ] {
        let api = Api::new(vec![]);
        assert!(
            run(
                &api,
                "search_global",
                json!({"query":"needle","account":"work","page":2,"cursor":cursor})
            )
            .await
            .is_err()
        );
        assert!(api.calls().is_empty());
    }
}
#[tokio::test]
async fn invalid_server_cursor_peers_cannot_produce_a_reusable_capability() {
    for peer in [
        None,
        Some(json!({"_":"peerUnknown"})),
        Some(json!({"_":"peerUser","user_id":"invalid"})),
    ] {
        let mut item = message(8, "match");
        if let Some(peer) = peer {
            item["peer_id"] = peer;
        } else {
            item.as_object_mut().unwrap().remove("peer_id");
        }
        let api = Api::new(vec![("messages.searchGlobal", messages(vec![item]))]);
        assert!(
            run(&api, "search_global", json!({"query":"needle"}))
                .await
                .is_err()
        );
        assert!(api.resolved.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn legacy_global_pages_advance_the_server_cursor_instead_of_repeating_page_one() {
    let mut first = messages(vec![message(9, "first")]);
    first["next_rate"] = json!(23);
    let api = Api::new(vec![
        ("messages.searchGlobal", first),
        (
            "messages.searchGlobal",
            messages(vec![message(4, "second")]),
        ),
    ]);
    let result = run(
        &api,
        "search_global",
        json!({"query":"needle","page":2,"page_size":1}),
    )
    .await
    .unwrap();
    assert_eq!(result["messages"][0]["id"], 4);
    assert_eq!(api.calls()[1].1["offset_rate"], 23);
    assert_eq!(api.calls()[1].1["offset_id"], 9);
    assert_eq!(api.calls()[1].1["offset_peer"]["user_id"], "7");
    let api = Api::new(vec![]);
    assert!(
        run(
            &api,
            "search_global",
            json!({"query":"needle","page":2502,"page_size":1})
        )
        .await
        .is_err()
    );
    assert!(api.calls().is_empty());
}

#[tokio::test]
async fn empty_media_sender_search_filters_a_bounded_topic_history_locally() {
    let mut photo = message(8, "photo");
    photo["from_id"] = json!({"_":"peerUser","user_id":"9"});
    photo["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"4"}});
    let mut wrong = photo.clone();
    wrong["id"] = json!(7);
    wrong["from_id"]["user_id"] = json!("10");
    let mut text = message(6, "plain");
    text["from_id"] = photo["from_id"].clone();
    for item in [&mut photo, &mut wrong, &mut text] {
        item["peer_id"] = json!({"_":"peerChannel","channel_id":"7"});
        item["reply_to"] = json!({"forum_topic":true,"reply_to_top_id":55});
    }
    let api = Api::new(vec![(
        "messages.getReplies",
        messages(vec![photo, wrong, text]),
    )])
    .identity(
        json!(7),
        json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"13"}),
    )
    .identity(
        json!(9),
        json!({"_":"inputPeerUser","user_id":"9","access_hash":"14"}),
    );
    let result = run(
        &api,
        "search_messages",
        json!({"chat_id":7,"topic_id":55,"from_user":9,"media_type":"photos","query":"","limit":2}),
    )
    .await
    .unwrap();
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"][0]["id"], 8);
    assert_eq!(result["scanned"], 3);
    assert_eq!(result["scan_limit"], 2500);
    assert_eq!(api.calls()[0].1["msg_id"], 55);
    assert!(
        api.calls()
            .iter()
            .all(|(name, _)| !name.to_ascii_lowercase().contains("read"))
    );
}

#[tokio::test]
async fn sender_media_scans_preserve_media_classes_and_ignore_another_peer_or_topic() {
    let examples = [
        (
            "voice",
            json!({"_":"messageMediaDocument","document":{"_":"document","attributes":[{"_":"documentAttributeAudio","voice":true}]}}),
        ),
        (
            "music",
            json!({"_":"messageMediaDocument","document":{"_":"document","attributes":[{"_":"documentAttributeAudio","voice":false}]}}),
        ),
        (
            "round_video",
            json!({"_":"messageMediaDocument","document":{"_":"document","attributes":[{"_":"documentAttributeVideo","round_message":true}]}}),
        ),
        (
            "videos",
            json!({"_":"messageMediaDocument","document":{"_":"document","attributes":[{"_":"documentAttributeVideo","round_message":false}]}}),
        ),
        (
            "files",
            json!({"_":"messageMediaDocument","document":{"_":"document","attributes":[{"_":"documentAttributeFilename","file_name":"fixture.txt"}]}}),
        ),
        (
            "gifs",
            json!({"_":"messageMediaDocument","document":{"_":"document","attributes":[{"_":"documentAttributeAnimated"}]}}),
        ),
        ("contacts", json!({"_":"messageMediaContact"})),
        ("geo", json!({"_":"messageMediaVenue"})),
        ("links", json!({"_":"messageMediaWebPage"})),
    ];
    for (kind, media) in examples {
        let mut good = message(9, "matching");
        good["from_id"] = json!({"_":"peerUser","user_id":"7"});
        good["media"] = media;
        let mut wrong = good.clone();
        wrong["id"] = json!(10);
        wrong["peer_id"]["user_id"] = json!("8");
        let api = Api::new(vec![("messages.getHistory", messages(vec![wrong, good]))]);
        let result = run(
            &api,
            "search_messages",
            json!({"chat_id":7,"from_user":7,"media_type":kind,"query":"","limit":2}),
        )
        .await
        .unwrap();
        assert_eq!(result["messages"].as_array().unwrap().len(), 1, "{kind}");
        assert_eq!(result["messages"][0]["id"], 9);
        assert_eq!(result["scanned"], 2);
    }
}

#[tokio::test]
async fn sender_media_scan_ceiling_reports_incomplete_results_and_stops_network_reads() {
    let mut pages = vec![];
    for page in 0..25 {
        let items = (0..100)
            .map(|index| {
                let mut m = message(5000 - page * 100 - index, "no media");
                m["from_id"] = json!({"_":"peerUser","user_id":"7"});
                m
            })
            .collect();
        pages.push(("messages.getHistory", messages(items)));
    }
    let api = Api::new(pages);
    let result = run(
        &api,
        "search_messages",
        json!({"chat_id":7,"from_user":7,"media_type":"photos","query":"","limit":1}),
    )
    .await
    .unwrap();
    assert_eq!(result["scanned"], 2500);
    assert_eq!(result["partial"], true);
    assert_eq!(result["exhausted"], false);
    assert_eq!(result["messages"], json!([]));
    assert_eq!(api.calls().len(), 25);
}

#[tokio::test]
async fn legacy_global_search_reports_stalled_cursor_advancement_without_returning_the_wrong_page()
{
    let mut batch = messages(vec![message(9, "page before target")]);
    batch["next_rate"] = json!(23);
    let api = Api::new(vec![
        ("messages.searchGlobal", batch.clone()),
        ("messages.searchGlobal", batch),
    ]);
    let result = run(
        &api,
        "search_global",
        json!({"query":"needle","page":4,"page_size":1}),
    )
    .await
    .unwrap();
    assert_eq!(result["partial"], true);
    assert_eq!(result["messages"], json!([]));
    assert_eq!(result["legacy_page_advance_remaining"], 1);
    assert_eq!(api.calls().len(), 2);
}

#[tokio::test]
async fn sender_media_search_recognizes_outgoing_private_messages_with_implicit_self_sender() {
    let mut own = message(9, "own photo");
    own["out"] = json!(true);
    own["peer_id"] = json!({"_":"peerUser","user_id":"8"});
    own["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"4"}});
    let api = Api::new(vec![
        ("users.getUsers", json!([{"_":"user","id":"7"}])),
        ("messages.getHistory", messages(vec![own])),
    ])
    .identity(
        json!(8),
        json!({"_":"inputPeerUser","user_id":"8","access_hash":"18"}),
    )
    .identity(json!("me"), json!({"_":"inputPeerSelf"}));
    let result = run(
        &api,
        "search_messages",
        json!({"chat_id":8,"from_user":"me","media_type":"photos","query":""}),
    )
    .await
    .unwrap();
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"][0]["id"], 9);
}

#[tokio::test]
async fn general_topic_sender_media_scans_history_without_cross_topic_matches() {
    let make = |id, reply: Value| {
        let mut item = message(id, "photo");
        item["peer_id"] = json!({"_":"peerChannel","channel_id":"7"});
        item["from_id"] = json!({"_":"peerUser","user_id":"9"});
        item["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"4"}});
        item["reply_to"] = reply;
        item
    };
    let api = Api::new(vec![
        (
            "channels.getChannels",
            json!({"chats":[{"_":"channel","id":"7","forum":true}]}),
        ),
        (
            "messages.getHistory",
            messages(vec![
                make(10, Value::Null),
                make(9, json!({"forum_topic":false,"reply_to_msg_id":77})),
                make(
                    8,
                    json!({"forum_topic":true,"reply_to_top_id":55,"reply_to_msg_id":77}),
                ),
                make(7, json!({"forum_topic":true,"reply_to_msg_id":55})),
                make(1, json!({"forum_topic":true,"reply_to_top_id":55})),
            ]),
        ),
    ])
    .identity(
        json!(7),
        json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"13"}),
    )
    .identity(
        json!(9),
        json!({"_":"inputPeerUser","user_id":"9","access_hash":"14"}),
    );
    let result = run(
        &api,
        "search_messages",
        json!({"chat_id":7,"topic_id":1,"from_user":9,"media_type":"photos","query":"","limit":10}),
    )
    .await
    .unwrap();
    assert_eq!(
        result["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![10, 9]
    );
    assert_eq!(result["scanned"], 5);
    assert_eq!(result["exhausted"], true);
    let calls = api.calls();
    assert_eq!(calls[0].0, "channels.getChannels");
    assert!(calls[1].1.get("msg_id").is_none());
    assert_eq!(calls.len(), 2);
}

#[tokio::test]
async fn sender_media_numeric_and_username_self_aliases_include_implicit_outgoing_messages() {
    for selector in [json!(7), json!("@self_fixture")] {
        let mut own = message(9, "own photo");
        own["out"] = json!(true);
        own["peer_id"] = json!({"_":"peerUser","user_id":"8"});
        own["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"4"}});
        let mut next = own.clone();
        next["id"] = json!(8);
        let mut explicit_other = own.clone();
        explicit_other["id"] = json!(7);
        explicit_other["from_id"] = json!({"_":"peerUser","user_id":"9"});
        let api = Api::new(vec![
            (
                "messages.getHistory",
                messages(vec![own, next, explicit_other]),
            ),
            ("users.getUsers", json!([{"_":"user","id":7}])),
        ])
        .identity(
            json!(8),
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"18"}),
        )
        .identity(
            selector.clone(),
            json!({"_":"inputPeerUser","user_id":"7","access_hash":"17"}),
        );
        let result = run(
            &api,
            "search_messages",
            json!({"chat_id":8,"from_user":selector,"media_type":"photos","query":"","limit":10}),
        )
        .await
        .unwrap();
        assert_eq!(
            result["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![9, 8]
        );
        assert_eq!(
            api.calls().len(),
            2,
            "self identity should be reused within the scan"
        );
        assert_eq!(api.calls()[1].1["id"], json!([{"_":"inputUserSelf"}]));
    }
}

#[tokio::test]
async fn sender_media_implicit_outgoing_messages_do_not_match_another_user() {
    let mut own = message(9, "own photo");
    own["out"] = json!(true);
    own["peer_id"] = json!({"_":"peerUser","user_id":"8"});
    own["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"4"}});
    let api = Api::new(vec![
        ("messages.getHistory", messages(vec![own])),
        ("users.getUsers", json!([{"_":"user","id":"7"}])),
    ])
    .identity(
        json!(8),
        json!({"_":"inputPeerUser","user_id":"8","access_hash":"18"}),
    )
    .identity(
        json!(9),
        json!({"_":"inputPeerUser","user_id":"9","access_hash":"19"}),
    );
    let result = run(
        &api,
        "search_messages",
        json!({"chat_id":8,"from_user":9,"media_type":"photos","query":""}),
    )
    .await
    .unwrap();
    assert_eq!(result["messages"], json!([]));
    assert_eq!(result["exhausted"], true);
}

#[tokio::test(start_paused = true)]
async fn sender_media_self_identity_timeout_keeps_prior_matches_within_scan_budget() {
    let mut own = message(9, "explicit own photo");
    own["out"] = json!(true);
    own["peer_id"] = json!({"_":"peerUser","user_id":"8"});
    own["media"] = json!({"_":"messageMediaPhoto","photo":{"_":"photo","id":"4"}});
    own["from_id"] = json!({"_":"peerUser","user_id":"7"});
    let mut implicit = own.clone();
    implicit["id"] = json!(8);
    implicit["from_id"] = Value::Null;
    let api = DelayedApi {
        api: Api::new(vec![("messages.getHistory", messages(vec![own, implicit]))]).identity(
            json!(8),
            json!({"_":"inputPeerUser","user_id":"8","access_hash":"18"}),
        ),
        search_delay: 0,
        cursor_delay: 0,
        self_delay: 40,
    };
    let started = tokio::time::Instant::now();
    let result = telegram_mcp::tools::messaging::call(
        &api,
        "search_messages",
        &json!({"chat_id":8,"from_user":7,"media_type":"photos","query":"","limit":10}),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"][0]["id"], 9);
    assert_eq!(result["partial"], true);
    assert_eq!(result["exhausted"], false);
    assert!(tokio::time::Instant::now() - started <= std::time::Duration::from_secs(30));
    assert_eq!(api.api.calls().len(), 1);
}
