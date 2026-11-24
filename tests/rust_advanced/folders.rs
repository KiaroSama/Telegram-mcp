use super::common::*;

#[tokio::test]
async fn folder_edit_preserves_unrelated_flags() {
    let m = Mock::new()
        .respond("messages.getDialogFilters", folder(json!([]), json!([])))
        .respond("messages.getDialogFilters", folder(json!([]), json!([])));
    run(
        &m,
        "add_chat_to_folder",
        json!({"folder_id":2,"chat_id":7,"pinned":true}),
    )
    .await
    .unwrap();
    let f = &m.request("messages.updateDialogFilter")["filter"];
    assert_eq!(f["color"], 3);
    assert_eq!(f["groups"], true);
    assert_eq!(f["exclude_archived"], true);
    assert_eq!(f["include_peers"].as_array().unwrap().len(), 1);
    assert_eq!(f["pinned_peers"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn folder_add_is_idempotent() {
    let p = Mock::new().peer;
    let m = Mock::new().respond("messages.getDialogFilters", folder(json!([p]), json!([])));
    let r = run(&m, "add_chat_to_folder", json!({"folder_id":2,"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(r["changed"], false);
    assert_eq!(m.count("messages.updateDialogFilter"), 0);
}

#[tokio::test]
async fn folder_delete_never_deletes_chats() {
    let m = Mock::new().respond("messages.getDialogFilters", folder(json!([]), json!([])));
    run(&m, "delete_folder", json!({"folder_id":2}))
        .await
        .unwrap();
    let p = m.request("messages.updateDialogFilter");
    assert_eq!(p, json!({"id":2}));
    assert_eq!(m.count("messages.deleteHistory"), 0);
}

#[tokio::test]
async fn system_folder_is_not_deleted() {
    let m = Mock::new();
    assert!(
        run(&m, "delete_folder", json!({"folder_id":1}))
            .await
            .is_err()
    );
    assert_eq!(m.count("messages.updateDialogFilter"), 0);
}

#[tokio::test]
async fn folder_reorder_requires_all_existing_folders() {
    let m = Mock::new().respond(
        "messages.getDialogFilters",
        json!({"filters":[{"id":2},{"id":3}]}),
    );
    assert!(
        run(&m, "reorder_folders", json!({"folder_ids":[2]}))
            .await
            .is_err()
    );
    assert_eq!(m.count("messages.updateDialogFiltersOrder"), 0);
}

#[tokio::test]
async fn shared_folder_preview_does_not_join() {
    let m = Mock::new();
    run(
        &m,
        "get_chatlist_invite",
        json!({"link":"https://t.me/addlist/abc_DEF-12"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.request("chatlists.checkChatlistInvite")["slug"],
        "abc_DEF-12"
    );
    assert_eq!(m.all().len(), 1);
}

#[tokio::test]
async fn shared_folder_rejects_lookalike_host() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "get_chatlist_invite",
            json!({"link":"https://t.me.evil.test/addlist/abc"})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}

#[tokio::test]
async fn shared_folder_rejects_url_credentials() {
    let m = Mock::new();
    assert!(
        run(
            &m,
            "get_chatlist_invite",
            json!({"link":"https://user@t.me/addlist/abc"})
        )
        .await
        .is_err()
    );
    assert!(m.all().is_empty());
}
#[tokio::test]
async fn saved_messages_is_stored_as_concrete_peer() {
    let mut m = Mock::new()
        .respond("messages.getDialogFilters", folder(json!([]), json!([])))
        .respond("messages.getDialogFilters", folder(json!([]), json!([])))
        .respond("users.getUsers", json!([{"id":"4242","access_hash":"99"}]));
    m.peer = json!({"_":"inputPeerSelf"});
    run(
        &m,
        "add_chat_to_folder",
        json!({"folder_id":2,"chat_id":"me"}),
    )
    .await
    .unwrap();
    assert_eq!(
        m.request("messages.updateDialogFilter")["filter"]["include_peers"][0],
        json!({"_":"inputPeerUser","user_id":"4242","access_hash":"99"})
    );
}
#[tokio::test]
async fn stored_self_peer_does_not_break_other_folder_members() {
    let m = Mock::new()
        .respond(
            "messages.getDialogFilters",
            folder(json!([{"_":"inputPeerSelf"}]), json!([])),
        )
        .respond(
            "messages.getDialogFilters",
            folder(json!([{"_":"inputPeerSelf"}]), json!([])),
        )
        .respond("users.getUsers", json!([{"id":"4242","access_hash":"99"}]));
    run(&m, "add_chat_to_folder", json!({"folder_id":2,"chat_id":7}))
        .await
        .unwrap();
    let entries = m.request("messages.updateDialogFilter")["filter"]["include_peers"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|p| p["user_id"] == "4242"));
    assert!(entries.iter().any(|p| p["channel_id"] == "7"));
}
struct FolderStore(std::sync::Mutex<Value>);
#[async_trait]
impl Telegram for FolderStore {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(method, &params)?;
        match method {
            "messages.getDialogFilters" => {
                let snapshot = self.0.lock().unwrap().clone();
                tokio::task::yield_now().await;
                Ok(snapshot)
            }
            "messages.updateDialogFilter" => {
                self.0.lock().unwrap()["filters"][0] = params["filter"].clone();
                Ok(json!(true))
            }
            _ => bail!("Unexpected request"),
        }
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        Ok(
            json!({"_":"inputPeerChannel","channel_id":input.as_i64().unwrap().to_string(),"access_hash":"99"}),
        )
    }
}
#[tokio::test]
async fn concurrent_folder_edits_do_not_lose_each_others_chat() {
    let store = FolderStore(std::sync::Mutex::new(folder(json!([]), json!([]))));
    let one = json!({"folder_id":2,"chat_id":10});
    let two = json!({"folder_id":2,"chat_id":20});
    let (a, b) = tokio::join!(
        advanced::call(&store, "add_chat_to_folder", &one),
        advanced::call(&store, "add_chat_to_folder", &two)
    );
    a.unwrap().unwrap();
    b.unwrap().unwrap();
    let stored = store.0.lock().unwrap();
    let peers = stored["filters"][0]["include_peers"].as_array().unwrap();
    assert_eq!(peers.len(), 2);
    assert!(peers.iter().any(|p| p["channel_id"] == "10"));
    assert!(peers.iter().any(|p| p["channel_id"] == "20"));
}
