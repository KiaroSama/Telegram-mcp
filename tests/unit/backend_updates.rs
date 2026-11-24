use super::*;
use crate::{state::EventQuery, tl::Schema};
use grammers_session::types::ChannelState;
use grammers_tl_types::Deserializable;

fn fixture() -> (tempfile::TempDir, Arc<State>) {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    (temp, state)
}
fn peer() -> MessageContext {
    MessageContext {
        chat_id: "42".into(),
        chat_type: "private",
        forum: false,
        username: Some("friend".into()),
        name: "نام کامل".into(),
        bot: false,
        sender_id: Some("42".into()),
        sender_name: Some("نام کامل".into()),
        sender_bot: false,
    }
}
fn fields() -> Value {
    json!({"id":7,"out":false,"mentioned":false,"date":1,"service":false})
}
fn raw(mut value: Value) -> tl::enums::Message {
    if value.get("_").is_none() {
        value["_"] = json!("message");
    }
    let bytes = Schema::global().encode_object(&value).unwrap();
    tl::enums::Message::from_bytes(&bytes).unwrap()
}
fn raw_message() -> Value {
    json!({"_":"message","id":7,"peer_id":{"_":"peerUser","user_id":"42"},
        "date":1,"message":"body-never-recorded","media":{"_":"messageMediaPhoto","photo":{"_":"photoEmpty","id":"1"}},
        "reply_to":{"_":"messageReplyHeader","reply_to_msg_id":3,"quote_text":"quote-never-recorded"}})
}
async fn events(state: &State) -> Value {
    state
        .wait_message_events(
            &EventQuery {
                limit: 100,
                ..Default::default()
            },
            &|_| true,
        )
        .await
        .unwrap()
}
fn delivered(message_box: Option<MessageBox>) -> DeliveredState {
    DeliveredState {
        date: 999,
        seq: 99,
        message_box,
    }
}
fn current() -> UpdatesState {
    UpdatesState {
        pts: 10,
        qts: 71,
        date: 100,
        seq: 5,
        channels: vec![ChannelState { id: 9, pts: 30 }],
    }
}

#[test]
fn typed_message_extractor_discards_text_quotes_and_media_references() {
    let extracted = message_fields(&raw(raw_message())).unwrap();
    let serialized = extracted.to_string();
    assert!(!serialized.contains("never-recorded"));
    assert!(!serialized.contains("photo"));
    assert!(!serialized.contains("access_hash"));
    assert_eq!(extracted["reply_to"]["reply_to_msg_id"], 3);
    assert_eq!(project("work", &extracted, &peer())["reply_to"], 3);
}

#[test]
fn typed_topic_creation_uses_its_own_message_id_without_service_title() {
    let source = json!({"_":"messageService","id":90,"peer_id":{"_":"peerChannel","channel_id":"9"},
        "date":1,"action":{"_":"messageActionTopicCreate","title":"title-never-recorded","icon_color":0}});
    let extracted = message_fields(&raw(source)).unwrap();
    let mut group = peer();
    group.chat_type = "supergroup";
    group.forum = true;
    let metadata = project("work", &extracted, &group);
    assert_eq!(metadata["topic"], 90);
    assert_eq!(metadata["service"], true);
    assert!(!extracted.to_string().contains("title-never-recorded"));
}

#[test]
fn topic_root_and_nested_replies_follow_upstream_semantics() {
    let mut group = peer();
    group.chat_type = "supergroup";
    group.forum = true;
    let mut source = raw_message();
    source["reply_to"] = json!({"_":"messageReplyHeader","forum_topic":true,"reply_to_msg_id":9});
    let extracted = message_fields(&raw(source.clone())).unwrap();
    let event = project("work", &extracted, &group);
    assert_eq!(event["topic"], 9);
    assert!(event["reply_to"].is_null());
    source["reply_to"]["reply_to_top_id"] = json!(9);
    source["reply_to"]["reply_to_msg_id"] = json!(11);
    let extracted = message_fields(&raw(source)).unwrap();
    let event = project("work", &extracted, &group);
    assert_eq!(event["topic"], 9);
    assert_eq!(event["reply_to"], 11);
}

#[test]
fn general_topic_is_assigned_only_to_known_forums() {
    let mut group = peer();
    group.chat_type = "supergroup";
    group.forum = true;
    assert_eq!(project("work", &fields(), &group)["topic"], 1);
    group.forum = false;
    assert!(project("work", &fields(), &group)["topic"].is_null());
}

