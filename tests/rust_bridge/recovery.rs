use super::*;

#[tokio::test]
async fn bridge_reuses_stdio_after_the_owner_exits_during_idle() {
    let fixture = Fixture::new();
    fixture.env_file(&format!("MCP_PORT={}\n", port()));
    let mut bridge = McpProcess::start(fixture.command(&["bridge"]));
    bridge.initialize("2024-11-05").await;
    let first = discovery(&fixture);
    Daemon::owned(&first).stop(&fixture).await;

    let answer = bridge
        .request(
            2,
            "tools/call",
            latest_meta(json!({"name":"list_accounts","arguments":{}})),
        )
        .await;
    let next = discovery(&fixture);
    let replacement = (next["pid"] != first["pid"]).then(|| Daemon::owned(&next));
    assert_ne!(next["pid"], first["pid"], "No replacement owner published");
    assert_ne!(
        next["token"], first["token"],
        "Owner bearer was not rotated"
    );
    let replacement = replacement.unwrap();
    assert!(answer.get("error").is_none(), "{answer}");
    assert_ne!(answer["result"]["isError"], true, "{answer}");
    assert_eq!(health(&next).await["pid"], next["pid"]);
    assert!(bridge.finish().await.is_empty());
    replacement.stop(&fixture).await;
}

#[tokio::test]
async fn owner_auth_rejection_is_typed_but_tools_call_domain_errors_are_http_200() {
    let fixture = Fixture::new();
    let mut owner = fixture
        .command(&["serve", "--http", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let record = tokio::time::timeout(DEADLINE, async {
        loop {
            if let Ok(bytes) = std::fs::read(fixture.state.join("http-service.json"))
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && value["pid"].as_u64() == owner.id().map(u64::from)
            {
                break value;
            }
            assert!(owner.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let daemon = Daemon::owned(&record);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(DEADLINE)
        .build()
        .unwrap();
    let url = record["url"].as_str().unwrap();
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"recovery-check","version":"1"}}});
    let refused = client.post(url).json(&initialize).send().await.unwrap();
    assert_eq!(refused.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        refused
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|header| header.to_str().ok()),
        Some("Bearer realm=\"telegram-mcp\"")
    );

    let token = record["token"].as_str().unwrap();
    let initialized = client
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .unwrap();
    assert_eq!(initialized.status(), reqwest::StatusCode::OK);
    let session = initialized.headers()["mcp-session-id"].clone();
    let _ = initialized.bytes().await.unwrap();
    let initialized = client
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .header("mcp-protocol-version", "2024-11-05")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert_eq!(initialized.status(), reqwest::StatusCode::ACCEPTED);
    let response = client
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .header("mcp-protocol-version", "2024-11-05")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"not_a_telegram_tool","arguments":{}}}))
        .send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.unwrap();
    let body = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|message| message["id"] == 2)
        .or_else(|| serde_json::from_str(&body).ok())
        .expect("The owner did not return a terminal tools/call reply");
    assert_eq!(body["result"]["isError"], true, "{body}");
    daemon.stop(&fixture).await;
    tokio::time::timeout(DEADLINE, owner.wait())
        .await
        .unwrap()
        .unwrap();
}
