use super::common::*;

#[tokio::test]
async fn statistics_async_token_is_not_reported_as_graph() {
    let m = Mock::new()
        .respond(
            "channels.getChannels",
            json!({"chats":[{"broadcast":true}]}),
        )
        .respond(
            "stats.getBroadcastStats",
            json!({"views_graph":{"_":"statsGraphAsync","token":"opaque"}}),
        );
    let r = run(
        &m,
        "get_channel_statistics",
        json!({"chat_id":7,"resolve_graphs":false}),
    )
    .await
    .unwrap();
    assert_eq!(r["views_graph"]["status"], "not_loaded");
    assert_eq!(m.count("stats.loadAsyncGraph"), 0);
    assert!(r["views_graph"].get("token").is_none());
}

#[tokio::test]
async fn statistics_async_graph_is_resolved() {
    let m=Mock::new().respond("channels.getChannels",json!({"chats":[{"broadcast":true}]})).respond("stats.getBroadcastStats",json!({"views_graph":{"_":"statsGraphAsync","token":"opaque"}})).respond("stats.loadAsyncGraph",json!({"_":"statsGraph","json":{"data":"{\"columns\":[[\"x\",0],[\"y0\",4]],\"names\":{\"y0\":\"Views\"}}"}}));
    let r = run(
        &m,
        "get_channel_statistics",
        json!({"chat_id":7,"include_graph_data":true}),
    )
    .await
    .unwrap();
    assert_eq!(r["views_graph"]["status"], "loaded");
    assert_eq!(r["views_graph"]["series"], json!(["Views"]));
    assert!(r["views_graph"]["columns"].is_array());
    assert_eq!(m.count("stats.loadAsyncGraph"), 1);
}

#[tokio::test]
async fn statistic_counter_preserves_previous_and_delta() {
    let m = Mock::new()
        .respond(
            "channels.getChannels",
            json!({"chats":[{"broadcast":true}]}),
        )
        .respond(
            "stats.getBroadcastStats",
            json!({"followers":{"_":"statsAbsValueAndPrev","current":120.0,"previous":100.0}}),
        );
    let r = run(&m, "get_channel_statistics", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(r["followers"]["current"], 120.0);
    assert_eq!(r["followers"]["previous"], 100.0);
    assert_eq!(r["followers"]["delta"], 20.0);
}
