mod local_support;
use anyhow::{Result, bail};
use async_trait::async_trait;
use local_support::fixture;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::Duration,
};
use telegram_mcp::{
    backend::Telegram,
    state::EventQuery,
    tl::Schema,
    tools::local::{export_unread_with_state, upstream_events::get_messages_after},
};
#[path = "rust_local_upstream/unread_review.rs"]
mod unread_review;

#[derive(Default)]
struct Api {
    calls: Mutex<Vec<(String, Value)>>,
    replies: Mutex<HashMap<String, VecDeque<Value>>>,
}
impl Api {
    fn reply(&self, method: &str, value: Value) {
        self.replies
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(value);
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
    fn dialog(&self, unread: i64, read: i64, top: i64) {
        self.reply("messages.getPeerDialogs",json!({"dialogs":[{"_":"dialog","peer":{"_":"peerUser","user_id":"42"},"unread_count":unread,"read_inbox_max_id":read,"top_message":top}]}));
    }
}
#[async_trait]
impl Telegram for Api {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        Schema::global().encode_call(method, &params)?;
        self.calls.lock().unwrap().push((method.into(), params));
        if method == "users.getUsers" && !self.replies.lock().unwrap().contains_key(method) {
            return Ok(json!([{"_":"user","id":"9","self":true}]));
        }
        self.replies
            .lock()
            .unwrap()
            .get_mut(method)
            .and_then(VecDeque::pop_front)
            .ok_or_else(|| anyhow::anyhow!("Unexpected RPC {method}"))
    }
    async fn resolve_peer(&self, value: &Value) -> Result<Value> {
        let id = value
            .as_i64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
        if id != Some(42) {
            bail!("Unexpected peer");
        }
        Ok(json!({"_":"inputPeerUser","user_id":"42","access_hash":"7"}))
    }
}
fn record(account: &str, chat: i64, id: i64) -> Value {
    json!({"account":account,"chat_id":chat,"chat_type":"private","message_id":id,"sender_id":chat,"sender_name":"نام","sender_bot":false,"out":false,"addressed":true,"service":false,"date":"2026-11-15T00:00:00Z"})
}
fn query() -> EventQuery {
    EventQuery {
        limit: 100,
        ..Default::default()
    }
}
fn message(id: i64, out: bool) -> Value {
    json!({"_":"message","id":id,"out":out,"peer_id":{"_":"peerUser","user_id":"42"},"from_id":{"_":"peerUser","user_id":"42"},"message":format!("message {id}"),"date":1,"media":{"_":"messageMediaEmpty"}})
}
fn page(messages: Vec<Value>) -> Value {
    json!({"messages":messages,"users":[{"_":"user","id":"42","first_name":"نام","last_name":"خانوادگی"}],"chats":[]})
}

