//! End-to-end export proofs use an isolated process for configuration, avoiding
//! process-wide environment mutation in Rust's parallel test runner.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use telegram_mcp::backend::Telegram;
use telegram_mcp::export::media;
use telegram_mcp::export::{self, settings::Settings};
#[path = "rust_visual_export/access.rs"]
mod access;
#[path = "rust_visual_export/media.rs"]
mod media_edges;
#[path = "rust_visual_export/migrated_replies.rs"]
mod migrated_replies;
#[path = "rust_visual_export/settings.rs"]
mod settings_edges;
#[path = "rust_visual_export/upstream.rs"]
mod upstream;

#[derive(Clone, Default)]
struct Fixture {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    block: bool,
    fail: bool,
    complex: bool,
    short_page: bool,
    admin_partial: bool,
    entered: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Telegram for Fixture {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        self.requests
            .lock()
            .unwrap()
            .push((method.into(), params.clone()));
        if self.block {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        if self.fail {
            bail!("Synthetic export failure");
        }
        if method == "account.initTakeoutSession" {
            return Ok(json!({"_":"account.takeout","id":"5"}));
        }
        let (mut method, mut params) = (method, &params);
        let mut range_min = 0;
        while ["invokeWithTakeout", "invokeWithMessagesRange"].contains(&method) {
            if method == "invokeWithMessagesRange" {
                range_min = params["range"]["min_id"].as_i64().unwrap();
            }
            method = params["query"]["_"].as_str().unwrap();
            params = &params["query"];
        }
        if method == "account.finishTakeoutSession" {
            return Ok(true.into());
        }
        if method == "messages.getSplitRanges" {
            return Ok(if self.complex {
                json!([{"_":"messageRange","min_id":1,"max_id":100},{"_":"messageRange","min_id":101,"max_id":200}])
            } else {
                json!([])
            });
        }
        if method == "channels.getFullChannel" {
            return Ok(if self.complex {
                json!({"full_chat":{"migrated_from_chat_id":"42"}})
            } else {
                json!({"full_chat":{}})
            });
        }
        if method == "messages.getRichMessage" {
            return Ok(
                json!({"messages":[{"id":params["id"],"rich_message":{"_":"richMessage","part":false,"blocks":[{"_":"pageBlockParagraph","text":{"_":"textPlain","text":"Hydrated rich text"}}],"photos":[],"documents":[]}}]}),
            );
        }
        if method == "messages.getCustomEmojiDocuments" {
            return Ok(params["document_id"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| {
                    let mut doc = document_message(10)["media"]["document"].clone();
                    doc["id"] = id.clone();
                    doc["attributes"] = json!([]);
                    doc
                })
                .collect());
        }
        if method == "messages.getHistory" {
            let offset = params["offset_id"].as_i64().unwrap();
            let ids: Vec<i64> = if self.short_page {
                match offset {
                    0 => vec![9],
                    9 => vec![8],
                    _ => vec![],
                }
            } else if self.complex {
                if offset != 0 {
                    vec![]
                } else {
                    vec![
                        if params["peer"]["_"] == "inputPeerChat" {
                            1
                        } else {
                            10
                        } + i64::from(range_min > 100),
                    ]
                }
            } else if offset == 0 {
                (101..=200).rev().collect()
            } else if offset == 101 {
                (98..=100).rev().collect()
            } else {
                vec![]
            };
            let messages:Vec<Value>=ids.into_iter().map(|id|{
                let mut message=json!({"_":"message","id":id,"date":1700000000,
                "message":format!("سلام 😀 message {id}"),"from_id":{"_":"peerUser","user_id":"9007199254740993"},
                "entities":[{"_":"messageEntityBold","offset":8,"length":7}],"new_layer_field":42});
                if self.complex{message["rich_message"]=json!({"_":"richMessage","part":true});}
                message
            }).collect();
            return Ok(
                json!({"_":"messages.messages","messages":messages,"users":[{"_":"user","id":"9007199254740993","first_name":"Sara","last_name":null}],"chats":[{"id":"12","title":"تست <script>"}]}),
            );
        }
        if method == "channels.getAdminLog" {
            if params["max_id"].as_str() != Some("0") {
                return Ok(json!({"events":[],"users":[],"chats":[]}));
            }
            let mut response = json!({"events":[{"id":"8007199254740999","date":1700000000,"user_id":"9007199254740993",
                "action":{"_":"channelAdminLogEventActionDeleteMessage","message":{"_":"message","id":12,"date":1700000000,"message":"Deleted text"}}}],"users":[],"chats":[]});
            if self.admin_partial {
                response["events"][0]["action"]["message"]["rich_message"] = json!({"part":true});
            }
            return Ok(response);
        }
        bail!("Unexpected fixture method: {method}")
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        if input.is_object() {
            return Ok(input.clone());
        }
        Ok(json!({"_":"inputPeerChannel","channel_id":"12","access_hash":"34"}))
    }
    async fn download(
        &self,
        location: Value,
        destination: &std::path::Path,
        max_bytes: u64,
    ) -> Result<Value> {
        self.requests.lock().unwrap().push((
            "fixture.download".into(),
            json!({"location":location,"max_bytes":max_bytes}),
        ));
        if self.fail {
            bail!("Synthetic expired reference");
        }
        std::fs::write(destination, b"downloaded fixture bytes")?;
        Ok(json!({"path":destination,"bytes":24}))
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        Some(Arc::new(self.clone()))
    }
}

fn settings(folder: &str) -> Settings {
    Settings {
        format: "html_and_json".into(),
        media: [false; 7],
        max_bytes: 8 * 1024 * 1024,
        from: 0,
        until: 0,
        path: std::path::PathBuf::from(std::env::var("TEST_EXPORT_ROOT").unwrap()).join(folder),
    }
}

fn isolated(name: &str) -> bool {
    isolated_with_allowlist(name, None)
}

fn isolated_with_allowlist(name: &str, allowlist: Option<&str>) -> bool {
    if std::env::var("TEST_EXPORT_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    if let Some(allowlist) = allowlist {
        command.env("TELEGRAM_ALLOWED_CHAT_IDS", allowlist);
    } else {
        command.env_remove("TELEGRAM_ALLOWED_CHAT_IDS");
    }
    let mut child = command
        .args(["--exact", name, "--nocapture"])
        .env("TEST_EXPORT_CHILD", name)
        .env("TEST_EXPORT_ROOT", &files)
        .env("TELEGRAM_STATE_DIR", temp.path().join("state"))
        .env(
            "TELEGRAM_FILE_ROOTS",
            serde_json::to_string(&vec![files.to_str().unwrap()]).unwrap(),
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Isolated export test exceeded its deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn export_round_trip_preserves_oldest_order_and_utf8() {
    if isolated("export_round_trip_preserves_oldest_order_and_utf8") {
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
        let value: Value =
            serde_json::from_slice(&std::fs::read(settings.path.join("result.json")).unwrap())
                .unwrap();
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
fn export_cancel_interrupts_pending_request_and_reports_cancelled() {
    if isolated("export_cancel_interrupts_pending_request_and_reports_cancelled") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            block: true,
            ..Default::default()
        };
        let settings = settings("cancel");
        let folder = settings.path.clone();
        let entered = api.entered.clone();
        let job = export::jobs::start(Arc::new(api), "chat", json!({"chat_id":"@test"}), settings)
            .unwrap();
        assert_eq!(job["status"], "running");
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        let cancelled = export::jobs::cancel(&json!({"job_id":job["job_id"]})).unwrap();
        assert_eq!(cancelled["cancelled"], true);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let status = export::jobs::status(&json!({"job_id":job["job_id"]})).unwrap();
                if status["status"] == "cancelled" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !folder.exists(),
            "A cancelled pending takeout initialization must not create export output"
        );
        assert_eq!(
            export::jobs::cancel(&json!({"job_id":job["job_id"]})).unwrap()["cancelled"],
            false
        );
    });
}

#[test]
fn export_failed_job_reports_failed_without_success_result() {
    if isolated("export_failed_job_reports_failed_without_success_result") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            fail: true,
            ..Default::default()
        };
        let settings = settings("fail");
        let job = export::jobs::start(Arc::new(api), "chat", json!({"chat_id":"@test"}), settings)
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let status = export::jobs::status(&json!({"job_id":job["job_id"]})).unwrap();
                if status["status"] == "failed" {
                    assert!(status.get("result").is_none());
                    assert!(status["error"].as_str().unwrap().contains("request failed"));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    });
}

