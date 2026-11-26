use super::Writer;
use crate::export::admin_log::map_event_parts;
use crate::export::admin_log_text::LogChat;
use crate::export::settings::Settings;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn settings(path: &std::path::Path, format: &str) -> Settings {
    Settings {
        path: path.to_owned(),
        format: format.into(),
        media: [false; 7],
        max_bytes: 4096,
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

#[test]
fn desktop_writer_renders_service_actions_vcards_thumbnails_and_scrubs_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let export_dir = temp.path().join("out");
    std::fs::create_dir_all(&export_dir).unwrap();
    let _guard = crate::state::State::at(state_dir, vec![export_dir.clone()]).unwrap();

    let peers = BTreeMap::from([
        (
            "user:7".into(),
            json!({"_":"user","id":"7","first_name":"Sara","access_hash":"999999"}),
        ),
        (
            "user:8".into(),
            json!({"_":"user","id":"8","first_name":"Ali"}),
        ),
    ]);
    let options = settings(&export_dir, "html_and_json");
    let mut writer = Writer::new(
        &export_dir,
        &options,
        "Team",
        &json!({"_":"inputPeerChannel","channel_id":"55","access_hash":"secret-hash"}),
    )
    .unwrap();

    let svc = json!({
        "_": "messageService",
        "id": 1,
        "date": 1700000000,
        "from_id": {"_": "peerUser", "user_id": "7"},
        "action": {"_": "messageActionChatCreate", "title": "Core <Team>", "users": ["8"]}
    });
    let doc_msg = json!({
        "_": "message",
        "id": 2,
        "date": 1700000010,
        "from_id": {"_": "peerUser", "user_id": "7"},
        "message": "Video attachment",
        "source_peer": {"_": "inputPeerChannel", "channel_id": "55", "access_hash": "secret-hash"},
        "export_file": "video_files/2_100_clip.mp4",
        "export_thumb": "video_files/2_100_clip.mp4_thumb.jpg",
        "export_thumb_size": 512,
        "media": {
            "_": "messageMediaDocument",
            "document": {
                "_": "document",
                "id": "100",
                "access_hash": "doc-secret",
                "file_reference": [7, 8, 9],
                "size": 2048,
                "mime_type": "video/mp4",
                "attributes": [
                    {"_": "documentAttributeFilename", "file_name": "clip.mp4"},
                    {"_": "documentAttributeVideo", "w": 640, "h": 360, "duration": 12, "round_message": false}
                ]
            }
        }
    });
    let contact_msg = json!({
        "_": "message",
        "id": 3,
        "date": 1700000020,
        "from_id": {"_": "peerUser", "user_id": "7"},
        "message": "",
        "contact_vcard": "contacts/contact_1.vcard",
        "contact_vcard_file_size": 54,
        "media": {
            "_": "messageMediaContact",
            "first_name": "Nima",
            "last_name": "K.",
            "phone_number": "+442079460123",
            "vcard": "BEGIN:VCARD\nVERSION:3.0\nFN:Nima K.\nEND:VCARD\n"
        }
    });

    writer.message(&svc, &peers).unwrap();
    writer.message(&doc_msg, &peers).unwrap();
    writer.message(&contact_msg, &peers).unwrap();
    writer.finish().unwrap();

    let raw_json = std::fs::read_to_string(export_dir.join("result.json")).unwrap();
    assert!(!raw_json.contains("secret-hash"));
    assert!(!raw_json.contains("doc-secret"));
    assert!(!raw_json.contains("file_reference"));
    let parsed: Value = serde_json::from_str(&raw_json).unwrap();
    assert_eq!(parsed["messages"][0]["service_action"], "create_group");
    assert_eq!(parsed["messages"][0]["members"], json!(["Ali"]));
    assert_eq!(parsed["messages"][1]["media_type"], "video_file");
    assert_eq!(parsed["messages"][1]["file"], "video_files/2_100_clip.mp4");
    assert_eq!(
        parsed["messages"][1]["thumbnail"],
        "video_files/2_100_clip.mp4_thumb.jpg"
    );
    assert_eq!(parsed["messages"][1]["thumbnail_file_size"], 512);
    assert_eq!(
        parsed["messages"][2]["contact_vcard"],
        "contacts/contact_1.vcard"
    );
    assert_eq!(
        parsed["messages"][2]["contact_information"]["first_name"],
        "Nima"
    );

    let html = std::fs::read_to_string(export_dir.join("messages.html")).unwrap();
    assert!(html.contains("Sara created group &laquo;Core &lt;Team&gt;&raquo; with members Ali"));
    assert!(html.contains("src=\"video_files/2_100_clip.mp4_thumb.jpg\""));
    assert!(html.contains("href=\"contacts/contact_1.vcard\""));
    assert!(!html.contains("<Team>"));
}

#[test]
fn admin_writer_renders_before_after_bubbles_without_inventing_missing_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let options = settings(temp.path(), "html_and_json");
    let peers = BTreeMap::from([(
        "user:10".into(),
        json!({"_":"user","id":"10","first_name":"Sara"}),
    )]);
    let chat = LogChat::from_peers(55, &peers, 0);
    let edit_both = json!({
        "id": "901",
        "date": 1700000100,
        "user_id": "10",
        "action": {
            "_": "channelAdminLogEventActionEditMessage",
            "prev_message": {"_": "message", "id": 41, "date": 1700000050, "message": "Old text", "export_file": "photos/old.jpg"},
            "new_message": {"_": "message", "id": 41, "date": 1700000100, "message": "New text", "export_file": "C:\\private\\leak.jpg"}
        }
    });
    let unknown_ev = json!({
        "id": "902",
        "date": 1700000200,
        "user_id": "10",
        "action": {"_": "channelAdminLogEventActionFutureSpec"}
    });

    let mut writer = Writer::new_admin(
        temp.path(),
        &options,
        "Audit - Recent actions",
        &json!({"_":"inputPeerChannel","channel_id":"55"}),
    )
    .unwrap();
    for raw in [&edit_both, &unknown_ev] {
        let (entry, parts) = map_event_parts(raw, &chat);
        writer.admin_event(&entry, &parts, &peers).unwrap();
    }
    writer.finish().unwrap();

    let raw_json = std::fs::read_to_string(temp.path().join("result.json")).unwrap();
    assert!(!raw_json.contains("C:\\private"));
    assert!(!raw_json.contains("leak.jpg"));
    assert!(!raw_json.contains("telegram_tl"));
    let doc: Value = serde_json::from_str(&raw_json).unwrap();
    assert_eq!(doc["events"].as_array().unwrap().len(), 2);
    assert_eq!(doc["events"][0]["before"]["export_file"], "photos/old.jpg");
    assert!(doc["events"][0]["after"].get("export_file").is_none());

    let html = std::fs::read_to_string(temp.path().join("messages.html")).unwrap();
    assert!(html.contains("data-snapshot=\"after\""));
    assert!(html.contains("data-snapshot=\"before\""));
    assert!(html.contains("href=\"photos/old.jpg\""));
    assert!(!html.contains("leak.jpg"));
    assert!(html.contains("FutureSpec"));
}
