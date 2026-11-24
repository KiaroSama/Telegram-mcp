use super::*;

fn snapshot() -> Value {
    json!({"format":"telegram-mcp/unread-export-v1","account":"work","telegram_user_id":"9","chats":{"42":{
        "chat_id":"42","unread_count_at_export":3,"snapshot_top_message":3,
        "messages_exported":1,"messages":[{"id":1,"text":"previous user's message"}],
        "next_after_id":1,"complete":false,"incomplete_reason":"message_limit"
    }}})
}

fn completed_snapshot() -> Value {
    let mut value = snapshot();
    let chat = &mut value["chats"]["42"];
    chat["unread_count_at_export"] = json!(1);
    chat["snapshot_top_message"] = json!(1);
    chat["complete"] = json!(true);
    chat["incomplete_reason"] = Value::Null;
    value
}

#[tokio::test]
async fn reused_account_label_cannot_resume_another_telegram_users_snapshot() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let before = serde_json::to_vec_pretty(&completed_snapshot()).unwrap();
    std::fs::write(&output, &before).unwrap();
    let api = Api::default();
    api.reply(
        "users.getUsers",
        json!([{"_":"user","id":"10","self":true}]),
    );
    let error = export_unread_with_state(
        &api,
        &state,
        &json!({"chat_ids":[42],"output_path":output,"account":"work"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("Telegram user"));
    assert_eq!(std::fs::read(&output).unwrap(), before);
    assert_eq!(
        api.calls()
            .iter()
            .map(|call| call.0.as_str())
            .collect::<Vec<_>>(),
        ["users.getUsers"]
    );
}

#[tokio::test]
async fn legacy_export_without_authenticated_user_identity_is_preserved_and_refused() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let mut previous = completed_snapshot();
    previous.as_object_mut().unwrap().remove("telegram_user_id");
    let before = serde_json::to_vec_pretty(&previous).unwrap();
    std::fs::write(&output, &before).unwrap();
    let api = Api::default();
    let error = export_unread_with_state(
        &api,
        &state,
        &json!({"chat_ids":[42],"output_path":output,"account":"work"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("identity"));
    assert_eq!(std::fs::read(&output).unwrap(), before);
    assert!(api.calls().iter().all(|call| call.0 == "users.getUsers"));
}

#[tokio::test]
async fn unread_exports_require_an_unambiguous_authenticated_self_response() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    for response in [
        json!([]),
        json!([{"_":"userEmpty","id":"9"}]),
        json!([{"_":"user","id":"9","self":false}]),
        json!([{"_":"user","id":"0","self":true}]),
        json!([{"_":"user","id":"9","self":true},{"_":"user","id":"10","self":true}]),
    ] {
        let api = Api::default();
        api.reply("users.getUsers", response);
        api.dialog(0, 9, 9);
        let error =
            export_unread_with_state(&api, &state, &json!({"chat_ids":[42],"output_path":output}))
                .await
                .unwrap_err();
        assert!(error.to_string().contains("identity"));
        assert!(!output.exists());
        assert_eq!(api.calls().len(), 1);
    }
}

#[tokio::test]
async fn pretty_encoding_budget_publishes_admitted_progress_and_resumes_the_same_snapshot() {
    const BOUND: usize = 32 * 1024 * 1024;
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let mut previous = snapshot();
    previous["chats"]["42"]["messages"][0]["marks"] = json!(vec![0; 350_000]);
    previous["chats"]["42"]["messages"][0]["text"] = json!("");
    let overhead = serde_json::to_vec_pretty(&previous).unwrap().len() + 1;
    previous["chats"]["42"]["messages"][0]["text"] =
        json!("x".repeat(BOUND - 1024 * 1024 - overhead));
    let mut before = serde_json::to_vec_pretty(&previous).unwrap();
    before.push(b'\n');
    assert_eq!(before.len(), BOUND - 1024 * 1024);
    assert!(serde_json::to_vec(&previous).unwrap().len() < BOUND - 3 * 1024 * 1024);
    std::fs::write(&output, &before).unwrap();
    let api = Api::default();
    api.dialog(3, 0, 3);
    let mut first = message(2, false);
    first["message"] = json!("a".repeat(256 * 1024));
    let mut second = message(3, false);
    second["message"] = json!("\n😀\"".repeat(130_000));
    api.reply("messages.getHistory", page(vec![second, first]));
    let args = json!({"chat_ids":[42],"output_path":output,"account":"work"});
    let partial = export_unread_with_state(&api, &state, &args).await.unwrap();
    assert_eq!(partial["messages_exported"], 1);
    assert_eq!(partial["complete"], false);
    let bytes = std::fs::read(&output).unwrap();
    assert!(bytes.len() <= BOUND);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let saved: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["telegram_user_id"], "9");
    assert_eq!(
        saved["chats"]["42"]["messages"].as_array().unwrap().len(),
        2
    );
    assert_eq!(saved["chats"]["42"]["next_after_id"], 2);
    assert_eq!(saved["chats"]["42"]["incomplete_reason"], "byte_budget");
    api.dialog(0, 3, 3);
    api.reply("messages.getHistory", page(vec![message(3, false)]));
    let resumed = export_unread_with_state(&api, &state, &args).await.unwrap();
    assert_eq!(resumed["messages_exported"], 1);
    assert_eq!(resumed["complete"], true);
    let final_bytes = std::fs::read(&output).unwrap();
    assert!(final_bytes.len() <= BOUND);
    let completed: Value = serde_json::from_slice(&final_bytes).unwrap();
    assert_eq!(
        completed["chats"]["42"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(completed["chats"]["42"]["snapshot_top_message"], 3);
    assert_eq!(completed["chats"]["42"]["next_after_id"], 3);
}
