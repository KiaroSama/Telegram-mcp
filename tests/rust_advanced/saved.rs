use super::common::*;

#[tokio::test]
async fn last_allowed_delete_round_can_still_finish() {
    let mut m = Mock::new().forum();
    for _ in 0..49 {
        m = m.respond("messages.deleteTopicHistory", json!({"offset":1}));
    }
    m = m.respond("messages.deleteTopicHistory", json!({"offset":0}));
    let r = run(&m, "delete_forum_topic", json!({"chat_id":7,"topic_id":12}))
        .await
        .unwrap();
    assert_eq!(r["deleted"], true);
    assert_eq!(r["rounds"], 50);
}

#[tokio::test]
async fn hidden_is_only_valid_for_general() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "edit_forum_topic",
            json!({"chat_id":7,"topic_id":2,"hidden":true})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn removing_chat_updates_included_and_pinned_lists() {
    let p = Mock::new().peer;
    let m = Mock::new()
        .respond("messages.getDialogFilters", folder(json!([p]), json!([p])))
        .respond("messages.getDialogFilters", folder(json!([p]), json!([p])));
    run(
        &m,
        "remove_chat_from_folder",
        json!({"folder_id":2,"chat_id":7}),
    )
    .await
    .unwrap();
    let f = &m.request("messages.updateDialogFilter")["filter"];
    assert_eq!(f["include_peers"], json!([]));
    assert_eq!(f["pinned_peers"], json!([]));
}

