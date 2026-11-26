use super::*;

#[test]
fn export_round_trip_preserves_oldest_order_and_utf8() {
    if isolated("writer::export_round_trip_preserves_oldest_order_and_utf8") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let settings = settings("roundtrip");
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "chat",
            &json!({"chat_id":"@test"}),
            &settings,
            receiver,
            "integration-roundtrip",
        )
        .await
        .unwrap();
        assert_eq!(result["messages"], 103);
        assert_eq!(result["takeout"], true);
        let raw_json = std::fs::read_to_string(settings.path.join("result.json")).unwrap();
        assert!(!raw_json.contains("access_hash"));
        let value: Value = serde_json::from_str(&raw_json).unwrap();
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 103);
        assert_eq!(messages.first().unwrap()["id"], 98);
        assert_eq!(messages.last().unwrap()["id"], 200);
        assert_eq!(messages[0]["from_id"], "user9007199254740993");
        assert_eq!(messages[0]["from"], "Sara");
        assert_eq!(messages[0]["telegram_tl"]["new_layer_field"], 42);
        let html = std::fs::read_to_string(settings.path.join("messages.html")).unwrap();
        assert!(html.contains("سلام"));
        assert!(html.contains("😀"));
        assert!(!html.contains("<script>"));
        let requests = api.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .all(|(method, _)| !method.contains("readHistory"))
        );
        assert!(
            requests
                .iter()
                .any(|(method, params)| method == "invokeWithTakeout"
                    && params["query"]["_"] == "account.finishTakeoutSession"
                    && params["query"]["success"] == true)
        );
    });
}

#[test]
fn export_existing_destination_is_never_overwritten() {
    if isolated("writer::export_existing_destination_is_never_overwritten") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let settings = settings("existing");
        std::fs::create_dir(&settings.path).unwrap();
        std::fs::write(settings.path.join("result.json"), b"original owner data").unwrap();
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &Fixture::default(),
            "chat",
            &json!({"chat_id":"@test"}),
            &settings,
            receiver,
            "existing",
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(settings.path.join("result.json")).unwrap(),
            b"original owner data"
        );
    });
}

#[test]
fn recent_action_export_rejects_nonuser_admin_before_rpc() {
    if isolated("writer::recent_action_export_rejects_nonuser_admin_before_rpc") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let settings = settings("events-reject");
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "recent_actions",
            &json!({"chat_id":"@test","event_types":["deleted_messages"],"admins":["@admin"],"query":"search"}),
            &settings,
            receiver,
            "events-reject",
        )
        .await;
        assert!(
            result.is_err(),
            "An admin resolving to a channel must be refused before the network request"
        );
        assert!(api.requests.lock().unwrap().is_empty());
    });
}

#[test]
fn recent_action_export_preserves_deleted_message_and_filter() {
    if isolated("writer::recent_action_export_preserves_deleted_message_and_filter") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let settings = settings("events");
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "recent_actions",
            &json!({"chat_id":"@test","event_types":["deleted_messages"],"query":"search"}),
            &settings,
            receiver,
            "events",
        )
        .await
        .unwrap();
        assert_eq!(result["events"], 1);
        let raw_json = std::fs::read_to_string(settings.path.join("result.json")).unwrap();
        assert!(!raw_json.contains("telegram_tl"));
        assert!(!raw_json.contains("access_hash"));
        let data: Value = serde_json::from_str(&raw_json).unwrap();
        assert_eq!(data["name"], "تست <script> - Recent actions");
        assert_eq!(data["events"][0]["event_id"], "8007199254740999");
        assert_eq!(data["events"][0]["action_kind"], "delete_message");
        assert_eq!(data["events"][0]["after"]["text"], "Deleted text");
        assert_eq!(data["messages"][0]["type"], "service");
        assert!(
            data["messages"][0]["information_text"]
                .as_str()
                .unwrap()
                .contains("Sara deleted message")
        );
        assert_eq!(data["messages"][1]["text"], "Deleted text");
        let html = std::fs::read_to_string(settings.path.join("messages.html")).unwrap();
        assert!(html.contains("&lt;script&gt; - Recent actions"));
        assert!(html.contains("Sara deleted message:"));
        assert!(html.contains("Deleted text"));
        assert!(!html.contains("<script>"));
        let requests = api.requests.lock().unwrap();
        let (_, params) = requests
            .iter()
            .find(|(name, _)| name == "channels.getAdminLog")
            .unwrap();
        assert_eq!(params["events_filter"]["delete"], true);
        assert_eq!(params["q"], "search");
    });
}

