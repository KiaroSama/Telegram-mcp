use super::support::*;
use serde_json::json;

#[tokio::test(start_paused = true)]
async fn cursor_hides_peer_credentials_and_expires_before_rpc() {
    let mut page = messages(vec![message(8, "hit")]);
    page["users"] = json!([{"_":"user","id":"7","access_hash":"985641032"}]);
    let api = Api::new(vec![("messages.searchGlobal", page)]);
    let args = json!({"query":"q","account":"example"});
    let result = run(&api, "search_global", args.clone()).await.unwrap();
    assert!(!result.to_string().contains("985641032"));
    assert!(api.resolved.lock().unwrap().is_empty());
    let mut next = args;
    next["cursor"] = result["next_cursor"].clone();
    for (k, v) in [
        ("page", json!(2)),
        ("query", json!("other")),
        ("account", json!("other")),
        ("cursor", json!(123)),
    ] {
        let mut forged = next.clone();
        forged[k] = v;
        assert!(run(&api, "search_global", forged).await.is_err());
    }
    tokio::time::advance(std::time::Duration::from_secs(86_400)).await;
    assert!(run(&api, "search_global", next).await.is_err());
    assert_eq!(api.calls().len(), 1);
}

#[tokio::test]
async fn three_advancing_empty_slices_return_resumable_partial() {
    let mut replies = Vec::new();
    for rate in 1..=3 {
        replies.push(("messages.searchGlobal",json!({"_":"messages.messagesSlice","messages":[],"users":[],"chats":[],"next_rate":rate})));
    }
    replies.push(("messages.searchGlobal", messages(vec![])));
    let api = Api::new(replies);
    let first = run(&api, "search_global", json!({"query":"q"}))
        .await
        .unwrap();
    assert_eq!(first["partial"], true);
    assert_eq!(first["exhausted"], false);
    assert_eq!(api.calls().len(), 3);
    let next = run(
        &api,
        "search_global",
        json!({"query":"q","cursor":first["next_cursor"]}),
    )
    .await
    .unwrap();
    assert_eq!(next["exhausted"], true);
    assert_eq!(api.calls()[3].1["offset_rate"], 3);
}

#[tokio::test]
async fn invalid_typing_duration_or_topic_never_resolves_a_peer() {
    for args in [
        json!({"chat_id":7,"seconds":0}),
        json!({"chat_id":7,"seconds":0.49}),
        json!({"chat_id":7,"seconds":46}),
        json!({"chat_id":7,"topic_id":0}),
    ] {
        let api = Api::new(vec![]);
        assert!(run(&api, "send_typing", args).await.is_err());
        assert!(api.resolved.lock().unwrap().is_empty());
        assert!(api.calls().is_empty());
    }
}

#[tokio::test]
async fn rich_table_exposes_quote_coordinates_and_safe_links() {
    let mut m = message(4, "");
    m["rich_message"] = json!({"_":"richMessage","blocks":[{"_":"pageBlockTable","rows":[{"cells":[{"text":{"_":"textEmail","email":"person@example.org","text":{"_":"textPlain","text":"mail"}}}]}]}]});
    m["reply_to"] = json!({"quote_text":"part","quote_offset":2});
    let api = Api::new(vec![("messages.getRichMessage", messages(vec![m]))]);
    let r = run(
        &api,
        "read_rich_message",
        json!({"chat_id":7,"message_id":4}),
    )
    .await
    .unwrap();
    assert_eq!(r["quote_sources"][0]["column"], 0);
    assert_eq!(r["links"][0], "mailto:person@example.org");
    assert_eq!(r["reply_quote"]["offset"], 2);
}

