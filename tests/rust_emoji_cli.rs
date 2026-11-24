use serde_json::{Value, json};
use std::path::Path;

fn invoke(root: &Path, args: &[&str]) -> std::process::Output {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_emoji-studio"))
        .args(args)
        .current_dir(root)
        .env("TELEGRAM_STATE_DIR", root.join("state"))
        .env_remove("TELEGRAM_MCP_URL")
        .env_remove("TELEGRAM_MCP_EMOJI_PACKS")
        .env_remove("TELEGRAM_MCP_LOG_FILE")
        .env_remove("TELEGRAM_MCP_LOG_MAX_BYTES")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Emoji Studio command exceeded its test deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn exported(root: &Path) {
    std::fs::create_dir_all(root.join(".thumbs")).unwrap();
    std::fs::write(root.join("index.json"),serde_json::to_vec(&json!({"packs":[{"set_name":"pack","title":"Owned pack","link":"https://t.me/addemoji/pack"}],"by_source_id":{"99":"1"}})).unwrap()).unwrap();
    std::fs::write(root.join("pack.json"),serde_json::to_vec(&json!({"emoji":[{"custom_emoji_id":"1","glyph":"💳"},{"custom_emoji_id":"2","glyph":"🔴"}]})).unwrap()).unwrap();
    for (id, colour) in [(1, [255, 0, 0, 255]), (2, [0, 0, 255, 255])] {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(16, 16, image::Rgba(colour)))
            .write_to(
                &mut std::fs::File::create(root.join(format!(".thumbs/{id}.webp"))).unwrap(),
                image::ImageFormat::WebP,
            )
            .unwrap();
    }
}

#[test]
fn emoji_cli_help_lists_every_actual_native_workflow_without_connecting() {
    let temp = tempfile::tempdir().unwrap();
    let output = invoke(temp.path(), &["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "sheet", "gif", "index", "refresh", "nearest", "lines", "insert", "accuracy",
    ] {
        assert!(help.contains(command), "Missing CLI command {command}");
    }
    assert!(!temp.path().join("state").exists());
}

#[test]
fn emoji_cli_indexes_refreshes_and_compares_local_art_without_a_server() {
    let temp = tempfile::tempdir().unwrap();
    let packs = temp.path().join("packs");
    exported(&packs);
    let cache = temp.path().join("fingerprints.json");
    let cache_arg = cache.to_str().unwrap();
    let packs_arg = packs.to_str().unwrap();
    let first = invoke(
        temp.path(),
        &["--cache", cache_arg, "index", "--packs-dir", packs_arg],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
    assert_eq!(stored.as_object().unwrap().len(), 2);
    assert_eq!(stored["1"]["silhouette"].as_array().unwrap().len(), 144);
    let second = invoke(
        temp.path(),
        &["--cache", cache_arg, "refresh", "--packs-dir", packs_arg],
    );
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("\"unchanged\": 2"));
    let output = temp.path().join("nearest.png");
    let nearest = invoke(
        temp.path(),
        &[
            "--cache",
            cache_arg,
            "nearest",
            "--packs-dir",
            packs_arg,
            "--id",
            "1",
            "--k",
            "1",
            "--out",
            output.to_str().unwrap(),
        ],
    );
    assert!(
        nearest.status.success(),
        "{}",
        String::from_utf8_lossy(&nearest.stderr)
    );
    assert!(output.is_file());
    let sheet = telegram_mcp::tools::visual::images::decode_image(&std::fs::read(&output).unwrap())
        .unwrap();
    assert!(sheet.width() > 100);
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(output.with_file_name("nearest.png.json")).unwrap())
            .unwrap();
    assert_eq!(metadata["cells"][0]["id"], "1");
    assert_eq!(metadata["cells"][1]["id"], "2");
}