#[test]
fn export_walks_split_ranges_and_migrated_history_and_hydrates_rich_blocks() {
    if isolated("writer::export_walks_split_ranges_and_migrated_history_and_hydrates_rich_blocks") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            complex: true,
            ..Default::default()
        };
        let settings = settings("split-migrated-rich");
        let (_sender, receiver) = tokio::sync::watch::channel(false);
        let result = export::run(
            &api,
            "chat",
            &json!({"chat_id":"@test"}),
            &settings,
            receiver,
            "complex",
        )
        .await
        .unwrap();
        assert_eq!(result["messages"], 4);
        let value: Value =
            serde_json::from_slice(&std::fs::read(settings.path.join("result.json")).unwrap())
                .unwrap();
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![-999999999, -999999998, 10, 11]
        );
        assert_eq!(messages[0]["source_peer"]["chat_id"], "42");
        assert_eq!(messages[0]["source_message_id"], 1);
        assert_eq!(messages[0]["rich_message"]["part"], false);
        assert!(
            std::fs::read_to_string(settings.path.join("messages.html"))
                .unwrap()
                .contains("Hydrated rich text")
        );
    });
}

#[derive(Clone, Default)]
struct AdminRichEventsFixture;

#[async_trait::async_trait]
impl Telegram for AdminRichEventsFixture {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        if method == "channels.getAdminLog" {
            if params["max_id"].as_str() != Some("0") {
                return Ok(json!({"events":[],"users":[],"chats":[]}));
            }
            return Ok(json!({
                "events": [
                    {
                        "id": "500",
                        "date": 1700000300,
                        "user_id": "10",
                        "action": {
                            "_": "channelAdminLogEventActionFutureUnknown99",
                            "raw": "<script>alert(9)</script>"
                        }
                    },
                    {
                        "id": "400",
                        "date": 1700000200,
                        "user_id": "10",
                        "action": {
                            "_": "channelAdminLogEventActionEditMessage",
                            "new_message": {"_": "message", "id": 88, "date": 1700000200, "message": "Edited without prev"}
                        }
                    },
                    {
                        "id": "300",
                        "date": 1700000100,
                        "user_id": "10",
                        "action": {
                            "_": "channelAdminLogEventActionEditMessage",
                            "prev_message": {
                                "_": "message",
                                "id": 77,
                                "from_id": {"_": "peerUser", "user_id": "20"},
                                "date": 1700000050,
                                "message": "Before <tag>",
                                "media": {"_": "messageMediaPhoto", "photo": {"_": "photo", "id": "301", "access_hash": "99", "file_reference": [1], "sizes": [{"_": "photoSize", "type": "x", "w": 64, "h": 64, "size": 24}]}}
                            },
                            "new_message": {
                                "_": "message",
                                "id": 77,
                                "from_id": {"_": "peerUser", "user_id": "20"},
                                "date": 1700000100,
                                "message": "After <tag>",
                                "media": {"_": "messageMediaPhoto", "photo": {"_": "photo", "id": "302", "access_hash": "99", "file_reference": [2], "sizes": [{"_": "photoSize", "type": "x", "w": 64, "h": 64, "size": 24}]}}
                            }
                        }
                    },
                    {
                        "id": "200",
                        "date": 1700000050,
                        "user_id": "10",
                        "action": {
                            "_": "channelAdminLogEventActionChangeHistoryTTL",
                            "prev_value": 0,
                            "new_value": 86400
                        }
                    }
                ],
                "users": [
                    {"_": "user", "id": "10", "first_name": "Sara"},
                    {"_": "user", "id": "20", "first_name": "Ali"}
                ],
                "chats": [{"_": "channel", "id": "12", "title": "Ops Group", "megagroup": true}]
            }));
        }
        bail!("Unexpected method: {method}")
    }

    async fn resolve_peer(&self, _input: &Value) -> Result<Value> {
        Ok(json!({"_":"inputPeerChannel","channel_id":"12","access_hash":"34"}))
    }

    async fn download(
        &self,
        _location: Value,
        destination: &std::path::Path,
        _max_bytes: u64,
    ) -> Result<Value> {
        std::fs::write(destination, b"photo-bytes")?;
        Ok(json!({"path": destination, "bytes": 11}))
    }

    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        Some(Arc::new(self.clone()))
    }
}

