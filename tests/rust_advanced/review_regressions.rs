use super::common::*;
use std::sync::Mutex;
async fn call(api: &dyn Telegram, name: &str, args: Value) -> Result<Value> {
    advanced::call(api, name, &args).await.unwrap()
}

#[tokio::test]
async fn poll_projection_never_accepts_another_private_chats_global_message_id() {
    let mut payload = message(poll());
    payload["messages"][0]["peer_id"] = json!({"_":"peerUser","user_id":"99"});
    let mut api = Mock::new().respond("messages.getMessages", payload);
    api.peer = json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"});
    assert!(
        run(
            &api,
            "get_poll_results",
            json!({"chat_id":42,"message_id":20})
        )
        .await
        .is_err()
    );
    assert_eq!(api.count("messages.getMessages"), 1);
    assert_eq!(api.count("messages.sendVote"), 0);
}

#[tokio::test]
async fn saved_messages_lookup_resolves_self_and_rejects_foreign_peers() {
    for (returned, allowed) in [(42, true), (99, false)] {
        let mut payload = message(poll());
        payload["messages"][0]["peer_id"] = json!({"_":"peerUser","user_id":returned.to_string()});
        let mut api = Mock::new()
            .respond("users.getUsers", json!([{"id":"42"}]))
            .respond("messages.getMessages", payload);
        api.peer = json!({"_":"inputPeerSelf"});
        assert_eq!(
            run(
                &api,
                "get_poll_results",
                json!({"chat_id":"me","message_id":20})
            )
            .await
            .is_ok(),
            allowed
        );
        assert_eq!(api.count("users.getUsers"), 1);
    }
}

#[tokio::test]
async fn quick_reply_send_success_survives_followup_catalogue_failure() {
    let api = Mock::new()
        .respond("messages.getQuickReplies", json!({"quick_replies":[]}))
        .respond("messages.getQuickReplies", json!({"$error":true}));
    let result = run(
        &api,
        "add_quick_reply",
        json!({"shortcut":"hello","text":"saved once"}),
    )
    .await
    .unwrap();
    assert_eq!(result["created"], true);
    assert!(
        result["warning"]
            .as_str()
            .unwrap()
            .contains("Do not repeat")
    );
    assert_eq!(api.count("messages.sendMessage"), 1);
}

