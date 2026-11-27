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
fn document_exports_preserve_file_sizes_from_decoded_tl_longs() {
    let schema = crate::tl::Schema::global();
    for size in [0_u64, 2048, 5_368_709_120, 9_007_199_254_740_993] {
        let document = json!({
            "_":"document","id":"7","access_hash":"9","file_reference":[],
            "date":1700000000,"mime_type":"application/octet-stream","size":size.to_string(),"dc_id":2,
            "attributes":[{"_":"documentAttributeFilename","file_name":"report.bin"}]
        });
        let decoded = schema
            .decode("Document", &schema.encode_object(&document).unwrap())
            .unwrap();
        assert_eq!(decoded["size"], size.to_string());
        let record = crate::export::render::json_message(
            &json!({"_":"message","id":1,"date":1700000000,"media":{"_":"messageMediaDocument","document":decoded}}),
            &BTreeMap::new(),
        ).unwrap();
        assert_eq!(record["file_size"], size);
    }
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

#[test]
fn admin_writer_preserves_sender_namespaces_in_before_and_after_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let peers = BTreeMap::from([
        (
            "user:9".into(),
            json!({"_":"user","id":"9","first_name":"Moderator"}),
        ),
        (
            "user:55".into(),
            json!({"_":"user","id":"55","first_name":"Alice"}),
        ),
        (
            "channel:55".into(),
            json!({"_":"channel","id":"55","title":"Channel author"}),
        ),
        (
            "user:56".into(),
            json!({"_":"user","id":"56","first_name":"Unrelated user"}),
        ),
    ]);
    let chat = LogChat::from_peers(55, &peers, 0);
    let mut writer = Writer::new_admin(
        temp.path(),
        &settings(temp.path(), "html_and_json"),
        "Sender identity",
        &json!({"_":"inputPeerChannel","channel_id":"55"}),
    )
    .unwrap();
    for (id, old_sender, new_sender) in [
        (
            1,
            json!({"_":"peerChannel","channel_id":"55"}),
            json!({"_":"peerUser","user_id":"55"}),
        ),
        (2, json!({"_":"peerChannel","channel_id":"56"}), Value::Null),
    ] {
        let raw = json!({
            "id":id,"date":1700000100,"user_id":"9",
            "action":{"_":"channelAdminLogEventActionEditMessage",
                "prev_message":{"_":"message","id":id,"date":1700000000,"from_id":old_sender,"peer_id":{"_":"peerChannel","channel_id":"55"},"message":"Before"},
                "new_message":{"_":"message","id":id,"date":1700000100,"from_id":new_sender,"peer_id":{"_":"peerChannel","channel_id":"55"},"message":"After"}}
        });
        let (entry, parts) = map_event_parts(&raw, &chat);
        writer.admin_event(&entry, &parts, &peers).unwrap();
    }
    writer.finish().unwrap();
    let doc: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("result.json")).unwrap()).unwrap();
    let snapshot = |event: &str, role: &str| {
        doc["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["event_id"] == event && m["snapshot_role"] == role)
            .unwrap()
    };
    assert_eq!(snapshot("1", "before")["from_id"], "channel55");
    assert_eq!(snapshot("1", "before")["from"], "Channel author");
    assert_eq!(snapshot("1", "after")["from_id"], "user55");
    assert_eq!(snapshot("1", "after")["from"], "Alice");
    assert_eq!(snapshot("2", "before")["from_id"], "channel56");
    assert_eq!(snapshot("2", "before")["from"], "channel56");
    assert_eq!(snapshot("2", "after")["from_id"], "channel55");
    let html = std::fs::read_to_string(temp.path().join("messages.html")).unwrap();
    assert!(
        html.contains("data-event-id=\"1\" data-snapshot=\"before\"><header><b>Channel author</b>")
    );
    assert!(html.contains("data-event-id=\"1\" data-snapshot=\"after\"><header><b>Alice</b>"));
    assert!(!html.contains("Unrelated user"));
}