#[test]
fn emoji_cli_sheet_keeps_document_ids_and_labels_associated_with_real_pixels() {
    let temp = tempfile::tempdir().unwrap();
    let packs = temp.path().join("packs");
    exported(&packs);
    let output = temp.path().join("sheet.png");
    let result = invoke(
        temp.path(),
        &[
            "sheet",
            "--packs-dir",
            packs.to_str().unwrap(),
            "--ids",
            "2,1",
            "--columns",
            "2",
            "--out",
            output.to_str().unwrap(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let sheet = telegram_mcp::tools::visual::images::decode_image(&std::fs::read(output).unwrap())
        .unwrap()
        .to_rgba8();
    let left = sheet.get_pixel(50, 50);
    let right = sheet.get_pixel(145, 50);
    assert!(left[2] > left[0]);
    assert!(right[0] > right[2]);
}

#[test]
fn emoji_cli_rejects_a_wrong_export_root_instead_of_building_a_false_empty_index() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache.json");
    let result = invoke(
        temp.path(),
        &[
            "--cache",
            cache.to_str().unwrap(),
            "index",
            "--packs-dir",
            temp.path().to_str().unwrap(),
        ],
    );
    assert!(!result.status.success());
    assert!(!cache.exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Emoji Mapper export"));
}

#[test]
fn emoji_cli_indexes_a_visible_animation_frame_instead_of_its_blank_endpoints() {
    let temp = tempfile::tempdir().unwrap();
    let packs = temp.path().join("packs");
    exported(&packs);
    // Synthetic lossless WebP with exactly blank/red/blank source frames.
    std::fs::write(
        packs.join(".thumbs/1.webp"),
        include_bytes!("fixtures/emoji/transparent-red-transparent.webp"),
    )
    .unwrap();
    let cache = temp.path().join("cache.json");
    let result = invoke(
        temp.path(),
        &[
            "--cache",
            cache.to_str().unwrap(),
            "index",
            "--packs-dir",
            packs.to_str().unwrap(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let index: Value = serde_json::from_slice(&std::fs::read(cache).unwrap()).unwrap();
    assert!(
        index["1"]["silhouette"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_f64().unwrap() > 0.9)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emoji_insert_sends_the_inspected_snapshot_with_the_approved_edit() {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    use rmcp::{
        ServerHandler,
        model::*,
        service::{RequestContext, RoleServer},
    };
    use std::sync::{Arc, Mutex};
    const TOKEN: &str = "a-synthetic-owner-token-with-more-than-32-characters";
    type Calls = Arc<Mutex<Vec<(String, Value)>>>;
    fn proof(purpose: &[u8], nonce: &str, target: &str, status: u16, body: &[u8]) -> String {
        use hmac::{Hmac, KeyInit, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(TOKEN.as_bytes()).unwrap();
        for part in [
            b"telegram-mcp-health-v2\0".as_slice(),
            purpose,
            b"\0",
            nonce.as_bytes(),
            b"\0",
            target.as_bytes(),
            b"\0",
            &status.to_be_bytes(),
            body,
        ] {
            mac.update(part);
        }
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    async fn health(request: axum::extract::Request) -> axum::response::Response {
        assert!(
            request
                .headers()
                .get(axum::http::header::AUTHORIZATION)
                .is_none(),
            "Health preflight must not disclose its reusable bearer"
        );
        let nonce = request.headers()["x-telegram-health-nonce"]
            .to_str()
            .unwrap();
        let target = request.uri().to_string();
        assert_eq!(
            request.headers()["x-telegram-health-proof"],
            proof(b"request", nonce, &target, 0, &[])
        );
        let body = serde_json::to_vec(
            &json!({"ready":true,"backend":"grammers","pid":std::process::id()}),
        )
        .unwrap();
        let signature = proof(b"response", nonce, &target, 200, &body);
        axum::response::Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .header("x-telegram-health-proof", signature)
            .body(axum::body::Body::from(body))
            .unwrap()
    }
    #[derive(Clone)]
    struct Remote(Calls);
    impl ServerHandler for Remote {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }
        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, rmcp::ErrorData> {
            let args = Value::Object(request.arguments.unwrap_or_default());
            self.0
                .lock()
                .unwrap()
                .push((request.name.to_string(), args));
            let payload = match request.name.as_ref() {
                "inspect_message" => {
                    json!({"message":{"id":42,"text":"hello 😀\nsecond","entities":[{"type":"bold","offset":0,"length":5}]}})
                }
                "edit_message" => json!({"approval_required":true,"performed":false}),
                _ => panic!("Unexpected CLI request {}", request.name),
            };
            Ok(CallToolResult::success(vec![ContentBlock::text(payload.to_string())]).into())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let remote = Remote(calls.clone());
    let service: StreamableHttpService<Remote, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(remote.clone()),
        Default::default(),
        StreamableHttpServerConfig::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .nest_service("/mcp", service)
                .route("/health", axum::routing::get(health)),
        )
        .await
        .unwrap();
    });
    let state =
        telegram_mcp::state::State::at(temp.path().join("state"), vec![temp.path().to_owned()])
            .unwrap();
    state
        .write_json(
            "http-service.json",
            &json!({"url":endpoint,"token":TOKEN,"health_protocol":2,"pid":std::process::id()}),
        )
        .unwrap();
    let output = invoke(
        temp.path(),
        &[
            "insert",
            "--chat",
            "@owner",
            "--message",
            "42",
            "--line",
            "2",
            "--id",
            "9",
            "--glyph",
            "✅",
        ],
    );
    serving.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("approval_required"));
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["inspect_message", "edit_message"]
    );
    let edit = &calls[1].1;
    assert_eq!(edit["new_text"], "hello 😀\nsecond✅");
    assert_eq!(
        edit["expected_source"],
        json!({"text":"hello 😀\nsecond","entities":[{"type":"bold","offset":0,"length":5}]})
    );
    assert_eq!(edit["entities"][1]["offset"], 15);
}
