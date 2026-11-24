use super::common::*;

#[tokio::test]
async fn general_topic_is_never_deleted() {
    let m = Mock::new();
    assert!(
        run(&m, "delete_forum_topic", json!({"chat_id":7,"topic_id":1}))
            .await
            .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn topic_deletion_continues_until_offset_zero() {
    let m = Mock::new()
        .forum()
        .respond("messages.deleteTopicHistory", json!({"offset":40}))
        .respond("messages.deleteTopicHistory", json!({"offset":0}));
    let r = run(&m, "delete_forum_topic", json!({"chat_id":7,"topic_id":12}))
        .await
        .unwrap();
    assert_eq!(r["deleted"], true);
    assert_eq!(r["rounds"], 2);
    assert_eq!(m.count("messages.deleteTopicHistory"), 2);
}

#[tokio::test]
async fn topic_deletion_reports_incomplete_at_bound() {
    let mut m = Mock::new().forum();
    for _ in 0..50 {
        m = m.respond("messages.deleteTopicHistory", json!({"offset":1}));
    }
    let r = run(&m, "delete_forum_topic", json!({"chat_id":7,"topic_id":12}))
        .await
        .unwrap();
    assert_eq!(r["deleted"], false);
    assert_eq!(m.count("messages.deleteTopicHistory"), 50);
}

#[tokio::test]
async fn edit_topic_no_changes_costs_no_request() {
    let m = Mock::new();
    assert!(
        run(&m, "edit_forum_topic", json!({"chat_id":7,"topic_id":2}))
            .await
            .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn false_topic_field_is_sent_and_omitted_title_stays_omitted() {
    let m = Mock::new().forum();
    run(
        &m,
        "edit_forum_topic",
        json!({"chat_id":7,"topic_id":2,"closed":false}),
    )
    .await
    .unwrap();
    let p = m.request("messages.editForumTopic");
    assert_eq!(p["closed"], false);
    assert!(p.get("title").is_none());
    assert!(p.get("icon_emoji_id").is_none());
    assert!(p.get("peer").is_some());
    assert!(p.get("channel").is_none());
}

#[tokio::test]
async fn pin_topic_supports_unpin() {
    let m = Mock::new().forum();
    run(
        &m,
        "pin_forum_topic",
        json!({"chat_id":7,"topic_id":2,"pinned":false}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.request("messages.updatePinnedForumTopic")["pinned"],
        false
    );
}

#[tokio::test]
async fn pinned_topics_keep_requested_order() {
    let m = Mock::new().forum();
    run(
        &m,
        "reorder_pinned_topics",
        json!({"chat_id":7,"topic_ids":[5,2,9]}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.request("messages.reorderPinnedForumTopics")["order"],
        json!([5, 2, 9])
    );
}

#[tokio::test]
async fn disable_forum_supplies_new_layer_sidebar_field() {
    let m = Mock::new().forum();
    run(&m, "disable_forum_topics", json!({"chat_id":7}))
        .await
        .unwrap();
    let p = m.request("channels.toggleForum");
    assert_eq!(p["enabled"], false);
    assert_eq!(p["tabs"], false);
    assert_eq!(p["sidebar"], false);
}

#[tokio::test]
async fn non_forum_refuses_topic_mutation() {
    let m = Mock::new().respond(
        "channels.getChannels",
        json!({"chats":[{"megagroup":true,"forum":false}]}),
    );
    assert!(
        run(&m, "delete_forum_topic", json!({"chat_id":7,"topic_id":2}))
            .await
            .is_err()
    );
    assert_eq!(m.count("messages.deleteTopicHistory"), 0);
}

#[tokio::test]
async fn topic_list_caps_page_and_reports_continuation() {
    let m = Mock::new().forum().respond(
        "messages.getForumTopics",
        json!({"topics":[{"id":3},{"id":4}]}),
    );
    let r = run(&m, "list_topics", json!({"chat_id":7,"limit":2}))
        .await
        .unwrap();
    assert_eq!(r["pagination"]["next_offset_topic"], 4);
    assert_eq!(r["pagination"]["has_more"], true);
    assert_eq!(m.request("messages.getForumTopics")["limit"], 2);
}