#[test]
fn admin_writer_rotates_before_an_event_crosses_the_page_article_limit() {
    let temp = tempfile::tempdir().unwrap();
    let peers = BTreeMap::new();
    let chat = LogChat::from_peers(55, &peers, 0);
    let mut writer = Writer::new_admin(
        temp.path(),
        &settings(temp.path(), "html"),
        "Admin pages",
        &json!({"_":"inputPeerChannel","channel_id":"55"}),
    )
    .unwrap();
    for id in 1..=2000 {
        let action = if id == 1000 {
            json!({"_":"channelAdminLogEventActionEditMessage",
                "prev_message":{"_":"message","id":7,"message":"Before"},
                "new_message":{"_":"message","id":7,"message":"After"}})
        } else {
            json!({"_":"channelAdminLogEventActionChangeTitle","prev_value":"Old","new_value":"New"})
        };
        let raw = json!({"id":id,"date":1700000000,"user_id":"9","action":action});
        let (entry, parts) = map_event_parts(&raw, &chat);
        writer.admin_event(&entry, &parts, &peers).unwrap();
    }
    writer.finish().unwrap();
    let first = std::fs::read_to_string(temp.path().join("messages.html")).unwrap();
    assert_eq!(first.matches("<article").count(), 999);
    let second = std::fs::read_to_string(temp.path().join("messages2.html")).unwrap();
    let third = std::fs::read_to_string(temp.path().join("messages3.html")).unwrap();
    assert_eq!(second.matches("<article").count(), 1000);
    assert_eq!(third.matches("<article").count(), 3);
    assert!(second.contains("data-event-id=\"1000\" data-snapshot=\"before\""));
    assert!(second.contains("data-event-id=\"1000\" data-snapshot=\"after\""));
    assert!(first.contains("href=\"messages2.html\">Next messages"));
    assert!(second.contains("href=\"messages3.html\">Next messages"));
    assert!(third.contains("href=\"messages2.html\">Previous messages"));
}