#[test]
fn outgoing_group_metadata_keeps_direction_identity_and_reply() {
    let mut group = peer();
    group.chat_id = "-1000000000009".into();
    group.chat_type = "supergroup";
    group.sender_id = Some("9007199254740993".into());
    let mut source = fields();
    source["out"] = json!(true);
    source["mentioned"] = json!(true);
    source["reply_to"] = json!({"reply_to_msg_id":6});
    let event = project("work", &source, &group);
    assert_eq!(event["sender_id"], "9007199254740993");
    assert_eq!(event["chat_id"], "-1000000000009");
    assert_eq!(event["out"], true);
    assert_eq!(event["addressed"], false);
    assert_eq!(event["reply_to"], 6);
    assert_eq!(event["date"], "1970-01-01T00:00:01Z");
}

#[test]
fn group_mentions_are_addressed_and_unknown_senders_stay_unknown() {
    let mut group = peer();
    group.chat_type = "group";
    group.sender_id = None;
    group.sender_name = None;
    assert_eq!(project("work", &fields(), &group)["addressed"], false);
    let mut source = fields();
    source["mentioned"] = json!(true);
    let event = project("work", &source, &group);
    assert_eq!(event["addressed"], true);
    assert!(event["sender_id"].is_null());
    assert!(event["sender_name"].is_null());
}

#[test]
fn empty_typed_messages_have_no_invented_event_metadata() {
    assert!(message_fields(&raw(json!({"_":"messageEmpty","id":7}))).is_none());
}

#[tokio::test]
async fn private_incoming_callback_populates_both_metadata_and_burst_streams() {
    let (_temp, state) = fixture();
    publish(&state, &AccessPolicy::default(), "work", &fields(), &peer()).unwrap();
    let result = events(&state).await;
    assert_eq!(result["events"].as_array().unwrap().len(), 1);
    assert_eq!(result["events"][0]["sender_name"], "نام کامل");
    assert_eq!(result["events"][0]["addressed"], true);
    assert!(result["events"][0].get("_chat_username").is_none());
    let pending = state
        .wait_incoming(
            Some("work"),
            Some("42"),
            std::time::Duration::from_millis(1),
            None,
            10,
        )
        .await
        .unwrap();
    assert_eq!(pending["pending_chats"][0]["count"], 1);
    assert_eq!(pending["pending_chats"][0]["last_message_id"], "7");
}

#[tokio::test]
async fn outgoing_and_group_callbacks_never_create_private_incoming_bursts() {
    let (_temp, state) = fixture();
    let mut source = fields();
    source["out"] = json!(true);
    publish(&state, &AccessPolicy::default(), "work", &source, &peer()).unwrap();
    let mut group = peer();
    group.chat_type = "supergroup";
    group.chat_id = "-1000000000009".into();
    publish(&state, &AccessPolicy::default(), "work", &fields(), &group).unwrap();
    let result = events(&state).await;
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
    assert_eq!(result["events"][0]["out"], true);
    assert_eq!(result["events"][1]["chat_type"], "supergroup");
    assert_eq!(state.incoming_feed_status().unwrap()["pending_chats"], 0);
}