#[tokio::test]
async fn metadata_stream_discards_message_bodies_and_keeps_outgoing_group_updates() {
    let (_dir, state) = fixture();
    let mut value = record("work", -42, 7);
    value["chat_type"] = json!("supergroup");
    value["out"] = json!(true);
    value["topic"] = json!(9);
    value["text"] = json!("private message text");
    value["media"] = json!({"access_hash":"secret"});
    value["_chat_username"] = json!("private_alias");
    state.record_message_event(value).unwrap();
    let result = state
        .wait_message_events(&query(), &|_| true)
        .await
        .unwrap();
    assert_eq!(result["events"][0]["out"], true);
    assert_eq!(result["events"][0]["topic"], 9);
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("private message text"));
    assert!(!encoded.contains("secret"));
    assert!(!encoded.contains("private_alias"));
}
#[tokio::test]
async fn event_log_filters_account_chat_and_topic_without_leaking_other_events() {
    let (_dir, state) = fixture();
    for (account, chat, id, topic) in [
        ("work", 42, 1, 9),
        ("personal", 42, 2, 9),
        ("work", 43, 3, 9),
        ("work", 42, 4, 10),
    ] {
        let mut value = record(account, chat, id);
        value["topic"] = json!(topic);
        state.record_message_event(value).unwrap();
    }
    let q = EventQuery {
        account: Some("WORK".into()),
        chat_id: Some("42".into()),
        ..query()
    };
    let result = state
        .wait_message_events(&q, &|event| event["topic"] == json!(9))
        .await
        .unwrap();
    assert_eq!(result["events"].as_array().unwrap().len(), 1);
    assert_eq!(result["events"][0]["message_id"], 1);
    assert_eq!(result["last_seq"], 4);
}
#[tokio::test]
async fn event_cursor_reports_dropped_history_and_resumes_batches_without_skips() {
    let (_dir, state) = fixture();
    for id in 1..=2001 {
        state.record_message_event(record("work", 42, id)).unwrap();
    }
    let q = EventQuery {
        limit: 1,
        ..query()
    };
    let first = state.wait_message_events(&q, &|_| true).await.unwrap();
    assert_eq!(first["gap"], true);
    assert_eq!(first["events"][0]["message_id"], 2);
    assert_eq!(first["last_seq"], 2);
    let next = state
        .wait_message_events(
            &EventQuery {
                after_seq: 2,
                boot: first["boot"].as_str().map(str::to_owned),
                limit: 1,
                ..query()
            },
            &|_| true,
        )
        .await
        .unwrap();
    assert_eq!(next["events"][0]["message_id"], 3);
    assert_eq!(next["gap"], false);
}
#[tokio::test]
async fn event_cursor_start_now_and_boot_reset_are_explicit() {
    let (_dir, state) = fixture();
    state.record_message_event(record("work", 42, 1)).unwrap();
    let now = state
        .wait_message_events(
            &EventQuery {
                after_seq: -1,
                ..query()
            },
            &|_| true,
        )
        .await
        .unwrap();
    assert_eq!(now["last_seq"], 1);
    assert_eq!(now["events"], json!([]));
    for q in [
        EventQuery {
            boot: Some("other process".into()),
            ..query()
        },
        EventQuery {
            after_seq: 99,
            ..query()
        },
    ] {
        let reset = state.wait_message_events(&q, &|_| true).await.unwrap();
        assert_eq!(reset["reset"], true);
        assert_eq!(reset["events"], json!([]));
    }
}
#[tokio::test]
async fn event_wait_observes_new_records_and_validates_untrusted_metadata() {
    let (_dir, state) = fixture();
    let waiting = state.clone();
    let task = tokio::spawn(async move {
        waiting
            .wait_message_events(
                &EventQuery {
                    timeout: Duration::from_secs(1),
                    ..query()
                },
                &|_| true,
            )
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;
    state.record_message_event(record("work", 42, 1)).unwrap();
    assert_eq!(task.await.unwrap()["events"][0]["message_id"], 1);
    let mut invalid = record("work", 42, 2);
    invalid["sender_name"] = json!({"text":"secret body"});
    assert!(state.record_message_event(invalid).is_err());
    let mut broadcast = record("work", 42, 3);
    broadcast["chat_type"] = json!("channel");
    state.record_message_event(broadcast).unwrap();
    assert_eq!(
        state
            .wait_message_events(
                &EventQuery {
                    after_seq: 1,
                    ..query()
                },
                &|_| true
            )
            .await
            .unwrap()["events"],
        json!([])
    );
}
#[tokio::test]
async fn messages_after_are_oldest_first_and_use_an_exclusive_cursor_without_read_markers() {
    let api = Api::default();
    api.reply(
        "messages.getHistory",
        page(vec![message(12, false), message(11, true)]),
    );
    let result = get_messages_after(&api, &json!({"chat_id":42,"after_id":10,"limit":2}))
        .await
        .unwrap();
    assert_eq!(result["messages"][0]["id"], 11);
    assert_eq!(result["messages"][1]["id"], 12);
    assert_eq!(result["next_after_id"], 12);
    assert_eq!(result["has_more"], true);
    assert_eq!(result["messages"][1]["sender"], "نام خانوادگی");
    let calls = api.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "messages.getHistory");
    assert_eq!(calls[0].1["offset_id"], 11);
    assert_eq!(calls[0].1["min_id"], 10);
    assert_eq!(calls[0].1["add_offset"], -2);
}
#[tokio::test]
async fn messages_after_enforces_topic_reads_and_validates_cursors_before_rpc() {
    let api = Api::default();
    for args in [
        json!({"chat_id":42,"after_id":-1}),
        json!({"chat_id":42,"after_id":2147483648i64}),
        json!({"chat_id":42,"topic_id":0}),
    ] {
        assert!(get_messages_after(&api, &args).await.is_err());
    }
    assert!(api.calls().is_empty());
    let mut permitted = message(11, false);
    permitted["reply_to"] = json!({"forum_topic":true,"reply_to_msg_id":7});
    let mut other = message(12, false);
    other["reply_to"] = json!({"forum_topic":true,"reply_to_msg_id":8});
    api.reply("messages.getReplies", page(vec![permitted, other]));
    let result = get_messages_after(&api, &json!({"chat_id":42,"after_id":10,"topic_id":7}))
        .await
        .unwrap();
    assert_eq!(result["messages"].as_array().unwrap().len(), 1);
    assert_eq!(result["messages"][0]["topic"], 7);
    assert_eq!(result["next_after_id"], 12);
    assert_eq!(api.calls()[0].0, "messages.getReplies");
    assert_eq!(api.calls()[0].1["msg_id"], 7);
}
#[tokio::test]
async fn unread_export_refuses_unrelated_or_malformed_existing_files_before_rpc() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let api = Api::default();
    for text in ["unrelated text".to_owned(),json!({"format":"telegram-mcp/unread-export-v1","account":"work","chats":{"42":{"messages":"corrupt"}}}).to_string()]{std::fs::write(&output,&text).unwrap();assert!(export_unread_with_state(&api,&state,&json!({"chat_ids":[42],"output_path":output,"account":"work"})).await.is_err());assert_eq!(std::fs::read_to_string(&output).unwrap(),text);assert!(api.calls().is_empty());}
}
#[tokio::test]
async fn unread_export_uses_dialog_inbox_boundary_and_excludes_outgoing_messages() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let api = Api::default();
    api.dialog(2, 3, 6);
    api.reply(
        "messages.getHistory",
        page(vec![message(6, false), message(5, false), message(4, true)]),
    );
    let result=export_unread_with_state(&api,&state,&json!({"chat_ids":[42],"output_path":output,"account":"work","include_media_metadata":false})).await.unwrap();
    assert_eq!(result["messages_exported"], 2);
    assert_eq!(result["complete"], true);
    assert_eq!(result["read_markers_sent"], false);
    let saved: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(saved["chats"]["42"]["messages"][0]["id"], 5);
    assert_eq!(saved["chats"]["42"]["next_after_id"], 6);
    assert!(saved["chats"]["42"]["messages"][0].get("media").is_none());
    assert_eq!(saved["telegram_user_id"], "9");
    assert_eq!(api.calls()[0].0, "users.getUsers");
    assert_eq!(api.calls()[2].1["min_id"], 3);
    assert_eq!(api.calls().len(), 3);
}
#[tokio::test]
async fn unread_export_resumes_partial_snapshot_even_after_inbox_read_boundary_changes() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let api = Api::default();
    api.dialog(2, 0, 2);
    api.reply("messages.getHistory", page(vec![message(1, false)]));
    let args = json!({"chat_ids":[42],"output_path":output,"account":"work","limit":1});
    let first = export_unread_with_state(&api, &state, &args).await.unwrap();
    assert_eq!(first["complete"], false);
    api.dialog(0, 2, 2);
    api.reply("messages.getHistory", page(vec![message(2, false)]));
    let next = export_unread_with_state(&api, &state, &args).await.unwrap();
    assert_eq!(next["complete"], true);
    let saved: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        saved["chats"]["42"]["messages"].as_array().unwrap().len(),
        2
    );
    assert_eq!(saved["chats"]["42"]["unread_count_at_export"], 2);
    assert_eq!(api.calls()[5].1["min_id"], 1);
    let calls = api.calls().len();
    assert!(
        export_unread_with_state(
            &api,
            &state,
            &json!({"chat_ids":[42],"output_path":output,"account":"work","resume":false})
        )
        .await
        .is_err()
    );
    assert_eq!(api.calls().len(), calls);
}
#[tokio::test]
async fn unread_export_with_zero_unread_never_requests_message_history() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let api = Api::default();
    api.dialog(0, 9, 9);
    let result =
        export_unread_with_state(&api, &state, &json!({"chat_ids":[42],"output_path":output}))
            .await
            .unwrap();
    assert_eq!(result["messages_exported"], 0);
    assert_eq!(result["complete"], true);
    assert_eq!(api.calls().len(), 2);
}