#[test]
fn export_existing_destination_is_never_overwritten() {
    if isolated("export_existing_destination_is_never_overwritten") {
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
    if isolated("recent_action_export_rejects_nonuser_admin_before_rpc") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api=Fixture::default();let settings=settings("events");let (_sender,receiver)=tokio::sync::watch::channel(false);
        let result=export::run(&api,"recent_actions",&json!({"chat_id":"@test","event_types":["deleted_messages"],"admins":["@admin"],"query":"search"}),&settings,receiver,"events").await;
        assert!(result.is_err(),"An admin resolving to a channel must be refused before the network request");
        assert!(api.requests.lock().unwrap().is_empty());
    });
}

#[test]
fn recent_action_export_preserves_deleted_message_and_filter() {
    if isolated("recent_action_export_preserves_deleted_message_and_filter") {
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
        let data: Value =
            serde_json::from_slice(&std::fs::read(settings.path.join("result.json")).unwrap())
                .unwrap();
        assert_eq!(
            data["messages"][0]["telegram_tl"]["admin_log_event"]["action"]["message"]["message"],
            "Deleted text"
        );
        assert_eq!(data["messages"][0]["id"], "8007199254740999");
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
fn export_unknown_job_is_an_explicit_error() {
    assert!(
        export::jobs::status(&json!({"job_id":"unknown-native-export"}))
            .unwrap_err()
            .to_string()
            .contains("No export")
    );
    assert_eq!(
        export::jobs::cancel(&json!({"job_id":"unknown-native-export"})).unwrap()["cancelled"],
        false
    );
}

#[test]
fn export_walks_split_ranges_and_migrated_history_and_hydrates_rich_blocks() {
    if isolated("export_walks_split_ranges_and_migrated_history_and_hydrates_rich_blocks") {
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

fn document_message(size: u64) -> Value {
    json!({"id":99,"out":false,"media":{"_":"messageMediaDocument","document":{"_":"document","id":"9007199254740993","access_hash":"44","file_reference":[1,2,3],"dc_id":3,"size":size,
        "attributes":[{"_":"documentAttributeFilename","file_name":"../report.txt"}]}}})
}

#[test]
fn export_media_policy_rejects_unselected_and_oversized_files_before_download() {
    if isolated("export_media_policy_rejects_unselected_and_oversized_files_before_download") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut settings = settings("policy");
        let mut message = document_message(10);
        assert!(
            !media::download_media(&api, &mut message, &settings, &settings.path)
                .await
                .unwrap()
        );
        assert_eq!(message["export_file_skipped"], "not selected");
        settings.media[6] = true;
        let mut message = document_message(settings.max_bytes + 1);
        assert!(
            !media::download_media(&api, &mut message, &settings, &settings.path)
                .await
                .unwrap()
        );
        assert!(
            message["export_file_skipped"]
                .as_str()
                .unwrap()
                .contains("size_limit")
        );
        assert!(api.requests.lock().unwrap().is_empty());
    });
}

#[test]
fn export_media_never_downloads_incoming_self_destructing_files() {
    if isolated("export_media_never_downloads_incoming_self_destructing_files") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut settings = settings("ttl");
        settings.media[6] = true;
        let mut message = document_message(10);
        message["media"]["ttl_seconds"] = 30.into();
        assert!(
            !media::download_media(&api, &mut message, &settings, &settings.path)
                .await
                .unwrap()
        );
        assert_eq!(
            message["export_file_skipped"],
            "incoming self-destructing media"
        );
        assert!(api.requests.lock().unwrap().is_empty());
    });
}

