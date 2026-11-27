use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn encoded_attachment_names_include_ids_and_preserve_extensions_within_component_limits() {
    if isolated(
        "media_edges::encoded_attachment_names_include_ids_and_preserve_extensions_within_component_limits",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut options = settings("unicode-names");
        options.media[6] = true;
        let mut message = document_message(24);
        message["id"] = json!("9223372036854775807");
        message["media"]["document"]["id"] = json!("9223372036854775806");
        message["media"]["document"]["attributes"] =
            json!([{"_":"documentAttributeFilename","file_name":format!("{}.pdf","界😀".repeat(100))}]);
        assert!(media::download_media(&api, &mut message, &options, &options.path).await.unwrap());
        let path = std::path::Path::new(message["export_file"].as_str().unwrap());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.len() <= 255);
        assert!(name.encode_utf16().count() <= 255);
        assert!(name.starts_with("9223372036854775807_9223372036854775806_"));
        assert!(name.ends_with(".pdf"));
        assert!(options.path.join(path).is_file());
    });
}

#[test]
fn photo_export_checks_progressive_sizes_then_downloads_largest_area_with_the_right_dc() {
    if isolated(
        "media_edges::photo_export_checks_progressive_sizes_then_downloads_largest_area_with_the_right_dc",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut options = settings("photo");
        options.media[0] = true;
        options.max_bytes = 100;
        let photo = json!({"id":3,"out":false,"media":{"photo":{"_":"photo","id":"9007199254740993","access_hash":"7","file_reference":[],"dc_id":4,"sizes":[{"_":"photoSize","type":"s","w":16,"h":16,"size":50},{"_":"photoSizeProgressive","type":"x","w":200,"h":100,"sizes":[100,500,1000]}]}}});
        let mut first = photo.clone();
        assert!(!media::download_media(&api, &mut first, &options, &options.path).await.unwrap());
        assert_eq!(first["export_file_skipped"], "file exceeds size_limit_mb");
        assert!(api.requests.lock().unwrap().is_empty());
        options.max_bytes = 2000;
        let mut second = photo;
        assert!(media::download_media(&api, &mut second, &options, &options.path).await.unwrap());
        let calls = api.requests.lock().unwrap();
        let location = &calls.last().unwrap().1["location"];
        assert_eq!(location["_"], "inputPhotoFileLocation");
        assert_eq!(location["thumb_size"], "x");
        assert_eq!(location["_dc_id"], 4);
        assert!(options.path.join(second["export_file"].as_str().unwrap()).is_file());
    });
}

#[test]
fn selected_attachment_categories_preserve_distinct_folders_and_do_not_require_other_media_flags() {
    if isolated(
        "media_edges::selected_attachment_categories_preserve_distinct_folders_and_do_not_require_other_media_flags",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let cases = [
            (
                json!({"_":"documentAttributeVideo","round_message":false}),
                1,
                "video_files",
            ),
            (
                json!({"_":"documentAttributeAudio","voice":true}),
                2,
                "voice_messages",
            ),
            (
                json!({"_":"documentAttributeVideo","round_message":true}),
                3,
                "round_video_messages",
            ),
            (json!({"_":"documentAttributeSticker"}), 4, "stickers"),
            (json!({"_":"documentAttributeAnimated"}), 5, "animations"),
        ];
        for (attribute, index, folder) in cases {
            let mut options = settings(&format!("category-{index}"));
            options.media[index] = true;
            let mut item = document_message(index as u64);
            item["media"]["document"]["attributes"] =
                json!([attribute,{"_":"documentAttributeFilename","file_name":"attachment.bin"}]);
            assert!(
                media::download_media(&api, &mut item, &options, &options.path)
                    .await
                    .unwrap()
            );
            assert!(
                item["export_file"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("{folder}/"))
            );
        }
    });
}

#[test]
fn outgoing_ttl_files_are_exportable_but_unsupported_media_never_creates_fake_files() {
    if isolated(
        "media_edges::outgoing_ttl_files_are_exportable_but_unsupported_media_never_creates_fake_files",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut options = settings("ttl");
        options.media[6] = true;
        let mut outgoing = document_message(7);
        outgoing["out"] = true.into();
        outgoing["media"]["ttl_seconds"] = json!(10);
        assert!(
            media::download_media(&api, &mut outgoing, &options, &options.path)
                .await
                .unwrap()
        );
        for mut item in [
            json!({"id":8}),
            json!({"id":9,"media":{"_":"messageMediaUnsupported"}}),
        ] {
            assert!(
                !media::download_media(&api, &mut item, &options, &options.path)
                    .await
                    .unwrap()
            );
            assert!(item.get("export_file").is_none());
        }
    });
}