#[test]
fn recent_actions_export_renders_chronological_bubbles_diffs_unknown_fallback_and_relative_media() {
    if isolated(
        "writer::recent_actions_export_renders_chronological_bubbles_diffs_unknown_fallback_and_relative_media",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let root = std::path::PathBuf::from(std::env::var("TEST_EXPORT_ROOT").unwrap());
        let empty_chosen = root.join("chosen-empty");
        std::fs::create_dir(&empty_chosen).unwrap();
        let args = json!({
            "chat_id": "@ops",
            "format": "html_and_json",
            "photos": true,
            "videos": false,
            "voice_messages": false,
            "video_messages": false,
            "stickers": false,
            "gifs": false,
            "files": true,
            "size_limit_mb": 8,
            "destination": empty_chosen,
        });
        let first_settings = Settings::parse(&args).unwrap();
        assert_eq!(
            first_settings.path,
            empty_chosen.canonicalize().unwrap(),
            "An explicit empty destination folder must receive output directly"
        );
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let res = export::run(
            &AdminRichEventsFixture,
            "recent_actions",
            &args,
            &first_settings,
            rx,
            "admin-full",
        )
        .await
        .unwrap();
        assert_eq!(res["events"], 4);
        assert_eq!(res["files"], 2);
        assert!(empty_chosen.join("result.json").is_file());
        assert!(empty_chosen.join("messages.html").is_file());

        let raw_json = std::fs::read_to_string(empty_chosen.join("result.json")).unwrap();
        assert!(!raw_json.contains("telegram_tl"));
        assert!(!raw_json.contains("access_hash"));
        assert!(!raw_json.contains("file_reference"));
        let doc: Value = serde_json::from_str(&raw_json).unwrap();
        assert_eq!(doc["name"], "Ops Group - Recent actions");
        let events = doc["events"].as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e["event_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["200", "300", "400", "500"]
        );
        assert_eq!(events[0]["action_kind"], "change_history_ttl");
        assert_eq!(
            events[0]["field_changes"][0]["field"],
            "history_ttl_seconds"
        );
        assert_eq!(events[1]["before"]["text"], "Before <tag>");
        assert_eq!(events[1]["after"]["text"], "After <tag>");
        assert!(
            events[1]["before"]["export_file"]
                .as_str()
                .unwrap()
                .starts_with("photos/")
        );
        assert!(
            events[1]["after"]["export_file"]
                .as_str()
                .unwrap()
                .starts_with("photos/")
        );
        assert!(
            events[2].get("before").is_none(),
            "Absent prev_message must not invent a before snapshot"
        );
        assert_eq!(events[2]["after"]["text"], "Edited without prev");
        assert!(
            events[3]["action_kind"]
                .as_str()
                .unwrap()
                .contains("FutureUnknown99")
        );

        let html = std::fs::read_to_string(empty_chosen.join("messages.html")).unwrap();
        assert!(html.contains("Ops Group - Recent actions"));
        assert!(html.contains("data-snapshot=\"after\""));
        assert!(html.contains("data-snapshot=\"before\""));
        assert!(html.contains("Before &lt;tag&gt;"));
        assert!(html.contains("After &lt;tag&gt;"));
        assert!(!html.contains("<script>"));
        assert_eq!(html.matches("data-snapshot=\"before\"").count(), 1);

        let second_settings = Settings::parse(&args).unwrap();
        assert_ne!(second_settings.path, first_settings.path);
        assert!(
            second_settings
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ChatExport_"),
            "Once destination is occupied, next export must select a fresh ChatExport_ child"
        );
    });
}
