use anyhow::{Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::BTreeSet, process::Stdio, time::Duration};
use telegram_mcp::{
    backend::Telegram,
    safeguard::{Category, Facts, categorize, decide},
    server::validate_arguments,
    tools::{self, secret},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

const READ_ONLY: &[&str] = &[
    "get_secret_backend_status",
    "get_secret_pause_status",
    "list_secret_chats",
    "list_secret_deliveries",
    "read_secret_messages",
    "search_secret_messages",
    "secret_chat_status",
];

fn cases() -> Vec<(&'static str, Category, Value)> {
    use Category::{Free, Gated, SeenSignal, Send};
    vec![
        ("secret_chat_status", Free, json!({"chat_id": 42})),
        ("list_secret_chats", Free, json!({})),
        (
            "create_secret_chat",
            Free,
            json!({"user_id": "fixture_peer"}),
        ),
        (
            "set_secret_chat_timer",
            Free,
            json!({"chat_id": 42, "seconds": 0}),
        ),
        ("close_secret_chat", Gated, json!({"secret_chat_id": 42})),
        ("forget_secret_chat", Free, json!({"secret_chat_id": 42})),
        (
            "send_secret_message",
            Send,
            json!({"chat_id": 42, "message": "آزمایش"}),
        ),
        (
            "send_secret_media",
            Send,
            json!({"chat_id": 42, "file_path": "fixture.jpg"}),
        ),
        (
            "read_secret_messages",
            Free,
            json!({"chat_id": 42, "limit": 3}),
        ),
        (
            "save_secret_media",
            Free,
            json!({"chat_id": 42, "message_id": 9}),
        ),
        (
            "forward_secret_media",
            Send,
            json!({"from_chat_id": 42, "message_id": 9, "to_chat_id": 43}),
        ),
        (
            "send_timed_secret_message",
            Send,
            json!({"chat_id": 42, "message": "fixture", "seconds": 8}),
        ),
        (
            "send_timed_secret_media",
            Send,
            json!({"chat_id": 42, "file_path": "fixture.jpg", "seconds": 8}),
        ),
        (
            "delete_secret_message",
            Gated,
            json!({"chat_id": 42, "message_id": 9}),
        ),
        (
            "clear_secret_history",
            Gated,
            json!({"chat_id": 42, "confirm_chat_id": 42}),
        ),
        (
            "mark_secret_read",
            SeenSignal,
            json!({"chat_id": 42, "message_id": 9}),
        ),
        (
            "send_secret_typing",
            SeenSignal,
            json!({"chat_id": 42, "action": "typing"}),
        ),
        (
            "search_secret_messages",
            Free,
            json!({"chat_id": 42, "query": "fixture"}),
        ),
        (
            "copy_into_secret_chat",
            Send,
            json!({"from_chat_id": 123, "message_id": 9, "to_chat_id": 42}),
        ),
        (
            "start_secret_auto_save",
            Gated,
            json!({"folder": "fixture-archive"}),
        ),
        ("stop_secret_auto_save", Gated, json!({})),
        ("delete_secret_chat", Gated, json!({"secret_chat_id": 42})),
        (
            "delete_secret_chat_both_sides",
            Gated,
            json!({"secret_chat_id": 42}),
        ),
        (
            "delete_saved_secret_messages",
            Gated,
            json!({"secret_chat_id": 42}),
        ),
        (
            "export_secret_chat",
            Gated,
            json!({"secret_chat_id": 42, "format": "json", "photos": false, "date_from": "2026-01-01"}),
        ),
        (
            "configure_secret_backend",
            Gated,
            json!({"mode": "bounded", "records": 10, "bytes": 4096, "age_seconds": 1.5}),
        ),
        ("get_secret_backend_status", Free, json!({})),
        ("pause_secret_chat", Gated, json!({"secret_chat_id": 42})),
        (
            "get_secret_pause_status",
            Free,
            json!({"secret_chat_id": 42}),
        ),
        ("check_secret_custody", Gated, json!({})),
        ("resume_secret_chat", Gated, json!({"secret_chat_id": 42})),
        (
            "list_secret_deliveries",
            Free,
            json!({"secret_chat_id": 42, "limit": 100, "offset": 4096}),
        ),
        (
            "acknowledge_secret_delivery",
            Gated,
            json!({"secret_chat_id": 42, "seq_no": 0}),
        ),
        (
            "receive_secret_media",
            Gated,
            json!({"secret_chat_id": 42, "message_id": 9, "max_bytes": 67108864}),
        ),
    ]
}

async fn exposed(mode: &str) -> Vec<Value> {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("fixture.env");
    std::fs::write(&config, "# Offline catalogue fixture\n").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_telegram-mcp"));
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|key| key.starts_with("TELEGRAM_") || key.starts_with("MCP_"))
        {
            command.env_remove(key);
        }
    }
    let mut child = command
        .current_dir(temp.path())
        .arg("--config")
        .arg(&config)
        .arg("serve")
        .env("TELEGRAM_STATE_DIR", temp.path().join("state"))
        .env("TELEGRAM_EXPOSED_TOOLS", mode)
        .env("MCP_TRANSPORT", "stdio")
        .env("TOKIO_WORKER_THREADS", "2")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": rmcp::model::ProtocolVersion::LATEST.as_str(),
            "io.modelcontextprotocol/clientInfo": {"name": "secret-catalogue-test", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    }});
    timeout(Duration::from_secs(60), async {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let response: Value =
            serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        drop(stdin);
        assert!(child.wait().await.unwrap().success());
        response["result"]["tools"]
            .as_array()
            .expect("tools/list must return tools")
            .clone()
    })
    .await
    .expect("Offline tools/list and stdio shutdown exceeded their deadline")
}

