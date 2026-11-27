use super::support::Api;
use serde_json::{Value, json};
use telegram_mcp::tools;

fn forum(steps: Vec<(&'static str, Value)>) -> Api {
    Api::new(steps).identity(
        json!("forum"),
        json!({"_":"inputPeerChannel","channel_id":"7","access_hash":"11"}),
    )
}

fn peers(enabled: bool) -> Value {
    json!({"chats":[{"_":"channel","id":"7","forum":enabled}]})
}

fn row(id: i64, thread: Option<i64>) -> Value {
    let reply = thread.map(|id| json!({"forum_topic":true,"reply_to_msg_id":id}));
    json!({"_":"message","id":id,"peer_id":{"_":"peerChannel","channel_id":"7"},"message":"needle","reply_to":reply})
}

#[tokio::test]
async fn general_pagination_counts_only_general_messages_after_filtering() {
    let api = forum(vec![
        ("channels.getChannels", peers(true)),
        (
            "messages.getHistory",
            json!({"messages":[row(9, Some(5)),row(8,None)]}),
        ),
        (
            "messages.getHistory",
            json!({"messages":[row(7, Some(5)),row(6,None),row(5,None)]}),
        ),
    ]);
    let result = tools::call(
        &api,
        "get_messages",
        &json!({"chat_id":"forum","topic_id":"1","page":2,"page_size":1}),
    )
    .await
    .unwrap();
    assert_eq!(result["messages"][0]["id"], 6);
    assert_eq!(result["has_more"], true);
    assert_eq!(result["scan_limit_reached"], false);
    let calls = api.calls();
    assert_eq!(calls[1].1["add_offset"], 0);
    assert_eq!(calls[2].1["offset_id"], 8);
}

#[tokio::test]
async fn general_list_and_search_do_not_pass_a_fake_thread_root() {
    for (name, args) in [
        (
            "list_messages",
            json!({"chat_id":"forum","topic_id":1,"search_query":"needle"}),
        ),
        (
            "search_messages",
            json!({"chat_id":"forum","topic_id":1,"query":"needle"}),
        ),
    ] {
        let api = forum(vec![
            ("channels.getChannels", peers(true)),
            (
                "messages.search",
                json!({"messages":[row(11, Some(9)),row(8,None)]}),
            ),
            ("messages.search", json!({"messages":[]})),
        ]);
        let value = tools::call(&api, name, &args).await.unwrap();
        assert_eq!(value["messages"].as_array().unwrap().len(), 1);
        assert_eq!(value["messages"][0]["id"], 8);
        assert!(api.calls()[1].1.get("top_msg_id").is_none());
    }
}

#[tokio::test]
async fn general_history_validates_the_forum_and_filters_the_chat_history() {
    let api = forum(vec![
        ("channels.getChannels", peers(true)),
        (
            "messages.getHistory",
            json!({"messages":[row(9, Some(5)),row(8,None),row(7,None),row(6,None)]}),
        ),
    ]);
    let value = tools::call(
        &api,
        "get_history",
        &json!({"chat_id":"forum","topic_id":1,"limit":2}),
    )
    .await
    .unwrap();
    assert_eq!(
        value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![8, 7]
    );
    assert_eq!(value["has_more"], true);
    assert_eq!(value["scan_limit_reached"], false);
    assert_eq!(value["metadata"]["effective_limit"], 2);
    assert!(api.calls()[1].1.get("msg_id").is_none());
    assert!(api.calls()[1].1.get("top_msg_id").is_none());
}

#[tokio::test]
async fn general_refuses_non_forum_and_reports_a_stalled_cursor() {
    let api = forum(vec![("channels.getChannels", peers(false))]);
    let args = json!({"chat_id":"forum","topic_id":1});
    assert!(
        tools::call(&api, "list_messages", &args)
            .await
            .unwrap_err()
            .to_string()
            .contains("forum")
    );
    let api = forum(vec![
        ("channels.getChannels", peers(true)),
        ("messages.getHistory", json!({"messages":[row(3,Some(2))]})),
        ("messages.getHistory", json!({"messages":[row(3,Some(2))]})),
    ]);
    let result = tools::call(&api, "list_messages", &args).await.unwrap();
    assert_eq!(result["scan_limit_reached"], true);
    assert_eq!(result["messages"], json!([]));
}

#[tokio::test]
async fn layer234_model_listing_and_bot_invite_send_exact_rpc_shapes() {
    let api = Api::new(vec![
        (
            "messages.getTranscriptionModels",
            json!([{"document":{"id":"9223372036854775807"},"languages":["en","fa"],"version":3}]),
        ),
        ("messages.inviteUsersToBot", json!(true)),
    ])
    .identity(
        json!("bot"),
        json!({"_":"inputPeerUser","user_id":"10","access_hash":"11"}),
    );
    let models = tools::call(&api, "list_transcription_models", &json!({"version":2}))
        .await
        .unwrap();
    assert_eq!(models[0]["document_id"], "9223372036854775807");
    assert_eq!(models[0]["languages"], json!(["en", "fa"]));
    assert_eq!(api.calls()[0].1, json!({"version":2}));
    let result = tools::call(
        &api,
        "invite_users_to_bot",
        &json!({"bot":"bot","user_ids":["user","user"]}),
    )
    .await
    .unwrap();
    assert_eq!(result, json!({"ok":true,"bot":"10","requested_count":1}));
    let calls = api.calls();
    assert_eq!(calls[1].1["bot"]["_"], "inputUser");
    assert_eq!(calls[1].1["users"][0]["_"], "inputUser");
}

#[tokio::test]
async fn layer234_ai_style_validates_tone_and_omits_style_when_cleared() {
    let api = Api::new(vec![
        ("aicompose.setChatStyle", json!(true)),
        ("aicompose.setChatStyle", json!(false)),
    ]);
    let result = tools::call(
        &api,
        "set_chat_ai_style",
        &json!({"chat_id":"peer","clear":true}),
    )
    .await
    .unwrap();
    assert_eq!(result["cleared"], true);
    assert!(api.calls()[0].1.get("style").is_none());
    let result = tools::call(
        &api,
        "set_chat_ai_style",
        &json!({"chat_id":"peer","proofread":true,"tone":{"kind":"slug","value":"brief"}}),
    )
    .await
    .unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(
        api.calls()[1].1["style"]["tone"],
        json!({"_":"inputAiComposeToneSlug","slug":"brief"})
    );
    assert!(
        tools::call(
            &api,
            "set_chat_ai_style",
            &json!({"chat_id":"peer","clear":true,"emojify":false})
        )
        .await
        .is_err()
    );
    assert_eq!(api.calls().len(), 2);
}