#[test]
fn unavailable_custom_emoji_is_recorded_once_and_never_retried_for_each_message() {
    if isolated(
        "media_edges::unavailable_custom_emoji_is_recorded_once_and_never_retried_for_each_message",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture {
            fail: true,
            ..Default::default()
        };
        let mut options = settings("missing-emoji");
        options.media[4] = true;
        let mut cache = std::collections::BTreeMap::new();
        let make = |id| {
            json!({"id":id,"entities":[
                {"_":"messageEntityCustomEmoji","document_id":"9007199254740993"},
                {"_":"messageEntityCustomEmoji","document_id":"9007199254740993"},
                {"_":"messageEntityCustomEmoji","document_id":"invalid"}
            ]})
        };
        let mut first = make(1);
        assert_eq!(
            media::hydrate_custom_emoji(&api, &mut first, &options, &options.path, &mut cache)
                .await
                .unwrap(),
            0
        );
        let mut second = make(2);
        assert_eq!(
            media::hydrate_custom_emoji(&api, &mut second, &options, &options.path, &mut cache)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            first["custom_emoji_unavailable"],
            json!(["9007199254740993"])
        );
        assert_eq!(
            second["custom_emoji_unavailable"],
            first["custom_emoji_unavailable"]
        );
        assert_eq!(api.requests.lock().unwrap().len(), 1);
        assert!(!options.path.exists());
    });
}

#[derive(Clone)]
struct EmojiFailureFixture {
    inner: Fixture,
    failure: &'static str,
}

#[async_trait::async_trait]
impl Telegram for EmojiFailureFixture {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        if method == "messages.getCustomEmojiDocuments" {
            self.inner
                .requests
                .lock()
                .unwrap()
                .push((method.into(), params));
            match self.failure {
                "lookup" => bail!("Synthetic emoji lookup failure"),
                "missing" => return Ok(json!([])),
                _ => {
                    let mut document = document_message(1)["media"]["document"].clone();
                    document["id"] = json!("9007199254740993");
                    if self.failure == "size_limit" {
                        document["size"] = json!(64 * 1024 * 1024);
                    }
                    return Ok(json!([document]));
                }
            }
        }
        let mut response = self.inner.invoke(method, params).await?;
        if let Some(messages) = response.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                message["entities"] = json!([{"_":"messageEntityCustomEmoji","offset":0,"length":1,"document_id":"9007199254740993"}]);
            }
        }
        Ok(response)
    }

    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.inner.resolve_peer(input).await
    }

    async fn download(
        &self,
        location: Value,
        destination: &std::path::Path,
        max_bytes: u64,
    ) -> Result<Value> {
        if self.failure == "download" {
            bail!("Synthetic emoji download failure");
        }
        self.inner.download(location, destination, max_bytes).await
    }
}

#[test]
fn selected_custom_emoji_failures_make_exports_partial_but_size_exclusions_do_not() {
    if isolated(
        "media_edges::selected_custom_emoji_failures_make_exports_partial_but_size_exclusions_do_not",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for failure in ["lookup", "missing", "download", "size_limit"] {
            let api = EmojiFailureFixture {
                inner: Fixture {
                    short_page: true,
                    ..Default::default()
                },
                failure,
            };
            let mut options = settings(&format!("emoji-{failure}"));
            options.media[4] = true;
            let (_sender, receiver) = tokio::sync::watch::channel(false);
            let result = export::run(
                &api,
                "chat",
                &json!({"chat_id":"@test"}),
                &options,
                receiver,
                "",
            )
            .await
            .unwrap();
            let failed = failure != "size_limit";
            assert_eq!(result["partial"], failed, "{failure}");
            assert_eq!(result["complete"], !failed, "{failure}");
            assert_eq!(
                result["failed_files"],
                if failed { 1 } else { 0 },
                "{failure}"
            );
            assert_eq!(result["messages"], 2);
            assert_eq!(result["files"], 0);
            assert_eq!(
                api.inner
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(name, _)| name == "messages.getCustomEmojiDocuments")
                    .count(),
                1
            );
            let doc: Value =
                serde_json::from_slice(&std::fs::read(options.path.join("result.json")).unwrap())
                    .unwrap();
            assert_eq!(doc["messages"].as_array().unwrap().len(), 2);
        }
    });
}