#[tokio::test]
async fn disallowed_callbacks_enter_neither_metadata_nor_private_bursts() {
    let (_temp, state) = fixture();
    let policy = AccessPolicy::parse(Some("99")).unwrap();
    publish(&state, &policy, "work", &fields(), &peer()).unwrap();
    assert!(
        events(&state).await["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.incoming_feed_status().unwrap()["pending_chats"], 0);
}

#[tokio::test]
async fn callback_topic_policy_blocks_other_topics_and_general() {
    let (_temp, state) = fixture();
    let policy = AccessPolicy::parse(Some("-1000000000009:9")).unwrap();
    let mut group = peer();
    group.chat_id = "-1000000000009".into();
    group.chat_type = "supergroup";
    group.forum = true;
    publish(&state, &policy, "work", &fields(), &group).unwrap();
    let mut source = fields();
    source["reply_to"] = json!({"forum_topic":true,"reply_to_msg_id":10});
    publish(&state, &policy, "work", &source, &group).unwrap();
    source["reply_to"]["reply_to_msg_id"] = json!(9);
    publish(&state, &policy, "work", &source, &group).unwrap();
    let result = events(&state).await;
    assert_eq!(result["events"].as_array().unwrap().len(), 1);
    assert_eq!(result["events"][0]["topic"], 9);
}

#[tokio::test]
async fn callback_username_scope_matches_without_disclosing_internal_alias() {
    let (_temp, state) = fixture();
    let policy = AccessPolicy::parse(Some("@FRIEND")).unwrap();
    publish(&state, &policy, "work", &fields(), &peer()).unwrap();
    let result = events(&state).await;
    assert_eq!(result["events"].as_array().unwrap().len(), 1);
    assert!(!result.to_string().contains("friend"));
}

#[tokio::test]
async fn broadcast_callback_is_not_misrepresented_as_group_event() {
    let (_temp, state) = fixture();
    let mut channel = peer();
    channel.chat_type = "channel";
    publish(
        &state,
        &AccessPolicy::default(),
        "work",
        &fields(),
        &channel,
    )
    .unwrap();
    assert!(
        events(&state).await["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.incoming_feed_status().unwrap()["pending_chats"], 0);
}

fn group_reply() -> (MessageContext, Value) {
    let mut group = peer();
    group.chat_id = "-1000000000009".into();
    group.chat_type = "supergroup";
    group.forum = true;
    let mut source = fields();
    source["reply_to"] = json!({"forum_topic":true,"reply_to_top_id":9,"reply_to_msg_id":11});
    (group, source)
}

#[tokio::test]
async fn forbidden_topic_reply_never_creates_the_reference_fetch() {
    let (group, mut source) = group_reply();
    let policy = AccessPolicy::parse(Some("-1000000000009:10")).unwrap();
    let mut called = false;
    address_reply(&policy, &mut source, &group, || async {
        called = true;
        Ok(Some(true))
    })
    .await;
    assert!(!called);
    assert!(source.get("reply_outgoing").is_none());
}

#[tokio::test]
async fn allowed_group_reply_to_owned_message_is_addressed() {
    let (group, mut source) = group_reply();
    let policy = AccessPolicy::parse(Some("-1000000000009:9")).unwrap();
    let mut called = false;
    address_reply(&policy, &mut source, &group, || async {
        called = true;
        Ok(Some(true))
    })
    .await;
    assert!(called);
    let event = project("work", &source, &group);
    assert_eq!(event["addressed"], true);
    assert!(event.get("addressed_unknown").is_none());
}

#[tokio::test]
async fn group_reply_to_someone_else_is_known_not_addressed() {
    let (group, mut source) = group_reply();
    address_reply(&AccessPolicy::default(), &mut source, &group, || async {
        Ok(Some(false))
    })
    .await;
    let event = project("work", &source, &group);
    assert_eq!(event["addressed"], false);
    assert!(event.get("addressed_unknown").is_none());
}

#[tokio::test]
async fn unavailable_reply_reference_reports_unknown_instead_of_false() {
    for missing in [false, true] {
        let (group, mut source) = group_reply();
        address_reply(&AccessPolicy::default(), &mut source, &group, || async {
            if missing {
                Ok(None)
            } else {
                anyhow::bail!("Reference unavailable")
            }
        })
        .await;
        let event = project("work", &source, &group);
        assert!(event["addressed"].is_null());
        assert_eq!(event["addressed_unknown"], true);
    }
}

#[tokio::test(start_paused = true)]
async fn reply_reference_fetch_is_bounded_to_five_seconds() {
    let (group, mut source) = group_reply();
    let started = tokio::time::Instant::now();
    address_reply(&AccessPolicy::default(), &mut source, &group, || async {
        std::future::pending::<Result<Option<bool>>>().await
    })
    .await;
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(5));
    assert_eq!(project("work", &source, &group)["addressed_unknown"], true);
}

#[tokio::test]
async fn structural_topic_replies_and_known_addressing_do_not_fetch() {
    for variant in 0..4 {
        let (mut group, mut source) = group_reply();
        match variant {
            0 => source["out"] = json!(true),
            1 => source["mentioned"] = json!(true),
            2 => group.chat_type = "private",
            _ => source["reply_to"]["reply_to_msg_id"] = json!(9),
        }
        let mut called = false;
        address_reply(&AccessPolicy::default(), &mut source, &group, || async {
            called = true;
            Ok(Some(true))
        })
        .await;
        assert!(!called);
    }
}

#[tokio::test]
async fn cross_chat_replies_never_fetch_outside_the_event_chat() {
    let (group, mut source) = group_reply();
    source["reply_to"]["reply_to_peer_id"] = json!("-1000000000099");
    let mut called = false;
    address_reply(&AccessPolicy::default(), &mut source, &group, || async {
        called = true;
        Ok(Some(true))
    })
    .await;
    assert!(!called);
    assert_eq!(project("work", &source, &group)["addressed_unknown"], true);
}

#[tokio::test]
async fn unresolved_addressing_survives_the_metadata_whitelist_without_false_value() {
    let (_temp, state) = fixture();
    let (group, source) = group_reply();
    publish(&state, &AccessPolicy::default(), "work", &source, &group).unwrap();
    let result = events(&state).await;
    assert_eq!(result["events"][0]["addressed_unknown"], true);
    assert!(result["events"][0].get("addressed").is_none());
}

fn packet(message: Value, users: Value, chats: Value, channel: bool) -> UpdatesLike {
    let update = if channel {
        "updateNewChannelMessage"
    } else {
        "updateNewMessage"
    };
    let bytes=Schema::global().encode_object(&json!({"_":"updates","date":1,"seq":1,
        "updates":[{"_":update,"message":message,"pts":1,"pts_count":1}],"users":users,"chats":chats})).unwrap();
    UpdatesLike::Updates(tl::enums::Updates::from_bytes(&bytes).unwrap())
}

async fn offline_update(message: Value, users: Value, chats: Value, channel: bool) -> Update {
    // The SenderPool runner is deliberately never run. This feeds actual
    // grammers conversion/cache logic through an in-memory update channel,
    // without any authorization key, Telegram connection or RPC execution.
    let pool = grammers_mtsender::SenderPool::new(
        Arc::new(grammers_session::storages::MemorySession::default()),
        1,
    );
    let client = Client::new(pool.handle);
    let (send, receive) = tokio::sync::mpsc::channel(1);
    let mut stream = client
        .stream_updates(receive, UpdatesConfiguration { catch_up: false })
        .await
        .unwrap();
    send.send(packet(message, users, chats, channel))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn official_update_conversion_populates_cached_private_sender_metadata() {
    let mut message = raw_message();
    message["reply_to"] = Value::Null;
    let update = offline_update(
        message,
        json!([{"_":"user","id":"42","first_name":"نام","last_name":"خانوادگی",
        "username":"friend","bot":true,"bot_info_version":1,"access_hash":"7"}]),
        json!([]),
        false,
    )
    .await;
    let Update::NewMessage(message) = update else {
        panic!("Expected new message")
    };
    let peer = context(&message).unwrap();
    assert_eq!(peer.chat_id, "42");
    assert_eq!(peer.chat_type, "private");
    assert_eq!(peer.name, "نام خانوادگی");
    assert_eq!(peer.sender_name.as_deref(), Some("نام خانوادگی"));
    assert_eq!(peer.sender_id.as_deref(), Some("42"));
    assert!(peer.bot && peer.sender_bot);
    assert_eq!(peer.username.as_deref(), Some("friend"));
    assert!(!peer.forum);
}

#[tokio::test]
async fn official_update_context_does_not_expose_the_outgoing_self_sentinel() {
    let mut source = raw_message();
    source["out"] = json!(true);
    source["reply_to"] = Value::Null;
    let update = offline_update(source, json!([]), json!([]), false).await;
    let Update::NewMessage(message) = update else {
        panic!("Expected new message")
    };
    let peer = context(&message).unwrap();
    assert!(peer.sender_id.is_none());
    assert!(peer.sender_name.is_none());
    assert_eq!(peer.name, "");
    assert_eq!(
        project(
            "work",
            &message_fields(&message.into_inner().raw).unwrap(),
            &peer
        )["addressed"],
        false
    );
}

#[tokio::test]
async fn official_group_conversion_distinguishes_basic_forum_and_broadcast_chats() {
    for (channel, chat, expected, forum) in [
        (
            false,
            json!({"_":"chat","id":9,"title":"Small group","photo":{"_":"chatPhotoEmpty"},"participants_count":2,"date":1,"version":1}),
            "group",
            false,
        ),
        (
            true,
            json!({"_":"channel","id":9,"title":"Forum","photo":{"_":"chatPhotoEmpty"},"date":1,"megagroup":true,"forum":true,"access_hash":"99"}),
            "supergroup",
            true,
        ),
        (
            true,
            json!({"_":"channel","id":9,"title":"Broadcast","photo":{"_":"chatPhotoEmpty"},"date":1,"broadcast":true,"access_hash":"99"}),
            "channel",
            false,
        ),
    ] {
        let source = json!({"_":"message","id":7,"peer_id":if channel{json!({"_":"peerChannel","channel_id":9})}else{json!({"_":"peerChat","chat_id":9})},"date":1,"message":"not-recorded"});
        let update = offline_update(source, json!([]), json!([chat]), channel).await;
        let Update::NewMessage(message) = update else {
            panic!("Expected new message")
        };
        let peer = context(&message).unwrap();
        assert_eq!(peer.chat_type, expected);
        assert_eq!(peer.forum, forum);
        assert_eq!(peer.chat_id, if channel { "-1000000000009" } else { "-9" });
        assert!(!peer.name.is_empty());
    }
}

#[tokio::test]
async fn unknown_channel_metadata_does_not_invent_a_forum_or_supergroup() {
    let source = json!({"_":"message","id":7,"peer_id":{"_":"peerChannel","channel_id":9},"date":1,"message":"not-recorded"});
    let update = offline_update(source, json!([]), json!([]), true).await;
    let Update::NewMessage(message) = update else {
        panic!("Expected new message")
    };
    let peer = context(&message).unwrap();
    assert_eq!(peer.chat_type, "channel");
    assert!(!peer.forum);
}

// Keep the fully qualified names used by the CI test inventory.
include!("backend_updates/checkpoint.rs");