#[test]
fn admin_log_actions_and_writers_cover_all_desktop_event_variants() {
    let temp = tempfile::tempdir().unwrap();
    let options = settings(temp.path(), "html_and_json");
    let peers = BTreeMap::from([
        (
            "user:1".into(),
            json!({"_":"user","id":"1","first_name":"Ada","username":"ada"}),
        ),
        (
            "user:2".into(),
            json!({"_":"user","id":"2","first_name":"Bob","username":"bob"}),
        ),
        (
            "channel:55".into(),
            json!({"_":"channel","id":"55","title":"Demo","username":"demo_ch","broadcast":true}),
        ),
    ]);
    let mut group_peers = peers.clone();
    group_peers.insert(
        "channel:55".into(),
        json!({"_":"channel","id":"55","title":"Demo","username":"demo_ch","megagroup":true}),
    );
    let sample_msg = json!({
        "_": "message",
        "id": 42,
        "date": 1700000100,
        "from_id": {"_": "peerUser", "user_id": "2"},
        "message": "hello https://example.com",
        "entities": [
            {"_": "messageEntityBold", "offset": 0, "length": 5},
            {"_": "messageEntityTextUrl", "offset": 6, "length": 19, "url": "https://example.com"},
            {"_": "messageEntityItalic", "offset": 0, "length": 2},
            {"_": "messageEntityCode", "offset": 2, "length": 2},
            {"_": "messageEntityPre", "offset": 0, "length": 1, "language": "rust"},
            {"_": "messageEntityCustomEmoji", "offset": 0, "length": 1, "document_id": "999"}
        ],
        "pinned": true,
        "export_file": "photos/item.jpg"
    });
    let prev_msg = json!({
        "_": "message",
        "id": 42,
        "date": 1700000050,
        "from_id": {"_": "peerUser", "user_id": "2"},
        "message": "before text",
        "pinned": false
    });
    let admin_rights = json!({"_": "chatAdminRights", "change_info": true, "post_messages": true, "delete_messages": true, "ban_users": true, "invite_users": true, "pin_messages": true, "manage_call": true, "other": true});
    let banned_rights = json!({"_": "chatBannedRights", "view_messages": true, "send_messages": true, "send_media": true, "until_date": 1700500000});
    let invite = json!({"_": "chatInviteExported", "link": "https://t.me/+abc123", "title": "VIP", "usage_limit": 5, "expire_date": 1700900000, "request_needed": true});
    let topic_prev = json!({"_": "forumTopic", "id": 7, "title": "OldTopic", "icon_emoji_id": "111", "closed": false, "hidden": false});
    let topic_new = json!({"_": "forumTopic", "id": 7, "title": "NewTopic", "icon_emoji_id": "222", "closed": true, "hidden": true});
    let actions = vec![
        json!({"_": "channelAdminLogEventActionChangeTitle", "prev_value": "Old", "new_value": "New"}),
        json!({"_": "channelAdminLogEventActionChangeAbout", "prev_value": "A", "new_value": "B"}),
        json!({"_": "channelAdminLogEventActionChangeUsername", "prev_value": "old_ch", "new_value": "new_ch"}),
        json!({"_": "channelAdminLogEventActionChangeUsernames", "prev_value": ["a", "b"], "new_value": ["b", "c"]}),
        json!({"_": "channelAdminLogEventActionChangePhoto", "prev_photo": {"_": "photoEmpty"}, "new_photo": {"_": "photo", "id": "55"}}),
        json!({"_": "channelAdminLogEventActionChangePhoto", "prev_photo": {"_": "photo", "id": "55"}, "new_photo": {"_": "photoEmpty"}}),
        json!({"_": "channelAdminLogEventActionToggleInvites", "new_value": true}),
        json!({"_": "channelAdminLogEventActionToggleSignatures", "new_value": true}),
        json!({"_": "channelAdminLogEventActionToggleSignatureProfiles", "new_value": true}),
        json!({"_": "channelAdminLogEventActionUpdatePinned", "message": sample_msg}),
        json!({"_": "channelAdminLogEventActionUpdatePinned", "message": prev_msg}),
        json!({"_": "channelAdminLogEventActionEditMessage", "prev_message": prev_msg, "new_message": sample_msg}),
        json!({"_": "channelAdminLogEventActionDeleteMessage", "message": sample_msg}),
        json!({"_": "channelAdminLogEventActionStopPoll", "message": sample_msg}),
        json!({"_": "channelAdminLogEventActionSendMessage", "message": sample_msg}),
        json!({"_": "channelAdminLogEventActionParticipantJoin"}),
        json!({"_": "channelAdminLogEventActionParticipantLeave"}),
        json!({"_": "channelAdminLogEventActionParticipantInvite", "participant": {"_": "channelParticipant", "user_id": "2"}}),
        json!({"_": "channelAdminLogEventActionParticipantJoinByInvite", "invite": invite, "via_chatlist": true}),
        json!({"_": "channelAdminLogEventActionParticipantJoinByRequest", "invite": invite, "approved_by": "1"}),
        json!({"_": "channelAdminLogEventActionParticipantToggleBan", "prev_participant": {"_": "channelParticipant", "user_id": "2"}, "new_participant": {"_": "channelParticipantBanned", "peer": {"_": "peerUser", "user_id": "2"}, "banned_rights": banned_rights}}),
        json!({"_": "channelAdminLogEventActionParticipantToggleAdmin", "prev_participant": {"_": "channelParticipant", "user_id": "2"}, "new_participant": {"_": "channelParticipantAdmin", "user_id": "2", "admin_rights": admin_rights, "rank": "Mod"}}),
        json!({"_": "channelAdminLogEventActionParticipantEditRank", "user_id": "2", "prev_rank": "Mod", "new_rank": "Lead"}),
        json!({"_": "channelAdminLogEventActionChangeStickerSet", "prev_stickerset": {"_": "inputStickerSetEmpty"}, "new_stickerset": {"_": "inputStickerSetShortName", "short_name": "pack1"}}),
        json!({"_": "channelAdminLogEventActionChangeEmojiStickerSet", "prev_stickerset": {"_": "inputStickerSetShortName", "short_name": "e1"}, "new_stickerset": {"_": "inputStickerSetEmpty"}}),
        json!({"_": "channelAdminLogEventActionTogglePreHistoryHidden", "new_value": true}),
        json!({"_": "channelAdminLogEventActionDefaultBannedRights", "prev_banned_rights": {"_": "chatBannedRights"}, "new_banned_rights": banned_rights}),
        json!({"_": "channelAdminLogEventActionChangeLinkedChat", "prev_value": "0", "new_value": "88"}),
        json!({"_": "channelAdminLogEventActionChangeLocation", "prev_value": {"_": "channelLocationEmpty"}, "new_value": {"_": "channelLocation", "address": "123 Main St"}}),
        json!({"_": "channelAdminLogEventActionToggleSlowMode", "prev_value": 0, "new_value": 30}),
        json!({"_": "channelAdminLogEventActionStartGroupCall", "call": {"_": "inputGroupCall", "id": "77"}}),
        json!({"_": "channelAdminLogEventActionDiscardGroupCall", "call": {"_": "inputGroupCall", "id": "77"}}),
        json!({"_": "channelAdminLogEventActionParticipantMute", "participant": {"_": "groupCallParticipant", "peer": {"_": "peerUser", "user_id": "2"}}}),
        json!({"_": "channelAdminLogEventActionParticipantUnmute", "participant": {"_": "groupCallParticipant", "peer": {"_": "peerUser", "user_id": "2"}}}),
        json!({"_": "channelAdminLogEventActionToggleGroupCallSetting", "join_muted": true}),
        json!({"_": "channelAdminLogEventActionParticipantVolume", "participant": {"_": "groupCallParticipant", "peer": {"_": "peerUser", "user_id": "2"}, "volume": 15000}}),
        json!({"_": "channelAdminLogEventActionExportedInviteDelete", "invite": invite}),
        json!({"_": "channelAdminLogEventActionExportedInviteRevoke", "invite": invite}),
        json!({"_": "channelAdminLogEventActionExportedInviteEdit", "prev_invite": invite, "new_invite": {"_": "chatInviteExported", "link": "https://t.me/+abc123", "title": "VIP2", "usage_limit": 10}}),
        json!({"_": "channelAdminLogEventActionChangeHistoryTTL", "prev_value": 0, "new_value": 86400}),
        json!({"_": "channelAdminLogEventActionToggleNoForwards", "new_value": true}),
        json!({"_": "channelAdminLogEventActionChangeAvailableReactions", "prev_value": {"_": "chatReactionsNone"}, "new_value": {"_": "chatReactionsAll"}}),
        json!({"_": "channelAdminLogEventActionToggleForum", "new_value": true}),
        json!({"_": "channelAdminLogEventActionCreateTopic", "topic": topic_prev}),
        json!({"_": "channelAdminLogEventActionEditTopic", "prev_topic": topic_prev, "new_topic": topic_new}),
        json!({"_": "channelAdminLogEventActionDeleteTopic", "topic": topic_new}),
        json!({"_": "channelAdminLogEventActionToggleAntiSpam", "new_value": true}),
        json!({"_": "channelAdminLogEventActionChangePeerColor", "prev_value": {"_": "peerColor", "color": 1}, "new_value": {"_": "peerColor", "color": 3, "background_emoji_id": "555"}}),
        json!({"_": "channelAdminLogEventActionChangeProfilePeerColor", "prev_value": {"_": "peerColor", "color": 2}, "new_value": {"_": "peerColor", "color": 4}}),
        json!({"_": "channelAdminLogEventActionChangeWallpaper", "prev_value": {"_": "wallPaperNoFile", "id": "1"}, "new_value": {"_": "wallPaper", "slug": "paper"}}),
        json!({"_": "channelAdminLogEventActionChangeEmojiStatus", "prev_value": {"_": "emojiStatusEmpty"}, "new_value": {"_": "emojiStatus", "document_id": "777", "until": 1700999999}}),
        json!({"_": "channelAdminLogEventActionToggleAutotranslation", "new_value": true}),
        json!({"_": "channelAdminLogEventActionParticipantSubExtend", "prev_participant": {"_": "channelParticipant", "user_id": "2"}, "new_participant": {"_": "channelParticipant", "user_id": "2", "subscription_until_date": 1701000000}}),
    ];

    let mut writer = Writer::new_admin(
        temp.path(),
        &options,
        "Full Recent Actions",
        &json!({"_":"inputPeerChannel","channel_id":"55"}),
    )
    .unwrap();
    for (idx, action) in actions.iter().enumerate() {
        let chat = if idx % 2 == 0 {
            LogChat::from_peers(55, &peers, 0)
        } else {
            LogChat::from_peers(55, &group_peers, 0)
        };
        let raw = json!({
            "id": (1000 + idx) as i64,
            "date": 1700000000 + idx as i64,
            "user_id": "1",
            "action": action,
        });
        let (entry, parts) = map_event_parts(&raw, &chat);
        assert!(!entry.summary.is_empty());
        writer.admin_event(&entry, &parts, &peers).unwrap();
    }
    writer.finish().unwrap();

    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(temp.path().join("result.json")).unwrap())
            .unwrap();
    assert_eq!(doc["events"].as_array().unwrap().len(), actions.len());
    let html = std::fs::read_to_string(temp.path().join("messages.html")).unwrap();
    assert!(html.contains("Full Recent Actions"));
    assert!(html.contains("NewTopic"));
}