#[tokio::test]
async fn catalogue_exposes_all_secret_tools_and_only_seven_in_read_only_mode() {
    let expected: BTreeSet<_> = cases().into_iter().map(|(name, _, _)| name).collect();
    assert_eq!(expected.len(), 34);
    assert_eq!(
        secret::TOOL_NAMES.iter().copied().collect::<BTreeSet<_>>(),
        expected
    );
    assert_eq!(secret::TOOL_NAMES.len(), expected.len());

    let all = exposed("all").await;
    assert_eq!(all.len(), 390);
    let all_names: BTreeSet<_> = all
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(all_names.len(), all.len());
    assert!(expected.is_subset(&all_names));

    let readonly = exposed("read-only").await;
    let readonly_secret: BTreeSet<_> = readonly
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .filter(|name| expected.contains(name))
        .collect();
    assert_eq!(readonly_secret, READ_ONLY.iter().copied().collect());
}

fn descriptors() -> Vec<Value> {
    serde_json::from_str(include_str!("../resources/tool-catalogue.json")).unwrap()
}

#[test]
fn safeguard_classifies_each_secret_tool_from_its_published_annotations() {
    let descriptors = descriptors();
    for (name, category, args) in cases() {
        let descriptor = descriptors
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        assert_eq!(descriptor["deferred"], false, "{name}");
        let hints = &descriptor["annotations"];
        assert!(!hints["title"].as_str().unwrap().is_empty(), "{name}");
        assert!(
            hints["idempotentHint"].is_boolean() && hints["openWorldHint"].is_boolean(),
            "{name}"
        );
        let read_only = hints["readOnlyHint"].as_bool().unwrap();
        let destructive = hints["destructiveHint"].as_bool().unwrap();
        assert_eq!(read_only, READ_ONLY.contains(&name), "{name}");
        assert_eq!(categorize(name, read_only, destructive), category, "{name}");

        let facts = Facts {
            ghost_on: true,
            first_message: true,
            ..Facts::default()
        };
        let decision = decide(name, read_only, destructive, &args, &facts);
        let reason = match category {
            Category::Gated => Some("gated"),
            Category::SeenSignal => Some("seen_signal_under_ghost"),
            Category::Send => Some("first_message"),
            Category::Free => None,
        };
        assert_eq!(
            decision.outcome,
            if reason.is_some() { "ask" } else { "run" },
            "{name}"
        );
        assert_eq!(
            decision.reasons,
            reason.into_iter().collect::<Vec<_>>(),
            "{name}"
        );
    }
}

struct Echo;

#[async_trait]
impl Telegram for Echo {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        Ok(json!({"method": method, "params": params}))
    }

    async fn resolve_peer(&self, value: &Value) -> Result<Value> {
        match value {
            value if value == "fixture_peer" => Ok(json!({"_":"inputPeerUser","user_id":123})),
            value if value == 123 => Ok(json!({"_":"inputPeerChat","chat_id":123})),
            _ => bail!("Unexpected cloud peer resolution in a secret-chat adapter"),
        }
    }
}