#[test]
fn export_media_download_preserves_long_ids_and_source_datacenter() {
    if isolated("export_media_download_preserves_long_ids_and_source_datacenter") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut settings = settings("download");
        settings.media[6] = true;
        let mut message = document_message(10);
        assert!(
            media::download_media(&api, &mut message, &settings, &settings.path)
                .await
                .unwrap()
        );
        let path = settings.path.join(message["export_file"].as_str().unwrap());
        assert_eq!(std::fs::read(path).unwrap(), b"downloaded fixture bytes");
        let requests = api.requests.lock().unwrap();
        let location = &requests[0].1["location"];
        assert_eq!(location["id"], "9007199254740993");
        assert_eq!(location["_dc_id"], 3);
        assert_eq!(location["file_reference"], json!([1, 2, 3]));
    });
}

#[test]
fn export_failed_media_download_is_reported_without_an_invented_file() {
    if isolated("export_failed_media_download_is_reported_without_an_invented_file") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            fail: true,
            ..Default::default()
        };
        let mut settings = settings("expired");
        settings.media[6] = true;
        let mut message = document_message(10);
        assert!(
            !media::download_media(&api, &mut message, &settings, &settings.path)
                .await
                .unwrap()
        );
        assert!(message.get("export_file").is_none());
        assert!(
            message["export_file_skipped"]
                .as_str()
                .unwrap()
                .contains("expired")
        );
    });
}

