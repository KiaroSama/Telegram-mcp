use super::common::*;
use serde_json::{Value, json};

async fn legacy(version: &str) {
    let fixture = Fixture::new();
    let mut child = McpProcess::start(fixture.command(&[]));
    child.initialize(version).await;
    let list = child.request(2, "tools/list", json!({})).await;
    assert_catalogue(tools(&list));
    let accounts = text_result(
        &child
            .request(
                3,
                "tools/call",
                json!({"name":"list_accounts","arguments":{}}),
            )
            .await,
    );
    assert_eq!(accounts["accounts"], json!([]));
    assert_eq!(accounts["backend"], "grammers");
    let stderr = child.finish().await;
    assert!(
        stderr.is_empty(),
        "Offline MCP operation unexpectedly emitted stderr output"
    );
    fixture.assert_no_telegram_sessions();
}
#[tokio::test]
async fn stdio_supports_2024_11_05_initialize_and_offline_tools() {
    legacy("2024-11-05").await;
}
#[tokio::test]
async fn stdio_supports_latest_protocol_with_initialize() {
    legacy(rmcp::model::ProtocolVersion::LATEST_WITH_INITIALIZE.as_str()).await;
}

#[tokio::test]
async fn latest_inline_protocol_works_without_initialize_and_requires_request_metadata() {
    let fixture = Fixture::new();
    let mut child = McpProcess::start(fixture.command(&["serve"]));
    let list = child.request(1, "tools/list", latest_meta(json!({}))).await;
    assert_catalogue(tools(&list));
    let missing = child.request(2, "tools/list", json!({})).await;
    assert!(
        missing.get("error").is_some(),
        "The newest protocol must reject absent per-request metadata"
    );
    let accounts = text_result(
        &child
            .request(
                3,
                "tools/call",
                latest_meta(json!({"name":"list_accounts","arguments":{}})),
            )
            .await,
    );
    assert_eq!(accounts["accounts"], json!([]));
    let stderr = child.finish().await;
    assert!(stderr.is_empty());
    fixture.assert_no_telegram_sessions();
}

#[tokio::test]
async fn unknown_tool_name_and_sensitive_arguments_never_reach_logs_or_error_echo() {
    let fixture = Fixture::new();
    let mut child = McpProcess::start(fixture.command(&[]));
    child.initialize("2024-11-05").await;
    let caller = format!("unknown_{:032x}", u128::from_be_bytes(*b"test-caller-name"));
    let payload = format!(
        "session_payload_{:032x}",
        u128::from_be_bytes(*b"test-session-msg")
    );
    let response = child
        .request(
            2,
            "tools/call",
            json!({"name":caller,"arguments":{"api_hash":payload,"text":payload}}),
        )
        .await;
    assert!(response.get("error").is_some() || response["result"]["isError"] == true);
    let printed = response.to_string();
    assert!(!printed.contains(&caller));
    assert!(!printed.contains(&payload));
    let stderr = child.finish().await;
    assert!(!stderr.contains(&caller));
    assert!(!stderr.contains(&payload));
    let log = std::fs::read_to_string(fixture.state.join("mcp.log"))
        .expect("Unknown tool failures must still produce a private operational log");
    assert!(log.contains("Tool invocation failed"));
    fixture.assert_logs_omit(&[&caller, &payload]);
    fixture.assert_no_telegram_sessions();
}

async fn selector(mode: &str) -> Vec<Value> {
    let fixture = Fixture::new();
    let mut command = fixture.command(&[]);
    command.env("TELEGRAM_EXPOSED_TOOLS", mode);
    let mut child = McpProcess::start(command);
    child.initialize("2024-11-05").await;
    let response = child.request(2, "tools/list", json!({})).await;
    let rows = tools(&response).to_vec();
    child.finish().await;
    fixture.assert_no_telegram_sessions();
    rows
}
#[tokio::test]
async fn read_only_selector_reaches_the_actual_server_process() {
    let rows = selector("read-only").await;
    assert!(!rows.is_empty());
    assert!(rows.len() < catalogue_names().len());
    assert!(
        rows.iter()
            .all(|tool| tool["annotations"]["readOnlyHint"] == true)
    );
    assert!(!rows.iter().any(|tool| tool["name"] == "send_message"));
}
#[tokio::test]
async fn combined_read_only_allow_and_exclude_lists_are_applied() {
    let rows = selector("read-only+send_message-get_me").await;
    assert!(rows.iter().any(|tool| tool["name"] == "send_message"));
    assert!(!rows.iter().any(|tool| tool["name"] == "get_me"));
    assert!(
        rows.iter()
            .all(|tool| tool["name"] == "send_message"
                || tool["annotations"]["readOnlyHint"] == true)
    );
}
#[tokio::test]
async fn all_selector_can_exclude_a_named_tool() {
    let rows = selector("all-send_message").await;
    assert_eq!(rows.len(), catalogue_names().len() - 1);
    assert!(!rows.iter().any(|tool| tool["name"] == "send_message"));
}