#[tokio::test]
async fn translate_refuses_both_modes() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "translate",
            json!({"to_language":"fa","text":"hello","chat_id":7,"message_ids":[1]})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn translate_refuses_neither_mode() {
    let m = Mock::new();
    assert!(
        run(&m, "translate", json!({"to_language":"fa"}))
            .await
            .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn translation_message_mode_has_no_loose_text() {
    let m = Mock::new();
    run(
        &m,
        "translate",
        json!({"to_language":"fa","chat_id":7,"message_ids":20}),
    )
    .await
    .unwrap();
    let r = m.request("messages.translateText");
    assert_eq!(r["id"], json!([20]));
    assert!(r.get("text").is_none());
}

#[tokio::test]
async fn translation_loose_text_is_wrapped() {
    let m = Mock::new();
    run(&m, "translate", json!({"to_language":"fa","text":"hello"}))
        .await
        .unwrap();
    let r = m.request("messages.translateText");
    assert_eq!(r["text"][0]["text"], "hello");
    assert!(r.get("peer").is_none());
    assert!(r.get("id").is_none());
}

#[tokio::test]
async fn mini_app_routes_short_name_to_app_request() {
    let m = Mock::new();
    let mut m = m;
    m.peer = json!({"_":"inputPeerUser","user_id":"4","access_hash":"9"});
    run(&m, "open_mini_app", json!({"bot":4,"short_name":"shop"}))
        .await
        .unwrap();
    let r = m.request("messages.requestAppWebView");
    assert_eq!(r["app"]["_"], "inputBotAppShortName");
    assert_eq!(r["app"]["short_name"], "shop");
    assert!(r.get("bot").is_none());
}

#[tokio::test]
async fn mini_app_warns_that_launch_url_is_credential() {
    let mut m = Mock::new().respond(
        "messages.requestMainWebView",
        json!({"url":"https://example.test/#tgWebAppData=secret"}),
    );
    m.peer = json!({"_":"inputPeerUser","user_id":"4","access_hash":"9"});
    let r = run(&m, "open_mini_app", json!({"bot":4})).await.unwrap();
    assert_eq!(r["url"], "https://example.test/#tgWebAppData=secret");
    assert!(
        r["credential_warning"]
            .as_str()
            .unwrap()
            .contains("credential")
    );
}
fn quick_reply() -> Value {
    json!({"quick_replies":[{"shortcut_id":5,"shortcut":"hello","count":1}]})
}
fn quick_messages() -> Value {
    json!({"messages":[{"id":10,"message":"Hello"}]})
}
#[tokio::test]
async fn adding_new_shortcut_reports_creation() {
    let m = Mock::new()
        .respond("messages.getQuickReplies", json!({"quick_replies":[]}))
        .respond("messages.getQuickReplies", quick_reply());
    let r = run(
        &m,
        "add_quick_reply",
        json!({"shortcut":"/hello","text":"Hi"}),
    )
    .await
    .unwrap();
    assert_eq!(r["created"], true);
    let p = m.request("messages.sendMessage");
    assert_eq!(p["peer"]["_"], "inputPeerSelf");
    assert_eq!(p["quick_reply_shortcut"]["shortcut"], "hello");
}
#[tokio::test]
async fn appending_existing_shortcut_does_not_claim_creation() {
    let m = Mock::new()
        .respond("messages.getQuickReplies", quick_reply())
        .respond("messages.getQuickReplies", quick_reply());
    let r = run(
        &m,
        "add_quick_reply",
        json!({"shortcut":"hello","text":"Hi"}),
    )
    .await
    .unwrap();
    assert_eq!(r["created"], false);
}
#[tokio::test]
async fn quick_reply_edit_keeps_shortcut_and_message_identity() {
    let m = Mock::new().respond("messages.getQuickReplyMessages", quick_messages());
    run(
        &m,
        "edit_quick_reply",
        json!({"shortcut_id":5,"message_id":10,"message":"Corrected"}),
    )
    .await
    .unwrap();
    let p = m.request("messages.editMessage");
    assert_eq!(p["quick_reply_shortcut_id"], 5);
    assert_eq!(p["id"], 10);
    assert_eq!(p["message"], "Corrected");
    assert_eq!(m.count("messages.deleteQuickReplyMessages"), 0);
}
#[tokio::test]
async fn quick_reply_edit_refuses_message_outside_shortcut() {
    let m = Mock::new().respond("messages.getQuickReplyMessages", quick_messages());
    assert!(
        run(
            &m,
            "edit_quick_reply",
            json!({"shortcut_id":5,"message_id":99,"message":"Corrected"})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.editMessage"), 0);
}
#[tokio::test]
async fn omitted_quick_reply_ids_delete_the_shortcut() {
    let m = Mock::new().respond("messages.getQuickReplies", quick_reply());
    run(&m, "delete_quick_reply", json!({"shortcut_id":5}))
        .await
        .unwrap();
    assert_eq!(m.count("messages.deleteQuickReplyShortcut"), 1);
    assert_eq!(m.count("messages.deleteQuickReplyMessages"), 0);
}
#[tokio::test]
async fn specified_quick_reply_ids_keep_the_shortcut() {
    let m = Mock::new()
        .respond("messages.getQuickReplies", quick_reply())
        .respond("messages.getQuickReplyMessages", quick_messages());
    run(
        &m,
        "delete_quick_reply",
        json!({"shortcut_id":5,"message_ids":[10]}),
    )
    .await
    .unwrap();
    assert_eq!(m.count("messages.deleteQuickReplyShortcut"), 0);
    assert_eq!(
        m.request("messages.deleteQuickReplyMessages")["id"],
        json!([10])
    );
}
#[tokio::test]
async fn empty_quick_reply_ids_never_delete_the_whole_shortcut() {
    let m = Mock::new().respond("messages.getQuickReplies", quick_reply());
    assert!(
        run(
            &m,
            "delete_quick_reply",
            json!({"shortcut_id":5,"message_ids":[]})
        )
        .await
        .is_err()
    );
    assert_eq!(m.count("messages.deleteQuickReplyShortcut"), 0);
    assert_eq!(m.count("messages.deleteQuickReplyMessages"), 0);
}
#[tokio::test]
async fn saved_gif_removal_refreshes_real_file_reference() {
    let d = json!({"_":"document","id":"123","access_hash":"456","file_reference":[9,8]});
    let m = Mock::new().respond("messages.getSavedGifs", json!({"gifs":[d]}));
    run(&m, "unsave_gif", json!({"document_id":"123"}))
        .await
        .unwrap();
    let p = m.request("messages.saveGif");
    assert_eq!(p["id"]["file_reference"], json!([9, 8]));
    assert_eq!(p["unsave"], true);
}
#[tokio::test]
async fn unsaved_gif_is_not_reported_as_removed() {
    let m = Mock::new().respond("messages.getSavedGifs", json!({"gifs":[]}));
    assert!(
        run(&m, "unsave_gif", json!({"document_id":"123"}))
            .await
            .is_err()
    );
    assert_eq!(m.count("messages.saveGif"), 0);
}
#[tokio::test]
async fn gif_handle_from_other_account_is_refused() {
    let m = Mock::new();
    let handle = format!(
        "gif:other:{}:123:result",
        chrono::Utc::now().timestamp() + 3600
    );
    assert!(
        run(
            &m,
            "send_gif",
            json!({"account":"mine","chat_id":7,"gif_id":handle})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn expired_gif_handle_is_refused() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "send_gif",
            json!({"chat_id":7,"gif_id":"gif:default:1:123:result"})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn gif_handle_preserves_colons_inside_result_id() {
    let m = Mock::new();
    let handle = format!(
        "gif:default:{}:123:a:b:c",
        chrono::Utc::now().timestamp() + 3600
    );
    run(
        &m,
        "send_gif",
        json!({"chat_id":7,"gif_id":handle,"topic_id":3}),
    )
    .await
    .unwrap();
    let p = m.request("messages.sendInlineBotResult");
    assert_eq!(p["id"], "a:b:c");
    assert_eq!(p["query_id"], "123");
    assert_eq!(p["reply_to"]["reply_to_msg_id"], 3);
}
#[tokio::test]
async fn adding_sticker_checks_expected_set_count() {
    let m = Mock::new().respond("messages.getStickerSet", json!({"set":{"count":4}}));
    assert!(run(&m,"add_sticker_to_set",json!({"short_name":"test","document_id":"123","access_hash":"456","emoji":"x","expected_count":3})).await.is_err());
    assert_eq!(m.count("stickers.addStickerToSet"), 0);
}
#[tokio::test]
async fn quick_reply_mentions_resolve_input_user_access_hash() {
    let mut m = Mock::new();
    m.peer = json!({"_":"inputPeerUser","user_id":"42","access_hash":"99"});
    run(&m,"add_quick_reply",json!({"shortcut":"hello","text":"Ada","entities":[{"type":"mention_name","offset":0,"length":3,"user_id":"42"}]})).await.unwrap();
    assert_eq!(
        m.request("messages.sendMessage")["entities"][0],
        json!({"_":"inputMessageEntityMentionName","offset":0,"length":3,"user_id":{"_":"inputUser","user_id":"42","access_hash":"99"}})
    );
}
#[tokio::test]
async fn combined_sticker_catalogue_includes_custom_emoji_sets() {
    let m = Mock::new()
        .respond("messages.getAllStickers", json!({"sets":[{"id":"1"}]}))
        .respond("messages.getEmojiStickers", json!({"sets":[{"id":"2"}]}));
    let r = run(&m, "get_sticker_sets", json!({"kind":"both"}))
        .await
        .unwrap();
    assert_eq!(r["sets"], json!([{"id":"1"},{"id":"2"}]));
    assert_eq!(m.count("messages.getAllStickers"), 1);
    assert_eq!(m.count("messages.getEmojiStickers"), 1);
}