#[test]
fn export_rich_files_are_downloaded_and_linked_by_original_document_id() {
    if isolated("export_rich_files_are_downloaded_and_linked_by_original_document_id") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api=Fixture::default();let mut settings=settings("rich-files");settings.media[6]=true;
        let mut message=json!({"id":99,"rich_message":{"documents":[document_message(10)["media"]["document"].clone()]}});
        assert_eq!(media::download_rich_media(&api,&mut message,&settings,&settings.path).await.unwrap(),1);
        let relative=message["rich_files"]["9007199254740993"].as_str().unwrap();assert!(settings.path.join(relative).exists());
    });
}

#[test]
fn export_signed_media_ids_do_not_collapse_to_the_same_filename() {
    if isolated("export_signed_media_ids_do_not_collapse_to_the_same_filename") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut settings = settings("signed-ids");
        settings.media[6] = true;
        let mut paths = Vec::new();
        for id in ["-9007199254740993", "-9007199254740992"] {
            let mut message = document_message(10);
            message["id"] = (-99).into();
            message["media"]["document"]["id"] = id.into();
            message["media"]["document"]["attributes"] = json!([]);
            assert!(
                media::download_media(&api, &mut message, &settings, &settings.path)
                    .await
                    .unwrap()
            );
            let path = message["export_file"].as_str().unwrap();
            assert!(path.starts_with("files/-99_-900719925474099"));
            assert!(path.contains("file_-900719925474099"));
            paths.push(path.to_owned());
        }
        assert_ne!(paths[0], paths[1]);
    });
}

#[test]
fn export_custom_emoji_assets_are_fetched_once_and_linked_without_id_rounding() {
    if isolated("export_custom_emoji_assets_are_fetched_once_and_linked_without_id_rounding") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api=Fixture::default();let mut settings=settings("emoji");settings.media[4]=true;let mut cache=std::collections::BTreeMap::new();
        let make=|id|json!({"id":id,"message":"😀😀","entities":[{"_":"messageEntityCustomEmoji","offset":0,"length":2,"document_id":"9007199254740993"},{"_":"messageEntityCustomEmoji","offset":2,"length":2,"document_id":"9007199254740994"}]});
        let mut first=make(99);assert_eq!(media::hydrate_custom_emoji(&api,&mut first,&settings,&settings.path,&mut cache).await.unwrap(),2);
        let mut second=make(100);assert_eq!(media::hydrate_custom_emoji(&api,&mut second,&settings,&settings.path,&mut cache).await.unwrap(),0);
        assert_eq!(first["custom_emoji_files"],second["custom_emoji_files"]);
        for id in ["9007199254740993","9007199254740994"] {assert!(settings.path.join(first["custom_emoji_files"][id].as_str().unwrap()).exists());}
        let html=export::render::html_message(&first,&std::collections::BTreeMap::new()).unwrap();assert_eq!(html.matches("class=\"custom-emoji\"").count(),2);assert_eq!(html.matches('😀').count(),2);
        assert_eq!(api.requests.lock().unwrap().iter().filter(|(name,_)|name=="messages.getCustomEmojiDocuments").count(),1);
    });
}
