//! Behavioral ports from message deletion, entity, paging and scheduling suites.
#[path = "common/messaging.rs"]
mod support;
use anyhow::anyhow;
use serde_json::json;
use std::sync::atomic::Ordering;
use support::*;

#[tokio::test]
async fn invalid_entity_refuses_before_resolving_peer() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "send_message",
            json!({"chat_id":7,"message":"x","entities":[bold(5,1)]})
        )
        .await
        .is_err()
    );
    assert_eq!(w.resolves.load(Ordering::Relaxed), 0);
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn reads_never_send_read_acknowledgements() {
    let w = Wire::new(vec![messages(vec![message(8, "hello")])]);
    run(&w, "get_history", json!({"chat_id":7})).await.unwrap();
    assert_eq!(
        w.calls().iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
        vec!["messages.getHistory"]
    );
}
#[tokio::test]
async fn page_limit_is_capped_and_reported() {
    let w = Wire::new(vec![messages(vec![])]);
    let result = run(
        &w,
        "get_messages",
        json!({"chat_id":7,"page_size":999,"page":3}),
    )
    .await
    .unwrap();
    assert_eq!(result["effective_limit"], 200);
    assert_eq!(result["requested_limit"], 999);
    assert_eq!(w.calls()[0].1["add_offset"], 400);
}
#[tokio::test]
async fn page_zero_is_refused_without_network() {
    let w = Wire::new(vec![]);
    assert!(
        run(&w, "get_messages", json!({"chat_id":7,"page":0}))
            .await
            .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn far_page_refuses_unbounded_scan() {
    let w = Wire::new(vec![]);
    assert!(
        run(&w, "get_messages", json!({"chat_id":7,"page":10000}))
            .await
            .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn from_date_after_to_date_is_refused() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "list_messages",
            json!({"chat_id":7,"from_date":"2026-06-01","to_date":"2026-05-01"})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn filtered_messages_use_server_side_date_bounds() {
    let w = Wire::new(vec![messages(vec![])]);
    run(
        &w,
        "list_messages",
        json!({"chat_id":7,"search_query":"hi","from_date":"2026-05-05","to_date":"2026-05-15"}),
    )
    .await
    .unwrap();
    let (method, params) = &w.calls()[0];
    assert_eq!(method, "messages.search");
    assert_eq!(params["q"], "hi");
    assert_eq!(params["max_date"], 1778889600);
    assert!(params["min_date"].as_i64().unwrap() > 0);
}
#[tokio::test]
async fn history_topic_uses_replies_not_whole_chat() {
    let w = Wire::new(vec![messages(vec![])]);
    run(&w, "get_history", json!({"chat_id":7,"topic_id":55}))
        .await
        .unwrap();
    assert_eq!(w.calls()[0].0, "messages.getReplies");
    assert_eq!(w.calls()[0].1["msg_id"], 55);
}
#[tokio::test]
async fn view_lookup_does_not_increment_counts() {
    let w = Wire::new(vec![json!({"views":[]})]);
    run(
        &w,
        "get_message_views",
        json!({"chat_id":7,"message_ids":[1,2]}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[0].1["increment"], false);
}
#[tokio::test]
async fn channel_inspection_uses_channel_scoped_message_ids() {
    let mut post = message(9, "post");
    post["peer_id"] = json!({"_":"peerChannel","channel_id":"7"});
    let w = Wire::channel(vec![messages(vec![post])]);
    run(&w, "inspect_message", json!({"chat_id":7,"message_id":9}))
        .await
        .unwrap();
    assert_eq!(w.calls()[0].0, "channels.getMessages");
    assert_eq!(w.calls()[0].1["channel"]["_"], "inputChannel");
}
#[tokio::test]
async fn legacy_global_page_reports_exhaustion_without_inventing_matches() {
    let w = Wire::new(vec![messages(vec![])]);
    let result = run(&w, "search_global", json!({"query":"q","page":2}))
        .await
        .unwrap();
    assert_eq!(result["messages"], json!([]));
    assert_eq!(result["exhausted"], true);
    assert_eq!(w.calls().len(), 1);
}

#[tokio::test]
async fn global_cursor_is_bound_to_query_and_account() {
    let w = Wire::new(vec![]);
    let cursor = json!({"query":"a","filter":{"_":"inputMessagesFilterEmpty"},"account":"alice"})
        .to_string();
    assert!(
        run(
            &w,
            "search_global",
            json!({"query":"b","account":"bob","cursor":cursor})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn sending_entities_and_effect_preserves_exact_ids() {
    let w = Wire::new(vec![]);
    run(&w,"send_message",json!({"chat_id":7,"message":"hello","entities":[{"type":"custom_emoji","offset":0,"length":5,"custom_emoji_id":"5934007978150595964"}],"effect_id":"5100000000000000011"})).await.unwrap();
    let req = &w.calls()[0].1;
    assert_eq!(req["entities"][0]["document_id"], "5934007978150595964");
    assert_eq!(req["effect"], "5100000000000000011");
}
#[tokio::test]
async fn send_keeps_topic_and_reply_id_distinct() {
    let w = Wire::new(vec![]);
    run(
        &w,
        "send_message",
        json!({"chat_id":7,"message":"text","topic_id":10,"reply_to_message_id":22}),
    )
    .await
    .unwrap();
    let req = &w.calls()[0].1;
    assert_eq!(req["reply_to"]["reply_to_msg_id"], 22);
    assert_eq!(req["reply_to"]["top_msg_id"], 10);
}
#[tokio::test]
async fn ordinary_message_has_no_rich_request() {
    let w = Wire::new(vec![]);
    run(&w, "send_message", json!({"chat_id":7,"message":"text"}))
        .await
        .unwrap();
    assert!(w.calls()[0].1.get("rich_message").is_none());
}
#[tokio::test]
async fn rich_markdown_uses_native_server_parsing() {
    let w = Wire::new(vec![]);
    run(
        &w,
        "send_message",
        json!({"chat_id":7,"message":"# heading","parse_mode":"rich_markdown","rich_rtl":true}),
    )
    .await
    .unwrap();
    let req = &w.calls()[0].1;
    assert_eq!(req["rich_message"]["_"], "inputRichMessageMarkdown");
    assert_eq!(req["rich_message"]["rtl"], true);
}
#[tokio::test]
async fn normal_entity_and_parse_mode_conflict_is_refused() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "send_message",
            json!({"chat_id":7,"message":"text","parse_mode":"html","entities":[bold(0,4)]})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn unchanged_edit_is_reported_without_retry() {
    let w = Wire::new(vec![]);
    w.replies
        .lock()
        .unwrap()
        .push_back(Err(anyhow!("MESSAGE_NOT_MODIFIED")));
    let result = run(
        &w,
        "edit_message",
        json!({"chat_id":7,"message_id":4,"new_text":"same"}),
    )
    .await
    .unwrap();
    assert_eq!(result["not_modified"], true);
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn scheduled_message_uses_native_repeat() {
    let w = Wire::new(vec![]);
    run(
        &w,
        "schedule_message",
        json!({"chat_id":7,"message":"daily","when":1893456000_i64,"repeat":"daily"}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[0].1["schedule_repeat_period"], 86400);
}
#[tokio::test]
async fn clearing_draft_preserves_topic_address() {
    let w = Wire::new(vec![]);
    run(&w, "clear_draft", json!({"chat_id":7,"topic_id":10}))
        .await
        .unwrap();
    assert_eq!(w.calls()[0].1["message"], "");
    assert_eq!(w.calls()[0].1["reply_to"]["top_msg_id"], 10);
}
#[tokio::test]
async fn history_deletion_repeats_positive_offset_until_zero() {
    let w = Wire::new(vec![
        json!({"offset":50}),
        json!({"offset":10}),
        json!({"offset":0}),
    ]);
    let result = run(&w, "delete_chat_history", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(result["passes"], 3);
    assert_eq!(w.calls().len(), 3);
    assert!(result.get("message_count").is_none());
}
#[tokio::test]
async fn stalled_history_deletion_stops_on_second_pass() {
    let w = Wire::new(vec![json!({"offset":50}), json!({"offset":50})]);
    assert!(
        run(&w, "delete_chat_history", json!({"chat_id":7}))
            .await
            .unwrap_err()
            .to_string()
            .contains("incomplete")
    );
    assert_eq!(w.calls().len(), 2);
}
#[tokio::test]
async fn channel_private_deletion_is_refused() {
    let w = Wire::channel(vec![]);
    assert!(
        run(&w, "delete_message", json!({"chat_id":7,"message_id":10}))
            .await
            .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn channel_revocation_uses_scoped_rpc() {
    let w = Wire::channel(vec![]);
    run(
        &w,
        "delete_message",
        json!({"chat_id":7,"message_id":10,"revoke":true}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[0].0, "channels.deleteMessages");
    assert_eq!(w.calls()[0].1["id"], json!([10]));
}
#[tokio::test]
async fn wrong_peer_message_cannot_be_deleted() {
    let mut wrong = message(10, "other");
    wrong["peer_id"]["user_id"] = json!("8");
    let w = Wire::new(vec![messages(vec![wrong])]);
    assert!(
        run(&w, "delete_message", json!({"chat_id":7,"message_id":10}))
            .await
            .is_err()
    );
    assert_eq!(w.calls().len(), 1);
    assert_eq!(w.calls()[0].0, "messages.getMessages");
}
#[tokio::test]
async fn existing_private_message_is_preflighted_before_deletion() {
    let w = Wire::new(vec![
        messages(vec![message(10, "mine")]),
        json!({"pts_count":1}),
    ]);
    run(&w, "delete_message", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    assert_eq!(w.calls()[1].0, "messages.deleteMessages");
    assert_eq!(w.calls()[1].1["revoke"], false);
}
#[tokio::test]
async fn missing_message_is_not_reported_as_newly_deleted() {
    let w = Wire::new(vec![messages(vec![])]);
    let result = run(&w, "delete_message", json!({"chat_id":7,"message_id":10}))
        .await
        .unwrap();
    assert_eq!(result["already_missing"], true);
    assert_eq!(w.calls().len(), 1);
}
#[tokio::test]
async fn empty_bulk_deletion_is_refused() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "delete_messages_bulk",
            json!({"chat_id":7,"message_ids":[]})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn deleting_boolean_message_id_is_refused() {
    let w = Wire::new(vec![]);
    assert!(
        run(&w, "delete_message", json!({"chat_id":7,"message_id":true}))
            .await
            .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn dropping_captions_requires_dropping_author() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "forward_messages",
            json!({"from_chat_id":7,"to_chat_id":8,"message_ids":[1],"drop_captions":true})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn copying_uses_one_random_id_per_message() {
    let w = Wire::new(vec![]);
    run(
        &w,
        "copy_message",
        json!({"from_chat_id":7,"to_chat_id":8,"message_id":[1,2],"expand_album":false}),
    )
    .await
    .unwrap();
    let req = &w.calls()[0].1;
    assert_eq!(req["drop_author"], true);
    assert_eq!(req["random_id"].as_array().unwrap().len(), 2);
    assert_ne!(req["random_id"][0], req["random_id"][1]);
}
#[tokio::test]
async fn album_expansion_keeps_source_message_and_siblings() {
    let mut original = message(10, "one");
    original["grouped_id"] = json!("999");
    let mut sibling = message(11, "two");
    sibling["grouped_id"] = json!("999");
    let w = Wire::new(vec![
        messages(vec![original.clone()]),
        messages(vec![original, sibling]),
        json!({}),
    ]);
    run(
        &w,
        "forward_message",
        json!({"from_chat_id":7,"to_chat_id":8,"message_id":10}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[2].1["id"], json!([10, 11]));
}
#[tokio::test]
async fn default_pin_is_silent() {
    let w = Wire::new(vec![]);
    run(&w, "pin_message", json!({"chat_id":7,"message_id":4}))
        .await
        .unwrap();
    assert_eq!(w.calls()[0].1["silent"], true);
}
#[tokio::test]
async fn notify_must_be_boolean() {
    let w = Wire::new(vec![]);
    assert!(
        run(
            &w,
            "pin_message",
            json!({"chat_id":7,"message_id":4,"notify":"yes"})
        )
        .await
        .is_err()
    );
    assert!(w.calls().is_empty());
}
#[tokio::test]
async fn removing_reaction_sends_empty_vector() {
    let w = Wire::new(vec![]);
    run(&w, "remove_reaction", json!({"chat_id":7,"message_id":4}))
        .await
        .unwrap();
    assert_eq!(w.calls()[0].1["reaction"], json!([]));
}
#[tokio::test]
async fn custom_reaction_ids_do_not_round() {
    let w = Wire::new(vec![]);
    run(
        &w,
        "send_reaction",
        json!({"chat_id":7,"message_id":4,"custom_emoji_id":"5934007978150595964"}),
    )
    .await
    .unwrap();
    assert_eq!(
        w.calls()[0].1["reaction"][0]["document_id"],
        "5934007978150595964"
    );
}
#[tokio::test]
async fn marking_topic_read_uses_its_real_top_message() {
    let w = Wire::new(vec![
        json!({"topics":[{"id":3,"top_message":77}]}),
        json!(true),
        json!({}),
    ]);
    run(&w, "mark_as_read", json!({"chat_id":7,"topic_id":3}))
        .await
        .unwrap();
    assert_eq!(w.calls()[1].1["read_max_id"], 77);
    assert_eq!(w.calls()[2].0, "messages.readMentions");
}
#[tokio::test]
async fn custom_emoji_swap_preserves_other_entities_and_text() {
    let mut msg = message(10, "hello world");
    msg["entities"] = json!([{"_":"messageEntityCustomEmoji","offset":0,"length":5,"document_id":"111"},{"_":"messageEntityBold","offset":6,"length":5}]);
    let w = Wire::new(vec![messages(vec![msg]), json!({})]);
    let result = run(
        &w,
        "replace_custom_emoji",
        json!({"chat_id":7,"message_id":10,"new_document_id":"5934007978150595964"}),
    )
    .await
    .unwrap();
    assert_eq!(result["changed"], 1);
    let req = &w.calls()[1].1;
    assert_eq!(req["message"], "hello world");
    assert_eq!(req["entities"][1]["_"], "messageEntityBold");
    assert_eq!(req["entities"][0]["document_id"], "5934007978150595964");
}

#[tokio::test]
async fn server_ignored_date_filters_are_rechecked_locally() {
    let mut too_new = message(30, "new");
    too_new["date"] = json!(1778889601);
    let mut inside = message(20, "inside");
    inside["date"] = json!(1778000000);
    let mut too_old = message(10, "old");
    too_old["date"] = json!(1);
    let w = Wire::new(vec![messages(vec![too_new, inside, too_old])]);
    let result = run(
        &w,
        "list_messages",
        json!({"chat_id":7,"from_date":"2026-05-05","to_date":"2026-05-15"}),
    )
    .await
    .unwrap();
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"][0]["message"], "inside");
}
#[tokio::test]
async fn topic_scoped_search_uses_the_topic_field_on_wire() {
    let w = Wire::new(vec![messages(vec![])]);
    run(
        &w,
        "search_messages",
        json!({"chat_id":7,"query":"term","topic_id":42}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[0].1["top_msg_id"], 42);
}
#[tokio::test]
async fn topic_scoped_paging_never_falls_back_to_whole_history() {
    let w = Wire::new(vec![messages(vec![])]);
    run(&w, "get_messages", json!({"chat_id":7,"topic_id":42}))
        .await
        .unwrap();
    assert_eq!(w.calls()[0].0, "messages.getReplies");
    assert_eq!(w.calls()[0].1["msg_id"], 42);
}
#[tokio::test]
async fn zero_context_returns_only_the_selected_message_window() {
    let w = Wire::new(vec![messages(vec![message(10, "selected")])]);
    run(
        &w,
        "get_message_context",
        json!({"chat_id":7,"message_id":10,"context_size":0}),
    )
    .await
    .unwrap();
    assert_eq!(w.calls()[0].1["limit"], 1);
}

#[tokio::test]
async fn fetching_an_account_global_id_never_returns_another_chats_message() {
    use telegram_mcp::tools::messaging::helpers::fetch;
    let mut other = message(9, "private in another chat");
    other["peer_id"] = json!({"_":"peerUser","user_id":"8"});
    let w = Wire::new(vec![messages(vec![other])]);
    let error = fetch(
        &w,
        &json!({"_":"inputPeerUser","user_id":"7","access_hash":"13"}),
        9,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("requested chat"));
    assert_eq!(w.calls().len(), 1);
}

#[tokio::test]
async fn fetched_messages_must_match_the_requested_peer_type_and_id() {
    use telegram_mcp::tools::messaging::helpers::fetch;
    for peer in [
        json!({"_":"inputPeerUser","user_id":"7","access_hash":"13"}),
        json!({"_":"inputPeerChat","chat_id":"7"}),
        json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"13"}),
    ] {
        for wrong in [
            json!(null),
            json!({"_":"peerUser","user_id":"8"}),
            json!({"_":"peerChat","chat_id":"8"}),
            json!({"_":"peerChannel","channel_id":"8"}),
        ] {
            let mut source = message(9, "unrelated content");
            source["peer_id"] = wrong;
            let w = Wire::new(vec![messages(vec![source])]);
            assert!(fetch(&w, &peer, 9).await.is_err());
            assert_eq!(w.calls().len(), 1);
        }
    }
    let w = Wire::new(vec![
        json!([{"_":"user","id":"7"}]),
        messages(vec![message(9, "saved message")]),
    ]);
    assert_eq!(
        fetch(&w, &json!({"_":"inputPeerSelf"}), 9).await.unwrap()["message"],
        "saved message"
    );
    assert_eq!(
        w.calls()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["users.getUsers", "messages.getMessages"]
    );
    let w = Wire::new(vec![json!([])]);
    assert!(fetch(&w, &json!({"_":"inputPeerSelf"}), 9).await.is_err());
    assert_eq!(w.calls().len(), 1);
}
