use super::support::*;
use serde_json::{Value, json};
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
    let parsed: Value = serde_json::from_str(cursor).unwrap();
    assert_eq!(parsed["offset_id"], 8);
    assert_eq!(parsed["offset_rate"], 33);
    run(
        &api,
        "search_global",
        json!({"query":"needle","account":"work","page_size":3,"page":2,"cursor":cursor}),
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
        item["peer_id"] = peer;
        let api = Api::new(vec![("messages.searchGlobal", messages(vec![item]))]);
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