#[tokio::test]
async fn each_published_schema_validates_and_routes_to_its_secret_backend_method() {
    let descriptors = descriptors();
    for (name, _, mut args) in cases() {
        let descriptor = descriptors
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        let schema = descriptor["inputSchema"].as_object().unwrap();
        args["account"] = json!("fixture-account");
        validate_arguments(&mut args, schema).unwrap_or_else(|error| panic!("{name}: {error:#}"));
        let output = tools::call(&Echo, name, &args)
            .await
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert_eq!(output["method"], format!("local.secret.{name}"));
        assert_eq!(output["params"]["account"], "fixture-account", "{name}");
        match name {
            "create_secret_chat" => assert_eq!(output["params"]["user_id"]["_"], "inputPeerUser"),
            "copy_into_secret_chat" => {
                assert_eq!(output["params"]["from_chat_id"]["_"], "inputPeerChat")
            }
            _ => {}
        }
    }
    assert!(secret::call(&Echo, "get_me", &json!({})).await.is_none());
}

#[tokio::test]
async fn autosave_and_custody_forward_validated_options_and_normalized_ids() {
    for (id, wire) in [
        (-1_999_999_999_958_i64, 42),
        (-698_877_786, -698_877_786),
        (-2_000_698_877_786, -698_877_786),
        (1_999_301_122_214, -698_877_786),
    ] {
        let result = tools::call(
            &Echo,
            "list_secret_deliveries",
            &json!({"secret_chat_id": id}),
        )
        .await
        .unwrap();
        assert_eq!(result["params"]["chat_id"], wire);
        assert_eq!(result["params"]["limit"], 50);
        assert_eq!(result["params"]["offset"], 0);
    }

    let result = tools::call(
        &Echo,
        "configure_secret_backend",
        &json!({"mode": "bounded", "records": 3, "bytes": 4096, "age_seconds": 1.5}),
    )
    .await
    .unwrap();
    assert_eq!(result["params"]["mode"], "bounded");
    assert_eq!(result["params"]["age_seconds"], 1.5);
    let result = tools::call(&Echo, "export_secret_chat", &json!({"secret_chat_id": 42, "format": "json", "photos": false, "date_from": "2026-01-01"})).await.unwrap();
    assert_eq!(result["params"]["chat_id"], 42);
    assert_eq!(result["params"]["format"], "json");
    assert_eq!(result["params"]["photos"], false);
    assert_eq!(result["params"]["date_from"], "2026-01-01");
}

#[tokio::test]
async fn rejects_invalid_secret_arguments_before_invoking_backend() {
    for (name, args, field) in [
        (
            "clear_secret_history",
            json!({"chat_id": 42, "confirm_chat_id": 43}),
            "confirm_chat_id",
        ),
        (
            "list_secret_deliveries",
            json!({"secret_chat_id": 42, "limit": 0}),
            "limit",
        ),
        (
            "list_secret_deliveries",
            json!({"secret_chat_id": 42, "limit": 101}),
            "limit",
        ),
        (
            "list_secret_deliveries",
            json!({"secret_chat_id": 42, "offset": -1}),
            "offset",
        ),
        (
            "list_secret_deliveries",
            json!({"secret_chat_id": 42, "offset": 4097}),
            "offset",
        ),
        (
            "list_secret_deliveries",
            json!({"secret_chat_id": 42, "limit": "10"}),
            "limit",
        ),
        (
            "get_secret_pause_status",
            json!({"secret_chat_id": true}),
            "secret_chat_id",
        ),
        (
            "acknowledge_secret_delivery",
            json!({"secret_chat_id": 42, "seq_no": -1}),
            "seq_no",
        ),
        (
            "configure_secret_backend",
            json!({"mode": "unsupported"}),
            "mode",
        ),
        (
            "configure_secret_backend",
            json!({"mode": "bounded", "records": 3, "bytes": 4096}),
            "age_seconds",
        ),
        (
            "configure_secret_backend",
            json!({"mode": "normal", "records": 3}),
            "budgets",
        ),
        (
            "send_timed_secret_message",
            json!({"chat_id": 42, "message": "fixture", "seconds": 0}),
            "seconds",
        ),
        (
            "send_timed_secret_media",
            json!({"chat_id": 42, "file_path": "fixture.jpg", "seconds": -1}),
            "seconds",
        ),
        (
            "receive_secret_media",
            json!({"secret_chat_id": 42, "message_id": 9, "max_bytes": 0}),
            "max_bytes",
        ),
        (
            "receive_secret_media",
            json!({"secret_chat_id": 42, "message_id": 9, "max_bytes": 67108865}),
            "max_bytes",
        ),
        ("start_secret_auto_save", json!({"folder": "  "}), "folder"),
        (
            "export_secret_chat",
            json!({"secret_chat_id": 42, "format": "pdf"}),
            "format",
        ),
    ] {
        let error = tools::call(&Echo, name, &args).await.expect_err(name);
        assert!(error.to_string().contains(field), "{name}: {error:#}");
    }
}