#[test]
fn rich_media_deduplicates_repeated_assets_and_keeps_successful_downloads_when_another_is_unselected()
 {
    if isolated(
        "media_edges::rich_media_deduplicates_repeated_assets_and_keeps_successful_downloads_when_another_is_unselected",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = Fixture::default();
        let mut options = settings("rich-media");
        options.media[6] = true;
        let doc = document_message(3)["media"]["document"].clone();
        let mut item = json!({"id":3,"rich_message":{"documents":[doc.clone(),doc],"photos":[{"_":"photo","id":"99","sizes":[]}]}});
        assert_eq!(
            media::download_rich_media(&api, &mut item, &options, &options.path)
                .await
                .unwrap(),
            1
        );
        assert_eq!(item["rich_files"].as_object().unwrap().len(), 1);
        assert_eq!(api.requests.lock().unwrap().len(), 1);
    });
}

#[derive(Clone, Default)]
struct RefreshFixture {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    always_stale_doc_id: Option<String>,
    download_attempts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Telegram for RefreshFixture {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        if method == "account.initTakeoutSession" {
            return Ok(json!({"_":"account.takeout","id":"9"}));
        }
        let mut inner_method = method;
        let mut inner_params = &params;
        while inner_method == "invokeWithTakeout" {
            inner_method = inner_params["query"]["_"].as_str().unwrap();
            inner_params = &inner_params["query"];
        }
        match inner_method {
            "account.finishTakeoutSession" => Ok(json!(true)),
            "messages.getSplitRanges" => Ok(json!([])),
            "channels.getFullChannel" => Ok(json!({"full_chat":{}})),
            "messages.getHistory" => {
                if inner_params["offset_id"].as_i64().unwrap() != 0 {
                    return Ok(json!({"messages":[],"users":[],"chats":[]}));
                }
                Ok(json!({
                    "messages": [
                        {
                            "_": "message",
                            "id": 20,
                            "date": 1700000100,
                            "message": "Contact card",
                            "media": {
                                "_": "messageMediaContact",
                                "first_name": "Mina",
                                "last_name": "R.",
                                "phone_number": "+12025550199",
                                "vcard": "BEGIN:VCARD\nVERSION:3.0\nFN:Mina R.\nTEL:+12025550199\nEND:VCARD\n"
                            }
                        },
                        {
                            "_": "message",
                            "id": 11,
                            "date": 1700000050,
                            "message": "Second attachment",
                            "media": {
                                "_": "messageMediaDocument",
                                "document": {
                                    "_": "document",
                                    "id": "502",
                                    "access_hash": "88",
                                    "file_reference": [0],
                                    "dc_id": 2,
                                    "size": 32,
                                    "mime_type": "video/mp4",
                                    "attributes": [
                                        {"_": "documentAttributeFilename", "file_name": "clip.mp4"},
                                        {"_": "documentAttributeVideo", "w": 320, "h": 180, "duration": 4, "round_message": false}
                                    ],
                                    "thumbs": [{"_": "photoSize", "type": "m", "w": 160, "h": 90, "size": 20}]
                                }
                            }
                        },
                        {
                            "_": "message",
                            "id": 10,
                            "date": 1700000010,
                            "message": "First attachment",
                            "media": {
                                "_": "messageMediaDocument",
                                "document": {
                                    "_": "document",
                                    "id": "501",
                                    "access_hash": "77",
                                    "file_reference": [0],
                                    "dc_id": 2,
                                    "size": 18,
                                    "attributes": [{"_": "documentAttributeFilename", "file_name": "notes.txt"}]
                                }
                            }
                        }
                    ],
                    "users": [],
                    "chats": [{"_": "channel", "id": "12", "title": "Refresh Channel"}]
                }))
            }
            "channels.getMessages" => {
                let mid = inner_params["id"][0]["id"].as_i64().unwrap();
                let doc_id = if mid == 10 { "501" } else { "502" };
                let fresh_ref = if self.always_stale_doc_id.as_deref() == Some(doc_id) {
                    json!([0])
                } else {
                    json!([9, 9, 9])
                };
                Ok(json!({
                    "messages": [{
                        "_": "message",
                        "id": mid,
                        "media": {
                            "_": "messageMediaDocument",
                            "document": {
                                "_": "document",
                                "id": doc_id,
                                "access_hash": "77",
                                "file_reference": fresh_ref,
                                "dc_id": 2,
                                "size": 18
                            }
                        }
                    }]
                }))
            }
            other => bail!("Unexpected RPC {other}"),
        }
    }

    async fn resolve_peer(&self, _input: &Value) -> Result<Value> {
        Ok(json!({"_":"inputPeerChannel","channel_id":"12","access_hash":"34"}))
    }

    async fn download(
        &self,
        location: Value,
        destination: &std::path::Path,
        _max_bytes: u64,
    ) -> Result<Value> {
        self.download_attempts.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        if location["file_reference"] == json!([0]) {
            bail!("RPC_ERROR 400: FILE_REFERENCE_EXPIRED");
        }
        std::fs::write(destination, b"recovered-media-bytes")?;
        Ok(json!({"path": destination, "bytes": 21}))
    }

    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        Some(Arc::new(self.clone()))
    }
}