#[test]
fn forum_topics_distinguish_membership_from_real_message_replies() {
    use telegram_mcp::tools::local::upstream_events::{message_reply_id, message_topic_id};
    let membership = json!({"id":11,"reply_to":{"forum_topic":true,"reply_to_msg_id":7}});
    assert_eq!(message_topic_id(&membership, true), Some(7));
    assert_eq!(message_reply_id(&membership), None);
    let reply =
        json!({"id":12,"reply_to":{"forum_topic":true,"reply_to_msg_id":11,"reply_to_top_id":7}});
    assert_eq!(message_topic_id(&reply, true), Some(7));
    assert_eq!(message_reply_id(&reply), Some(11));
    assert_eq!(message_topic_id(&json!({"id":2}), true), Some(1));
    assert_eq!(message_topic_id(&membership, false), None);
    assert_eq!(
        message_topic_id(
            &json!({"id":9,"action":{"_":"messageActionTopicCreate"}}),
            true
        ),
        Some(9)
    );
}

#[tokio::test]
async fn unread_export_refuses_invalid_dialog_counts_without_creating_success_output() {
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let api = Api::default();
    api.dialog(-1, 0, 2);
    assert!(
        export_unread_with_state(&api, &state, &json!({"chat_ids":[42],"output_path":output}))
            .await
            .is_err()
    );
    assert!(!output.exists());
    assert_eq!(api.calls().len(), 2);
}