#[tokio::test]
async fn folder_creation_refuses_an_id_claimed_during_preparation() {
    let api = Mock::new()
        .respond("messages.getDialogFilters", json!({"filters":[]}))
        .respond("messages.getDialogFilters", json!({"filters":[{"id":2}]}));
    assert!(
        run(&api, "create_folder", json!({"title":"Work"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("occupied")
    );
    assert_eq!(api.count("messages.updateDialogFilter"), 0);
}

#[tokio::test]
async fn folder_reordering_round_trips_snapshot_default_folder_zero() {
    let api=Mock::new().respond("messages.getDialogFilters",json!({"filters":[{"_":"dialogFilterDefault"},{"_":"dialogFilter","id":2},{"_":"dialogFilter","id":3}]}));
    run(&api, "reorder_folders", json!({"folder_ids":[2,0,3]}))
        .await
        .unwrap();
    assert_eq!(
        api.request("messages.updateDialogFiltersOrder")["order"],
        json!([2, 0, 3])
    );
}

#[tokio::test]
async fn folder_membership_removal_excludes_implicitly_included_peers() {
    let api = Mock::new().respond("messages.getDialogFilters", folder(json!([]), json!([])));
    let result = run(
        &api,
        "remove_chat_from_folder",
        json!({"folder_id":2,"chat_id":7}),
    )
    .await
    .unwrap();
    assert_eq!(result["changed"], true);
    assert_eq!(
        api.request("messages.updateDialogFilter")["filter"]["exclude_peers"][0]["channel_id"],
        "7"
    );
}

#[tokio::test]
async fn folder_membership_add_removes_an_existing_exclusion() {
    let mut definition = folder(json!([]), json!([]));
    definition["filters"][0]["exclude_peers"] = json!([Mock::new().peer]);
    let api = Mock::new().respond("messages.getDialogFilters", definition);
    run(
        &api,
        "add_chat_to_folder",
        json!({"folder_id":2,"chat_id":7}),
    )
    .await
    .unwrap();
    let written = api.request("messages.updateDialogFilter");
    assert_eq!(written["filter"]["exclude_peers"], json!([]));
    assert_eq!(
        written["filter"]["include_peers"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn checklist_removal_refuses_a_concurrent_append_observed_before_write() {
    let todo = json!({"_":"todoList","title":{"_":"textWithEntities","text":"Tasks","entities":[]},"others_can_append":true,"list":[{"_":"todoItem","id":1,"title":{"_":"textWithEntities","text":"One","entities":[]}},{"_":"todoItem","id":2,"title":{"_":"textWithEntities","text":"Two","entities":[]}}]});
    let first = json!({"messages":[{"id":20,"media":{"_":"messageMediaToDo","todo":todo}}]});
    let mut changed = first.clone();
    changed["messages"][0]["media"]["todo"]["list"].as_array_mut().unwrap().push(json!({"_":"todoItem","id":3,"title":{"_":"textWithEntities","text":"Concurrent","entities":[]}}));
    let api = Mock::new()
        .respond("channels.getMessages", first)
        .respond("channels.getMessages", changed);
    assert!(
        run(
            &api,
            "remove_checklist_tasks",
            json!({"chat_id":7,"message_id":20,"task_ids":[1]})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("changed")
    );
    assert_eq!(api.count("messages.editMessage"), 0);
}

#[tokio::test]
async fn forum_topic_short_pages_keep_complete_activity_cursor() {
    let api = Mock::new().forum().respond(
        "messages.getForumTopics",
        json!({"count":20,"topics":[{"id":3,"date":100,"top_message":90}],
        "messages":[{"id":90,"date":999,"peer_id":{"_":"peerChannel","channel_id":"7"}}]}),
    );
    let result = run(
        &api,
        "list_topics",
        json!({"chat_id":7,"limit":200,"offset_topic":2,"offset_id":80,"offset_date":900}),
    )
    .await
    .unwrap();
    assert_eq!(api.request("messages.getForumTopics")["offset_id"], 80);
    assert_eq!(api.request("messages.getForumTopics")["offset_date"], 900);
    assert_eq!(result["pagination"]["has_more"], true);
    assert_eq!(
        result["pagination"]["next_cursor"],
        json!({"offset_topic":3,"offset_id":90,"offset_date":999})
    );
}

#[tokio::test]
async fn forum_topic_creation_order_uses_topic_date_and_empty_page_finishes() {
    let api=Mock::new().forum().respond("messages.getForumTopics",json!({"order_by_create_date":true,"count":5,"topics":[{"id":3,"date":100,"top_message":90}],"messages":[{"id":90,"date":999,"peer_id":{"_":"peerChannel","channel_id":"7"}}]}));
    let result = run(&api, "list_topics", json!({"chat_id":7}))
        .await
        .unwrap();
    assert_eq!(result["pagination"]["next_offset_date"], 100);
    let api = Mock::new()
        .forum()
        .respond("messages.getForumTopics", json!({"count":5,"topics":[]}));
    let result = run(
        &api,
        "list_topics",
        json!({"chat_id":7,"offset_topic":3,"offset_id":90,"offset_date":100}),
    )
    .await
    .unwrap();
    assert_eq!(result["pagination"]["has_more"], false);
    assert!(result["pagination"]["next_cursor"].is_null());
}

fn set_documents() -> Value {
    json!({"set":{"count":1},"documents":[{"_":"document","id":"123","access_hash":"456","file_reference":[9,8,7]}]})
}
#[tokio::test]
async fn inspected_sticker_references_survive_public_redaction_for_all_mutations() {
    for (tool, method, extra) in [
        (
            "remove_sticker_from_set",
            "stickers.removeStickerFromSet",
            json!({}),
        ),
        (
            "move_sticker_in_set",
            "stickers.changeStickerPosition",
            json!({"position":0}),
        ),
        (
            "add_sticker_to_set",
            "stickers.addStickerToSet",
            json!({"short_name":"target","emoji":"🦆"}),
        ),
    ] {
        let api = Mock::new()
            .cached(&format!("sticker-review-{tool}"))
            .respond("messages.getStickerSet", set_documents());
        run(&api, "inspect_sticker_set", json!({"short_name":"source"}))
            .await
            .unwrap();
        let mut args = json!({"document_id":"123"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        run(&api, tool, args).await.unwrap();
        let written = api.request(method);
        let doc = if tool == "add_sticker_to_set" {
            &written["sticker"]["document"]
        } else {
            &written["sticker"]
        };
        assert_eq!(doc["access_hash"], "456");
        assert_eq!(doc["file_reference"], json!([9, 8, 7]));
    }
}

#[tokio::test]
async fn sticker_references_are_scoped_to_the_authenticated_session() {
    let first = Mock::new()
        .cached("sticker-review-first")
        .respond("messages.getStickerSet", set_documents());
    run(
        &first,
        "inspect_sticker_set",
        json!({"short_name":"source"}),
    )
    .await
    .unwrap();
    let second = Mock::new().cached("sticker-review-other");
    assert!(
        run(
            &second,
            "remove_sticker_from_set",
            json!({"document_id":123})
        )
        .await
        .is_err()
    );
    assert_eq!(second.count("stickers.removeStickerFromSet"), 0);
    let second = Mock::new().respond("messages.getStickerSet", set_documents());
    run(
        &second,
        "remove_sticker_from_set",
        json!({"document_id":123,"short_name":"source"}),
    )
    .await
    .unwrap();
    assert_eq!(
        second.request("stickers.removeStickerFromSet")["sticker"]["file_reference"],
        json!([9, 8, 7])
    );
}

struct StoryApi(Mutex<Option<std::path::PathBuf>>);
#[async_trait]
impl Telegram for StoryApi {
    async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
        telegram_mcp::tl::Schema::global().encode_call(method, &args)?;
        assert_eq!(method, "stories.sendStory");
        Ok(json!({}))
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        Ok(json!({"_":"inputPeerSelf"}))
    }
    async fn upload(&self, path: &std::path::Path) -> Result<Value> {
        *self.0.lock().unwrap() = Some(path.to_owned());
        Ok(json!({"_":"inputFile","id":"42","parts":1,"name":"photo.jpg","md5_checksum":""}))
    }
}
#[tokio::test]
async fn story_upload_uses_the_same_resolved_absolute_path_as_metadata_inspection() {
    let state = telegram_mcp::state::State::global().unwrap();
    let root = &state.file_roots()[0];
    let temp = tempfile::tempdir_in(root).unwrap();
    let file = temp.path().join("photo.jpg");
    std::fs::write(&file, b"synthetic image upload bytes").unwrap();
    let relative = file.strip_prefix(root).unwrap().to_str().unwrap();
    let api = StoryApi(Mutex::new(None));
    call(
        &api,
        "post_story",
        json!({"file_path":relative,"privacy":"contacts"}),
    )
    .await
    .unwrap();
    assert_eq!(
        api.0.lock().unwrap().as_ref().unwrap(),
        &file.canonicalize().unwrap()
    );
}