#[test]
fn expired_file_reference_refreshes_once_recovers_thumb_and_vcard_with_monotonic_elapsed_time() {
    if isolated(
        "media_edges::expired_file_reference_refreshes_once_recovers_thumb_and_vcard_with_monotonic_elapsed_time",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = RefreshFixture::default();
        let mut opts = settings("refresh-ok");
        opts.media[1] = true;
        opts.media[6] = true;
        let out_dir = opts.path.clone();
        let job = export::jobs::start(
            Arc::new(api.clone()),
            "chat",
            json!({"chat_id":"@test"}),
            opts,
        )
        .unwrap();
        let mut prev_elapsed = job["elapsed_seconds"].as_f64().unwrap();
        let final_status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let st = export::jobs::status(&json!({"job_id": job["job_id"]})).unwrap();
                let cur = st["elapsed_seconds"].as_f64().unwrap();
                assert!(
                    cur >= prev_elapsed,
                    "elapsed_seconds must be monotonic: {cur} < {prev_elapsed}"
                );
                prev_elapsed = cur;
                if st["status"] != "running" {
                    break st;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();

        assert_eq!(final_status["status"], "done");
        assert_eq!(final_status["partial"], false);
        assert_eq!(final_status["result"]["complete"], true);
        assert_eq!(final_status["result"]["failed_files"], 0);
        assert!(
            final_status["result"]["refreshed_references"]
                .as_u64()
                .unwrap()
                >= 2
        );
        let frozen = final_status["elapsed_seconds"].as_f64().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let after = export::jobs::status(&json!({"job_id": job["job_id"]})).unwrap();
        assert_eq!(after["elapsed_seconds"].as_f64().unwrap(), frozen);

        let doc: Value =
            serde_json::from_slice(&std::fs::read(out_dir.join("result.json")).unwrap()).unwrap();
        let msgs = doc["messages"].as_array().unwrap();
        assert!(out_dir.join(msgs[0]["file"].as_str().unwrap()).is_file());
        assert!(out_dir.join(msgs[1]["file"].as_str().unwrap()).is_file());
        assert!(
            out_dir
                .join(msgs[1]["thumbnail"].as_str().unwrap())
                .is_file()
        );
        assert!(
            out_dir
                .join(msgs[2]["contact_vcard"].as_str().unwrap())
                .is_file()
        );
        let html = std::fs::read_to_string(out_dir.join("messages.html")).unwrap();
        assert!(html.contains("media_contact"));
        assert!(html.contains("vCard"));
        assert!(html.contains("class=\"thumb\""));
    });
}

#[test]
fn second_stale_reference_retries_only_once_keeps_siblings_and_reports_partial_status() {
    if isolated(
        "media_edges::second_stale_reference_retries_only_once_keeps_siblings_and_reports_partial_status",
    ) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let api = RefreshFixture {
            always_stale_doc_id: Some("502".into()),
            ..Default::default()
        };
        let mut opts = settings("refresh-partial");
        opts.media[1] = true;
        opts.media[6] = true;
        let out_dir = opts.path.clone();
        let job = export::jobs::start(
            Arc::new(api.clone()),
            "chat",
            json!({"chat_id":"@test"}),
            opts,
        )
        .unwrap();
        let final_status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let st = export::jobs::status(&json!({"job_id": job["job_id"]})).unwrap();
                if st["status"] != "running" {
                    break st;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            final_status["status"], "partial",
            "Still-stale optional media must report partial status rather than complete success"
        );
        assert_eq!(final_status["partial"], true);
        assert_eq!(final_status["failed_files"], 1);
        assert_eq!(final_status["result"]["complete"], false);
        assert_eq!(final_status["result"]["refreshed_references"], 2);

        let refresh_rpcs = api
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p)| m == "channels.getMessages" && p["id"][0]["id"] == 11)
            .count();
        assert_eq!(
            refresh_rpcs, 1,
            "Stale file reference for message 11 must refresh at most once"
        );

        let doc: Value =
            serde_json::from_slice(&std::fs::read(out_dir.join("result.json")).unwrap()).unwrap();
        let msgs = doc["messages"].as_array().unwrap();
        assert!(
            out_dir.join(msgs[0]["file"].as_str().unwrap()).is_file(),
            "Successful sibling media must be preserved"
        );
        assert!(msgs[1].get("file").is_none());
        assert!(
            out_dir
                .join(msgs[2]["contact_vcard"].as_str().unwrap())
                .is_file()
        );
    });
}
