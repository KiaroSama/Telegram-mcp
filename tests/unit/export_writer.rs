use super::Writer;
use crate::export::settings::Settings;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn settings(path: &std::path::Path, format: &str) -> Settings {
    Settings {
        path: path.to_owned(),
        format: format.into(),
        media: [false; 7],
        max_bytes: 1024,
        from: 0,
        until: 0,
    }
}
fn message(id: i64) -> Value {
    json!({"_":"message","id":id,"date":1700000000,"message":format!("سلام 😀 {id}"),"entities":[]})
}

#[test]
fn json_only_export_finishes_as_valid_ordered_json_without_creating_html() {
    let temp = tempfile::tempdir().unwrap();
    let options = settings(temp.path(), "json");
    let mut writer = Writer::new(
        temp.path(),
        &options,
        "<private>",
        &json!({"_":"inputPeerUser","user_id":"9007199254740993"}),
    )
    .unwrap();
    assert!(
        !temp.path().join("result.json").exists(),
        "Partial JSON must remain private until finish"
    );
    writer.message(&message(7), &BTreeMap::new()).unwrap();
    writer.message(&message(8), &BTreeMap::new()).unwrap();
    writer.finish().unwrap();
    let value: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("result.json")).unwrap()).unwrap();
    assert_eq!(value["type"], "personal_chat");
    assert_eq!(value["id"], "9007199254740993");
    assert_eq!(value["messages"][0]["id"], 7);
    assert_eq!(value["messages"][1]["id"], 8);
    assert_eq!(value["messages"][1]["text"], "سلام 😀 8");
    assert!(!temp.path().join("messages.html").exists());
}

#[test]
fn html_only_export_rotates_after_a_thousand_messages_and_links_both_directions() {
    let temp = tempfile::tempdir().unwrap();
    let options = settings(temp.path(), "html");
    let mut writer = Writer::new(
        temp.path(),
        &options,
        "<channel>",
        &json!({"_":"inputPeerChannel","channel_id":"7"}),
    )
    .unwrap();
    for id in 1..=1001 {
        writer.message(&message(id), &BTreeMap::new()).unwrap();
    }
    writer.finish().unwrap();
    let first = std::fs::read_to_string(temp.path().join("messages.html")).unwrap();
    let second = std::fs::read_to_string(temp.path().join("messages2.html")).unwrap();
    assert_eq!(first.matches("<article").count(), 1000);
    assert_eq!(second.matches("<article").count(), 1);
    assert!(first.contains("href=\"messages2.html\">Next messages"));
    assert!(second.contains("href=\"messages.html\">Previous messages"));
    assert!(second.contains("&lt;channel&gt;"));
    assert!(second.ends_with("</html>\n"));
    assert!(!temp.path().join("result.json").exists());
    assert!(!temp.path().join("messages3.html").exists());
}

#[test]
fn empty_channel_export_and_saved_messages_keep_their_actual_peer_types() {
    for (peer, kind) in [
        (json!({"_":"inputPeerChannel","channel_id":"7"}), "channel"),
        (json!({"_":"inputPeerSelf"}), "personal_chat"),
        (json!({"_":"inputPeerChat","chat_id":"9"}), "private_group"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let options = settings(temp.path(), "json");
        Writer::new(temp.path(), &options, "empty", &peer)
            .unwrap()
            .finish()
            .unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(temp.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(value["type"], kind);
        assert_eq!(value["messages"], json!([]));
    }
}

#[test]
fn finalization_never_overwrites_an_existing_export_json() {
    let temp = tempfile::tempdir().unwrap();
    let options = settings(temp.path(), "json");
    let writer = Writer::new(
        temp.path(),
        &options,
        "new",
        &json!({"_":"inputPeerUser","user_id":"7"}),
    )
    .unwrap();
    std::fs::write(temp.path().join("result.json"), b"existing owner's export").unwrap();
    assert!(
        writer
            .finish()
            .unwrap_err()
            .to_string()
            .contains("finalized")
    );
    assert_eq!(
        std::fs::read(temp.path().join("result.json")).unwrap(),
        b"existing owner's export"
    );
    assert_eq!(
        std::fs::read_dir(temp.path()).unwrap().count(),
        1,
        "Unpublished temporary JSON must be removed"
    );
}

#[test]
fn html_creation_never_truncates_an_existing_export() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("messages.html"), b"prior HTML").unwrap();
    assert!(
        Writer::new(
            temp.path(),
            &settings(temp.path(), "html"),
            "new",
            &json!({})
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(temp.path().join("messages.html")).unwrap(),
        b"prior HTML"
    );
}

#[test]
fn invalid_entity_message_does_not_publish_incomplete_json() {
    let temp = tempfile::tempdir().unwrap();
    let options = settings(temp.path(), "html_and_json");
    let mut writer = Writer::new(
        temp.path(),
        &options,
        "broken",
        &json!({"_":"inputPeerUser","user_id":"7"}),
    )
    .unwrap();
    let bad = json!({"_":"message","id":7,"date":1700000000,"message":"😀","entities":[{"_":"messageEntityBold","offset":1,"length":1}]});
    assert!(writer.message(&bad, &BTreeMap::new()).is_err());
    drop(writer);
    assert!(!temp.path().join("result.json").exists());
}