#[test]
fn service_projection_covers_all_desktop_service_actions() {
    use crate::export::service::project_service_action;
    let peers = BTreeMap::from([
        (
            "user:1".into(),
            json!({"_":"user","id":"1","first_name":"Ada"}),
        ),
        (
            "user:2".into(),
            json!({"_":"user","id":"2","first_name":"Bob"}),
        ),
    ]);
    let actions = vec![
        json!({"_": "messageActionChatCreate", "title": "Crew", "users": ["1", "2"]}),
        json!({"_": "messageActionChannelCreate", "title": "News"}),
        json!({"_": "messageActionChatEditTitle", "title": "Renamed"}),
        json!({"_": "messageActionChatEditPhoto", "photo": {"_": "photo", "id": "7"}}),
        json!({"_": "messageActionChatDeletePhoto"}),
        json!({"_": "messageActionChatAddUser", "users": ["2"]}),
        json!({"_": "messageActionChatDeleteUser", "user_id": "2"}),
        json!({"_": "messageActionChatJoinedByLink", "inviter_id": "1"}),
        json!({"_": "messageActionChatJoinedByRequest"}),
        json!({"_": "messageActionChatMigrateTo", "channel_id": "99"}),
        json!({"_": "messageActionChannelMigrateFrom", "title": "OldGroup", "chat_id": "12"}),
        json!({"_": "messageActionPinMessage"}),
        json!({"_": "messageActionHistoryClear"}),
        json!({"_": "messageActionGameScore", "game_id": "5", "score": 99}),
        json!({"_": "messageActionPhoneCall", "call_id": "3", "duration": 65, "video": true, "reason": {"_": "phoneCallDiscardReasonHangup"}}),
        json!({"_": "messageActionPhoneCall", "call_id": "4", "reason": {"_": "phoneCallDiscardReasonMissed"}}),
        json!({"_": "messageActionScreenshotTaken"}),
        json!({"_": "messageActionCustomAction", "message": "Custom <notice>"}),
        json!({"_": "messageActionContactSignUp"}),
        json!({"_": "messageActionSetMessagesTTL", "period": 86400}),
        json!({"_": "messageActionSetMessagesTTL", "period": 3600}),
        json!({"_": "messageActionSetMessagesTTL", "period": 0}),
        json!({"_": "messageActionInviteToGroupCall", "users": ["2"]}),
        json!({"_": "messageActionTopicCreate", "title": "General"}),
        json!({"_": "messageActionTopicEdit", "title": "General 2", "closed": true, "hidden": false}),
        json!({"_": "messageActionSuggestProfilePhoto"}),
        json!({"_": "messageActionUnknown"}),
    ];
    for (idx, action) in actions.iter().enumerate() {
        let msg = if idx % 2 == 0 {
            json!({"_": "messageService", "id": idx + 1, "peer_id": {"_": "peerChannel", "channel_id": "55"}, "reply_to": {"reply_to_msg_id": 10}, "export_file": "photos/pic.jpg", "action": action})
        } else {
            json!({"_": "messageService", "id": idx + 1, "peer_id": {"_": "peerChat", "chat_id": "12"}, "from_id": {"_": "peerUser", "user_id": "1"}, "action": action})
        };
        let proj = project_service_action(&msg, &peers, "Ada").unwrap();
        assert!(!proj.summary_text.is_empty());
        assert!(!proj.html_body.contains("<notice>"));
    }
}

#[test]
fn dual_writer_main_file_path_slice_and_finish_semantics_match_desktop() {
    let temp_both = tempfile::tempdir().unwrap();
    let opts_both = settings(temp_both.path(), "html_and_json");
    let mut writer = Writer::new(
        temp_both.path(),
        &opts_both,
        "Dual Slice",
        &json!({"_":"inputPeerUser","user_id":"10"}),
    )
    .unwrap();
    assert_eq!(
        writer.main_file_path(),
        temp_both.path().join("messages.html")
    );
    writer
        .write_dialog_slice(&[message(1), message(2)], &BTreeMap::new())
        .unwrap();
    writer.finish().unwrap();
    assert!(temp_both.path().join("messages.html").exists());
    assert!(temp_both.path().join("result.json").exists());

    let temp_json = tempfile::tempdir().unwrap();
    let opts_json = settings(temp_json.path(), "json");
    let writer_json = Writer::new(
        temp_json.path(),
        &opts_json,
        "Json Only",
        &json!({"_":"inputPeerUser","user_id":"11"}),
    )
    .unwrap();
    assert_eq!(
        writer_json.main_file_path(),
        temp_json.path().join("result.json")
    );
    writer_json.finish().unwrap();
}