#[tokio::test]
async fn unresolved_group_addressing_preserves_only_an_explicit_boolean_unknown_marker() {
    let (_dir, state) = fixture();
    let mut unknown = record("work", -1000000000007, 1);
    unknown["chat_type"] = json!("supergroup");
    unknown["addressed"] = Value::Null;
    unknown["addressed_unknown"] = json!(true);
    state.record_message_event(unknown.clone()).unwrap();
    let result = state
        .wait_message_events(&query(), &|_| true)
        .await
        .unwrap();
    assert!(result["events"][0].get("addressed").is_none());
    assert_eq!(result["events"][0]["addressed_unknown"], true);
    let known = record("work", 42, 2);
    state.record_message_event(known).unwrap();
    let next = state
        .wait_message_events(
            &EventQuery {
                after_seq: 1,
                ..query()
            },
            &|_| true,
        )
        .await
        .unwrap();
    assert_eq!(next["events"][0]["addressed"], true);
    assert!(next["events"][0].get("addressed_unknown").is_none());
    for invalid in [json!("yes"), json!({"text":"untrusted body"})] {
        unknown["addressed_unknown"] = invalid;
        assert!(state.record_message_event(unknown.clone()).is_err());
    }
}

#[tokio::test]
async fn concurrent_unread_exports_lease_the_destination_without_locking_its_data_bytes() {
    use std::sync::Arc;
    struct PausingApi {
        api: Api,
        entered: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }
    #[async_trait]
    impl Telegram for PausingApi {
        async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
            if method == "messages.getPeerDialogs" {
                self.entered.notify_one();
                self.resume.notified().await;
            }
            self.api.invoke(method, params).await
        }
        async fn resolve_peer(&self, value: &Value) -> Result<Value> {
            self.api.resolve_peer(value).await
        }
    }
    let (dir, state) = fixture();
    let output = dir.path().join("allowed/unread.json");
    let api = Arc::new(PausingApi {
        api: Api::default(),
        entered: Default::default(),
        resume: Default::default(),
    });
    api.api.dialog(0, 9, 9);
    let first_api = api.clone();
    let first_state = state.clone();
    let args = json!({"chat_ids":[42],"output_path":output,"account":"work"});
    let first_args = args.clone();
    let first = tokio::spawn(async move {
        export_unread_with_state(first_api.as_ref(), &first_state, &first_args).await
    });
    tokio::time::timeout(Duration::from_secs(3), api.entered.notified())
        .await
        .unwrap();
    let competing = Api::default();
    assert!(
        export_unread_with_state(&competing, &state, &args)
            .await
            .is_err()
    );
    assert!(competing.calls().is_empty());
    assert!(!output.exists());
    api.resume.notify_one();
    assert_eq!(first.await.unwrap().unwrap()["complete"], true);
    let saved: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(saved["account"], "work");
    let resumed = export_unread_with_state(&competing, &state, &args)
        .await
        .unwrap();
    assert_eq!(resumed["complete"], true);
    assert_eq!(competing.calls().len(), 1);
    assert_eq!(competing.calls()[0].0, "users.getUsers");
}