#[tokio::test]
async fn selector_only_callback_and_wrong_peer_requests() {
    for selector in [json!({"button_text":"go"}), json!({"button_index":0})] {
        let mut m = message(5, "");
        m["reply_markup"] = json!({"_":"replyInlineMarkup","rows":[{"buttons":[{"text":"go","type":{"_":"inlineButtonTypeCallback","data":[1]}}]}]});
        let api = Api::new(vec![
            ("messages.getMessages", messages(vec![m])),
            ("messages.getBotCallbackAnswer", json!(true)),
        ]);
        let mut args = selector;
        args["chat_id"] = json!(7);
        args["message_id"] = json!(5);
        run(&api, "press_inline_button", args).await.unwrap();
        assert_eq!(api.calls().len(), 2);
    }
    let m = keyboard(
        json!({"_":"buttonTypeRequestPeer","peer_type":{"_":"requestPeerTypeBroadcast"},"max_quantity":1}),
    );
    let api = Api::new(vec![("messages.getHistory", messages(vec![m]))]);
    let mut args = keyboard_args();
    args["peers"] = json!([7]);
    assert!(run(&api, "answer_reply_button", args).await.is_err());
    assert_eq!(api.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn typing_cancellation_clears_the_indicator_and_releases_the_slot() {
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use telegram_mcp::backend::Telegram;
    #[derive(Clone, Default)]
    struct Recorder(Arc<std::sync::Mutex<Vec<String>>>);
    #[async_trait]
    impl Telegram for Recorder {
        async fn resolve_peer(&self, _: &Value) -> anyhow::Result<Value> {
            Ok(json!({"_":"inputPeerChat","chat_id":"81"}))
        }
        async fn invoke(&self, _: &str, p: Value) -> anyhow::Result<Value> {
            self.0
                .lock()
                .unwrap()
                .push(p["action"]["_"].as_str().unwrap().into());
            Ok(json!(true))
        }
        fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
            Some(Arc::new(self.clone()))
        }
    }
    let api = Recorder::default();
    let copy = api.clone();
    let task = tokio::spawn(async move {
        telegram_mcp::tools::messaging::call(
            &copy,
            "send_typing",
            &json!({"chat_id":81,"seconds":45}),
        )
        .await
    });
    tokio::task::yield_now().await;
    assert_eq!(
        api.0.lock().unwrap().as_slice(),
        ["sendMessageTypingAction"]
    );
    let result = telegram_mcp::tools::messaging::call(
        &api,
        "send_typing",
        &json!({"chat_id":81,"seconds":0.5}),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::task::yield_now().await;
    assert_eq!(
        api.0.lock().unwrap().as_slice(),
        ["sendMessageTypingAction", "sendMessageCancelAction"]
    );
    assert!(
        telegram_mcp::tools::messaging::call(
            &api,
            "send_typing",
            &json!({"chat_id":81,"seconds":0.5})
        )
        .await
        .unwrap()
        .is_ok()
    );
}

#[tokio::test]
async fn inline_listing_uses_limit_and_active_reply_keyboard_skips_inapplicable_messages() {
    let mut inline = message(2, "");
    inline["reply_markup"] = json!({"_":"replyInlineMarkup","rows":[{"buttons":[{"text":"go","type":{"_":"inlineButtonTypeCallback","data":[1]}}]}]});
    let mut outgoing = keyboard(json!({"_":"buttonTypeDefault"}));
    outgoing["out"] = json!(true);
    let mut selective = keyboard(json!({"_":"buttonTypeDefault"}));
    selective["reply_markup"]["selective"] = json!(true);
    let api = Api::new(vec![(
        "messages.getHistory",
        messages(vec![outgoing, selective, inline]),
    )]);
    let result = run(&api, "list_inline_buttons", json!({"chat_id":7,"limit":3}))
        .await
        .unwrap();
    assert_eq!(api.calls()[0].1["limit"], 3);
    assert_eq!(result["message_id"], 2);
    assert_eq!(result["buttons"][0]["column"], 0);
    assert_eq!(result["buttons"][0]["pressable"], true);
    assert!(result["active_reply_keyboard"].is_null());
}
