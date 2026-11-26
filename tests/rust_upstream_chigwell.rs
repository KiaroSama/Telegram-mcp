use anyhow::{Result, bail};
use async_trait::async_trait;
use grammers_session::{
    types::UpdatesState,
    updates::{MessageBoxes, UpdatesLike},
};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Mutex};
use telegram_mcp::{backend::Telegram, server, tools};

#[derive(Default)]
struct Fixture {
    replies: Mutex<VecDeque<(&'static str, Value)>>,
    calls: Mutex<Vec<(&'static str, Value)>>,
}

impl Fixture {
    fn with<const N: usize>(replies: [(&'static str, Value); N]) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            ..Self::default()
        }
    }
}

#[async_trait]
impl Telegram for Fixture {
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        Ok(json!({"_":"inputPeerUser","user_id":"7","access_hash":"8"}))
    }

    async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
        let Some((expected, reply)) = self.replies.lock().unwrap().pop_front() else {
            bail!("Unexpected Telegram method: {method}");
        };
        assert_eq!(method, expected);
        self.calls.lock().unwrap().push((expected, args));
        Ok(reply)
    }
}

#[tokio::test]
async fn message_read_reports_distinct_kinds_without_changing_text_or_utf16_entities() {
    let text = "  سلام 🙂\nline  ";
    let entities = json!([
        {"_":"messageEntityTextUrl","offset":10,"length":4,"url":"https://example.com"},
        {"_":"messageEntityCustomEmoji","offset":7,"length":2,"document_id":"9007199254740993"},
        {"_":"messageEntityBold","offset":2,"length":4},
        {"_":"messageEntityBold","offset":10,"length":4},
        {"_":"messageEntityStrike","offset":10,"length":4}
    ]);
    let formatted = json!({"_":"message","id":1,"message":text,"entities":entities});
    let plain = json!({"_":"message","id":2,"message":"plain","entities":null});
    let api = Fixture::with([(
        "messages.getHistory",
        json!({"messages":[formatted, plain]}),
    )]);
    let result = tools::call(&api, "get_messages", &json!({"chat_id":7}))
        .await
        .unwrap();
    let rows = result["messages"].as_array().unwrap();
    assert_eq!(rows[0]["message"], text);
    assert_eq!(rows[0]["entities"], entities);
    assert_eq!(
        rows[0]["formatting"],
        json!(["bold", "custom_emoji", "strikethrough", "text_url"])
    );
    assert_eq!(rows[1], plain);
    let public = telegram_mcp::sanitize::sanitize_value(result);
    assert_eq!(public["messages"][0]["message"], text.trim());
    assert_eq!(public["messages"][0]["entities"][1]["offset"], 5);
    assert_eq!(public["messages"][0]["entities"][1]["length"], 2);
}

#[tokio::test]
async fn message_read_rebases_utf16_and_links_the_message_peer() {
    let row = |peer: Value| {
        json!({"_":"message","id":9,"peer_id":peer,"message":"A\u{202e}🙂B","entities":[
        {"_":"messageEntityCustomEmoji","offset":2,"length":2,"document_id":"9007199254740993"},
        {"_":"messageEntityBold","offset":1,"length":1}],
        "reply_to":{"quote_text":"A\u{202e}B","quote_entities":[{"_":"messageEntityItalic","offset":2,"length":1}]}})
    };
    let api = Fixture::with([(
        "messages.getHistory",
        json!({"messages":[
        row(json!({"_":"peerChannel","channel_id":"7"})),
        row(json!({"_":"peerChannel","channel_id":"-10012345678901"})),
        row(json!({"_":"peerChat","chat_id":"7"}))],
        "chats":[{"_":"channel","id":7,"username":"publicchat"}]}),
    )]);
    let result = tools::call(&api, "get_messages", &json!({"chat_id":7}))
        .await
        .unwrap();
    let rows = result["messages"].as_array().unwrap();
    assert_eq!(rows[0]["message"], "A🙂B");
    assert_eq!(
        rows[0]["entities"],
        json!([{"_":"messageEntityCustomEmoji","offset":1,"length":2,"document_id":"9007199254740993"}])
    );
    assert_eq!(rows[0]["reply_to"]["quote_text"], "AB");
    assert_eq!(rows[0]["reply_to"]["quote_entities"][0]["offset"], 1);
    let host = telegram_mcp::permalinks::link_domain();
    assert_eq!(rows[0]["permalink"], format!("https://{host}/publicchat/9"));
    assert_eq!(
        rows[1]["permalink"],
        format!("https://{host}/c/12345678901/9")
    );
    assert!(rows[2].get("permalink").is_none());
}

#[test]
fn pinned_grammers_marks_a_disconnect_for_difference_on_the_existing_update_cursor() {
    let mut boxes = MessageBoxes::load(UpdatesState {
        pts: 17,
        qts: 0,
        date: 1700000000,
        seq: 2,
        channels: vec![],
    });
    let initial = boxes.get_difference().unwrap();
    assert_eq!(initial.pts, 17);
    boxes.apply_difference(grammers_tl_types::enums::updates::Difference::Empty(
        grammers_tl_types::types::updates::DifferenceEmpty {
            date: 1700000001,
            seq: 3,
        },
    ));
    assert!(boxes.get_difference().is_none());
    assert!(
        boxes
            .process_updates(UpdatesLike::ConnectionClosed)
            .is_err()
    );
    let reconnect = boxes.get_difference().unwrap();
    assert_eq!(reconnect.pts, 17);
    assert_eq!(reconnect.date, 1700000001);
}

#[tokio::test]
async fn private_chat_leave_explains_membership_without_dispatching_a_mutation() {
    let api = Fixture::default();
    let error = tools::call(&api, "leave_chat", &json!({"chat_id":7}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Private chats have no membership to leave"));
    assert!(error.contains("delete_chat"));
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn photo_id_is_exact_and_missing_or_malformed_ids_never_delete_the_current_photo() {
    let id = "9007199254740993";
    let saved = json!({"_":"photo","id":id,"access_hash":"9","file_reference":[3,4]});
    let first: Vec<_> = (1..=100)
        .map(|id| json!({"_":"photo","id":id.to_string()}))
        .collect();
    let api = Fixture::with([
        ("photos.getUserPhotos", json!({"photos":first})),
        ("photos.getUserPhotos", json!({"photos":[saved]})),
        ("photos.deletePhotos", json!([id])),
    ]);
    tools::call(&api, "delete_profile_photo", &json!({"photo_id":id}))
        .await
        .unwrap();
    {
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls[0].1["limit"], 100);
        assert_eq!(calls[1].1["offset"], 100);
        assert_eq!(calls[2].1["id"][0]["id"], id);
        assert_eq!(calls[2].1["id"][0]["file_reference"], json!([3, 4]));
    }

    let defaults = Fixture::with([
        ("photos.getUserPhotos", json!({"photos":[saved]})),
        ("photos.deletePhotos", json!([id])),
    ]);
    tools::call(&defaults, "delete_profile_photo", &json!({}))
        .await
        .unwrap();
    assert_eq!(defaults.calls.lock().unwrap()[0].1["limit"], 1);

    let missing = Fixture::with([("photos.getUserPhotos", json!({"photos":[saved]}))]);
    let error = tools::call(&missing, "delete_profile_photo", &json!({"photo_id":"99"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("No matching profile photo"));
    assert_eq!(missing.calls.lock().unwrap().len(), 1);
    for photo_id in [json!(0), json!("not-a-number")] {
        let invalid = Fixture::default();
        assert!(
            tools::call(
                &invalid,
                "delete_profile_photo",
                &json!({"photo_id":photo_id})
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("photo_id")
        );
        assert!(invalid.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn contact_export_keeps_only_allowed_rows_and_drops_excluded_phone_numbers() {
    let api = Fixture::with([(
        "contacts.getContacts",
        json!({"users":[
            {"_":"user","id":"7","first_name":"Allowed","phone":"70001"},
            {"_":"user","id":"9","first_name":"Excluded","phone":"90001"}
        ]}),
    )]);
    let mut exported = tools::call(&api, "export_contacts", &json!({}))
        .await
        .unwrap();
    assert_eq!(exported["results"].as_array().unwrap().len(), 2);
    server::access::AccessPolicy::parse(Some("7"))
        .unwrap()
        .filter_for("export_contacts", &mut exported)
        .unwrap();
    let rows = exported["results"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "7");
    assert!(!exported.to_string().contains("90001"));
}

#[test]
fn catalogue_exposes_exact_photo_ids_and_local_audio_with_no_account_selector() {
    let catalogue: Vec<Value> =
        serde_json::from_str(include_str!("../resources/tool-catalogue.json")).unwrap();
    let delete = catalogue
        .iter()
        .find(|t| t["name"] == "delete_profile_photo")
        .unwrap();
    let schema = delete["inputSchema"].as_object().unwrap();
    let mut with_id = json!({"photo_id":"9007199254740993"});
    server::validate_arguments(&mut with_id, schema).unwrap();
    assert_eq!(with_id["photo_id"], "9007199254740993");
    server::validate_arguments(&mut json!({}), schema).unwrap();

    let audio = catalogue
        .iter()
        .find(|t| t["name"] == "transcribe_audio")
        .unwrap();
    assert_eq!(audio["inputSchema"]["required"], json!(["file_path"]));
    assert!(audio["inputSchema"]["properties"].get("account").is_none());
    assert_eq!(audio["annotations"]["readOnlyHint"], true);
    assert_eq!(audio["annotations"]["openWorldHint"], false);
    assert!(tools::messaging::TOOL_NAMES.contains(&"transcribe_audio"));
}
