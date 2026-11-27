use super::*;
use axum::{Router, extract::Request, http::StatusCode, middleware, response::IntoResponse};
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::atomic::AtomicUsize;
use tokio::{net::TcpListener, sync::Notify, task::JoinHandle, time::Instant};

const TEST_TOKEN: &str = "synthetic-private-discovery-0123456789";
const LIMIT: Duration = Duration::from_secs(8);

#[derive(Clone, Default)]
struct ToolOwner {
    calls: Arc<AtomicUsize>,
    tools: Vec<Tool>,
    instructions: Option<String>,
}

impl ServerHandler for ToolOwner {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions = self.instructions.clone();
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.clone(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if request.name == "reject" {
            return Err(rejection());
        }
        Ok(CallToolResult::success(vec![ContentBlock::text("owner reply")]).into())
    }
}

fn rejection() -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params("owner rejected parameters", Some(json!({"field":"target"})))
}

struct HttpFixture {
    url: String,
    task: JoinHandle<std::io::Result<()>>,
}

impl HttpFixture {
    async fn serve(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await });
        Self { url, task }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn state() -> (tempfile::TempDir, Arc<State>) {
    let temp = tempfile::tempdir().unwrap();
    let state = State::at(temp.path().join("state"), vec![temp.path().to_owned()]).unwrap();
    (temp, Arc::new(state))
}

fn record(url: &str) -> Value {
    json!({"url":url,"pid":std::process::id(), "started":"synthetic-first-owner", "token":TEST_TOKEN})
}

async fn bridge_for(state: Arc<State>, owner: &HttpFixture, record: Value) -> Bridge {
    state.write_json(DISCOVERY, &record).unwrap();
    let remote = ()
        .serve(StreamableHttpClientTransport::with_client(
            http::local_http_client(None).unwrap(),
            client_configuration(&owner.url, TEST_TOKEN),
        ))
        .await
        .unwrap();
    assert!(!remote.is_closed());
    Bridge::new(
        state,
        BridgeSession {
            remote,
            identity: DiscoveryIdentity(record),
            tools: Vec::new(),
            instructions: None,
            stale: AtomicBool::new(false),
        },
    )
}

fn call(name: &'static str) -> CallToolRequestParams {
    CallToolRequestParams::new(name)
}

#[tokio::test]
async fn expired_idle_session_returns_404_and_bridge_reinitializes_once() {
    let (_temp, state) = state();
    let owner = ToolOwner::default();
    let calls = Arc::clone(&owner.calls);
    let manager = Arc::new(http::session_manager(Duration::from_secs(3600)));
    let service = StreamableHttpService::new(
        move || Ok(owner.clone()),
        Arc::clone(&manager),
        StreamableHttpServerConfig::default(),
    );
    let expires = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&expires);
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(
            move |request: Request, next: middleware::Next| {
                let count = Arc::clone(&count);
                async move {
                    let is_post = request.method() == axum::http::Method::POST;
                    let response = next.run(request).await;
                    if is_post && response.status() == StatusCode::NOT_FOUND {
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                    response
                }
            },
        ));
    let owner = HttpFixture::serve(router).await;
    let bridge = bridge_for(state, &owner, record(&owner.url)).await;
    use rmcp::transport::streamable_http_server::session::SessionManager;
    let original = {
        let sessions = manager.sessions.read().await;
        assert_eq!(sessions.len(), 1);
        sessions.keys().next().unwrap().clone()
    };
    // Expire only the original session. Its replacement must survive a slow CI handshake.
    tokio::time::timeout(LIMIT, manager.close_session(&original))
        .await
        .expect("original MCP session did not close")
        .unwrap();
    assert!(manager.sessions.read().await.is_empty());

    let result = bridge
        .forward(call("test"), Some(Instant::now() + LIMIT))
        .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(expires.load(Ordering::Relaxed), 1);
    let replacements = manager.sessions.read().await;
    assert_eq!(replacements.len(), 1);
    assert!(!replacements.contains_key(&original));
    drop(replacements);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    drop(bridge);
    drop(owner);
}

#[tokio::test]
async fn bridge_returns_the_original_mcp_code_message_and_data() {
    let (_temp, state) = state();
    let tool = ToolOwner::default();
    let calls = Arc::clone(&tool.calls);
    let service = StreamableHttpService::new(
        move || Ok(tool.clone()),
        Arc::new(http::session_manager(LIMIT)),
        StreamableHttpServerConfig::default(),
    );
    let owner = HttpFixture::serve(Router::new().nest_service("/mcp", service)).await;
    let bridge = bridge_for(state, &owner, record(&owner.url)).await;
    let expired = bridge
        .forward(call("reject"), Some(Instant::now()))
        .await
        .unwrap_err();
    assert_eq!(expired.message, REQUEST_TIMED_OUT);
    let expected = rejection();
    let error = bridge
        .forward(call("reject"), Some(Instant::now() + LIMIT))
        .await
        .unwrap_err();
    assert_eq!(error.code, expected.code);
    assert_eq!(error.message, expected.message);
    assert_eq!(error.data, expected.data);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(bridge.current().unwrap().is_some());
    drop(bridge);
    drop(owner);
}

#[derive(Default)]
struct Control {
    arrived: Notify,
    release: Notify,
    posts: AtomicUsize,
    lose_reply: bool,
}

async fn fixture_owner(
    state: &State,
    token: &str,
    control: Option<Arc<Control>>,
) -> (HttpFixture, Value, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    fixture_owner_with_handler(state, token, control, ToolOwner::default()).await
}

async fn fixture_owner_with_handler(
    state: &State,
    token: &str,
    control: Option<Arc<Control>>,
    tool: ToolOwner,
) -> (HttpFixture, Value, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let calls = Arc::clone(&tool.calls);
    let inits = Arc::new(AtomicUsize::new(0));
    let init_count = Arc::clone(&inits);
    let service = StreamableHttpService::new(
        move || Ok(tool.clone()),
        Arc::new(http::session_manager(LIMIT)),
        StreamableHttpServerConfig::default(),
    );
    let router = Router::new().nest_service("/mcp", service).route(
        "/health",
        axum::routing::get(|| async {
            axum::Json(json!({"ready":true,"backend":"grammers","pid":std::process::id()}))
        }),
    );
    let router = http::authenticated_router(router, token.to_owned(), None).layer(
        middleware::from_fn(move |request: Request, next: middleware::Next| {
            let control = control.clone();
            let init_count = Arc::clone(&init_count);
            async move {
                if request.method() != axum::http::Method::POST {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 65536).await.unwrap();
                let json: Value = serde_json::from_slice(&bytes).unwrap();
                let request = Request::from_parts(parts, axum::body::Body::from(bytes));
                if json["method"] == "initialize" {
                    init_count.fetch_add(1, Ordering::Relaxed);
                }
                if json["method"] == "tools/call"
                    && let Some(control) = control
                {
                    control.posts.fetch_add(1, Ordering::Relaxed);
                    if control.lose_reply {
                        let response = next.run(request).await;
                        assert_eq!(response.status(), StatusCode::OK);
                        // The handler ran; only its acknowledgement is lost.
                        return axum::response::Response::builder()
                            .header("content-type", "application/json")
                            .body(axum::body::Body::from("{"))
                            .unwrap();
                    }
                    control.arrived.notify_one();
                    control.release.notified().await;
                    return (
                        StatusCode::UNAUTHORIZED,
                        [(
                            axum::http::header::WWW_AUTHENTICATE,
                            http::OWNER_BEARER_CHALLENGE,
                        )],
                    )
                        .into_response();
                }
                next.run(request).await
            }
        }),
    );
    let owner = HttpFixture::serve(router).await;
    let value = json!({"url":owner.url,"pid":std::process::id(),"started":"synthetic-generation","token":token,"health_protocol":2});
    state.write_json(DISCOVERY, &value).unwrap();
    (owner, value, calls, inits)
}

fn owner_lease(state: &State) -> std::fs::File {
    let path = state.root().join("http-service.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&file).unwrap();
    file
}

fn owner_with_catalogue(names: &[&str], instructions: &str) -> ToolOwner {
    let tools = names
        .iter()
        .map(|name| {
            serde_json::from_value(json!({
                "name": name,
                "description": format!("Synthetic {name} capability"),
                "inputSchema": {"type": "object", "properties": {}}
            }))
            .unwrap()
        })
        .collect();
    ToolOwner {
        tools,
        instructions: Some(instructions.to_owned()),
        ..Default::default()
    }
}

async fn wire_tools(client: &RunningService<RoleClient, ()>) -> Vec<Tool> {
    // Use the wire request so a client-side catalogue cache cannot hide bridge behavior.
    let response = tokio::time::timeout(
        LIMIT,
        client.send_request(ClientRequest::ListToolsRequest(ListToolsRequest {
            method: Default::default(),
            params: None,
            extensions: Default::default(),
        })),
    )
    .await
    .expect("The bridge tool listing exceeded its fixture deadline")
    .unwrap();
    let ServerResult::ListToolsResult(result) = response else {
        panic!("The bridge did not return a tool catalogue");
    };
    result.tools
}

async fn duplex_client<S: ServerHandler + Send + Sync + 'static>(
    handler: S,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, S>,
) {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let serving = tokio::spawn(async move { handler.serve(server_io).await.unwrap() });
    let client = tokio::time::timeout(LIMIT, ().serve(client_io))
        .await
        .unwrap()
        .unwrap();
    let server = tokio::time::timeout(LIMIT, serving).await.unwrap().unwrap();
    (client, server)
}

#[tokio::test]
async fn tool_listing_refreshes_replaced_owner_before_any_tool_call() {
    let (_temp, state) = state();
    let _lease = owner_lease(&state);
    let original = owner_with_catalogue(&["previous_tool"], "Original owner instructions");
    let original_tools = original.tools.clone();
    let (old, _, old_calls, old_inits) =
        fixture_owner_with_handler(&state, TEST_TOKEN, None, original).await;
    let session = tokio::time::timeout(LIMIT, BridgeSession::attach(&state))
        .await
        .unwrap()
        .unwrap();
    let bridge = Arc::new(Bridge::new(Arc::clone(&state), session));
    let (client, server) = duplex_client(Arc::clone(&bridge)).await;
    assert_eq!(
        client.peer_info().unwrap().instructions.as_deref(),
        Some("Original owner instructions")
    );
    assert_eq!(wire_tools(&client).await, original_tools);

    let replacement = owner_with_catalogue(
        &["replacement_read", "replacement_write"],
        "Replacement owner instructions",
    );
    let replacement_tools = replacement.tools.clone();
    let (_replacement, _, replacement_calls, replacement_inits) = fixture_owner_with_handler(
        &state,
        "synthetic-replacement-owner-0123456789",
        None,
        replacement,
    )
    .await;
    // Keep the old HTTP owner healthy: the new private discovery record must trigger refresh.
    assert!(!old.task.is_finished());
    let (first, second, third, fourth) = tokio::join!(
        wire_tools(&client),
        wire_tools(&client),
        wire_tools(&client),
        wire_tools(&client),
    );
    for tools in [first, second, third, fourth] {
        assert_eq!(
            tools, replacement_tools,
            "tools/list returned the previous owner's capabilities"
        );
    }
    let discovered = tokio::time::timeout(
        LIMIT,
        client.discover(RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::from_build_env(),
            ClientCapabilities::default(),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        discovered.instructions.as_deref(),
        Some("Replacement owner instructions")
    );
    assert_eq!(wire_tools(&client).await, replacement_tools);
    assert_eq!(old_inits.load(Ordering::Relaxed), 1);
    assert_eq!(replacement_inits.load(Ordering::Relaxed), 1);
    assert_eq!(old_calls.load(Ordering::Relaxed), 0);
    assert_eq!(replacement_calls.load(Ordering::Relaxed), 0);
    client.cancel().await.unwrap();
    server.cancel().await.unwrap();
}

#[derive(Default)]
struct WaitingControl {
    entered: Notify,
    release: Notify,
    cancelled: Notify,
    calls: AtomicUsize,
    completed: AtomicUsize,
}

struct WaitingOwner(Arc<WaitingControl>);

impl ServerHandler for WaitingOwner {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        self.0.entered.notify_one();
        tokio::select! {
            biased;
            () = context.ct.cancelled() => {
                self.0.cancelled.notify_one();
                Err(rmcp::ErrorData::internal_error("Synthetic owner cancelled", None))
            }
            () = self.0.release.notified() => {
                self.0.completed.fetch_add(1, Ordering::Relaxed);
                Ok(CallToolResult::success(vec![ContentBlock::text("owner completed once")]).into())
            }
        }
    }
}

struct WaitingFixture {
    _temporary: tempfile::TempDir,
    bridge: Arc<Bridge>,
    client: RunningService<RoleClient, ()>,
    server: RunningService<RoleServer, Arc<Bridge>>,
    owner: RunningService<RoleServer, WaitingOwner>,
    control: Arc<WaitingControl>,
}

impl WaitingFixture {
    async fn new() -> Self {
        Self::with_policy(None).await
    }

    async fn with_policy(policy: Option<Value>) -> Self {
        let (temporary, state) = state();
        let control = Arc::new(WaitingControl::default());
        let (remote, owner) = duplex_client(WaitingOwner(Arc::clone(&control))).await;
        let mut identity = record("http://127.0.0.1:1/mcp");
        if let Some(policy) = policy {
            identity["request_timeouts"] = policy;
        }
        state.write_json(DISCOVERY, &identity).unwrap();
        let bridge = Arc::new(Bridge::new(
            state,
            BridgeSession {
                remote,
                identity: DiscoveryIdentity(identity),
                tools: Vec::new(),
                instructions: None,
                stale: AtomicBool::new(false),
            },
        ));
        let (client, server) = duplex_client(Arc::clone(&bridge)).await;
        Self {
            _temporary: temporary,
            bridge,
            client,
            server,
            owner,
            control,
        }
    }

    fn context(&self, id: i64) -> RequestContext<RoleServer> {
        RequestContext::new(RequestId::Number(id), self.server.peer().clone())
    }

    async fn close(self) {
        self.client.cancel().await.unwrap();
        self.server.cancel().await.unwrap();
        self.owner.cancel().await.unwrap();
    }
}

const BUDGET_CASE_ENV: &str = "TELEGRAM_MCP_TEST_BRIDGE_BUDGET_CASE";

fn isolated_budget_cases(test: &str, cases: &[(&str, Option<&str>)]) {
    // Isolate process configuration; the parent test never changes its environment.
    let mut failures = Vec::new();
    for &(case, timeout) in cases {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(BUDGET_CASE_ENV, case)
            .env_remove("TELEGRAM_APPROVAL_BOT_TOKEN")
            .env_remove("TELEGRAM_APPROVAL_OWNER_IDS")
            .env_remove("TELEGRAM_APPROVAL_OWNER_ID")
            // A timeout setting without a configured bot must retain the 300s default.
            .env("TELEGRAM_APPROVAL_TIMEOUT_SECONDS", "5")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(timeout) = timeout {
            command.env("TELEGRAM_TOOL_TIMEOUT_SECONDS", timeout);
        } else {
            command.env_remove("TELEGRAM_TOOL_TIMEOUT_SECONDS");
        }
        let mut child = command.spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Bridge budget fixture child exceeded its deadline: {case}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        if !output.status.success() {
            failures.push(format!(
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(start_paused = true)]
async fn bridge_wait_budget_preserves_approval_long_calls_and_disabled_timeout() {
    let Ok(case) = std::env::var(BUDGET_CASE_ENV) else {
        isolated_budget_cases(
            "server::bridge::recovery_tests::bridge_wait_budget_preserves_approval_long_calls_and_disabled_timeout",
            &[
                ("nonbot-approval", None),
                ("long-tool", Some("600")),
                ("disabled-tool", Some("0")),
            ],
        );
        return;
    };
    let (wait, ordinary, export) = match case.as_str() {
        "nonbot-approval" => (200, json!(55.0), json!(120.0)),
        // A 600s ordinary budget can cover 550s of preflight, a 250s approval
        // wait, and another 550s after the owner resets the ordinary budget.
        "long-tool" => (1350, json!(600.0), json!(600.0)),
        "disabled-tool" => (2000, Value::Null, Value::Null),
        _ => panic!("Unknown isolated bridge budget fixture"),
    };
    let policy = json!({"version":1,"ordinary_seconds":ordinary,"export_seconds":export,"approval_seconds":300.0});
    assert_owner_wait(
        &case,
        wait,
        Some(policy),
        case != "disabled-tool",
        "mutation",
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn bridge_wait_budget_uses_the_attached_owner_policy() {
    let Ok(case) = std::env::var(BUDGET_CASE_ENV) else {
        isolated_budget_cases(
            "server::bridge::recovery_tests::bridge_wait_budget_uses_the_attached_owner_policy",
            &[
                ("owner-long-approval", None),
                ("owner-long-tool", None),
                ("owner-long-export", None),
                ("owner-disabled-tool", None),
                ("legacy-owner", None),
                ("unknown-policy-version", None),
                ("oversized-tool-policy", None),
                ("oversized-approval-policy", None),
                ("partial-policy", None),
            ],
        );
        return;
    };
    let (wait, policy, finite, tool) = match case.as_str() {
        "owner-long-approval" => (
            3000,
            Some(
                json!({"version":1,"ordinary_seconds":55.0,"export_seconds":120.0,"approval_seconds":3600.0}),
            ),
            true,
            "mutation",
        ),
        "owner-long-tool" => (
            1350,
            Some(
                json!({"version":1,"ordinary_seconds":600.0,"export_seconds":600.0,"approval_seconds":300.0}),
            ),
            true,
            "mutation",
        ),
        "owner-long-export" => (
            1350,
            Some(
                json!({"version":1,"ordinary_seconds":55.0,"export_seconds":600.0,"approval_seconds":300.0}),
            ),
            true,
            "export_secret_chat",
        ),
        "owner-disabled-tool" => (
            2000,
            Some(
                json!({"version":1,"ordinary_seconds":null,"export_seconds":null,"approval_seconds":300.0}),
            ),
            false,
            "mutation",
        ),
        "legacy-owner" => (2000, None, false, "mutation"),
        "unknown-policy-version" => (
            2000,
            Some(
                json!({"version":2,"ordinary_seconds":55.0,"export_seconds":120.0,"approval_seconds":300.0}),
            ),
            false,
            "mutation",
        ),
        "oversized-tool-policy" => (
            2000,
            Some(
                json!({"version":1,"ordinary_seconds":1e300,"export_seconds":1e300,"approval_seconds":300.0}),
            ),
            false,
            "mutation",
        ),
        "oversized-approval-policy" => (
            2000,
            Some(
                json!({"version":1,"ordinary_seconds":55.0,"export_seconds":120.0,"approval_seconds":1e300}),
            ),
            false,
            "mutation",
        ),
        "partial-policy" => (
            2000,
            Some(json!({"version":1,"ordinary_seconds":55.0})),
            false,
            "mutation",
        ),
        _ => panic!("Unknown isolated owner policy fixture"),
    };
    assert_owner_wait(&case, wait, policy, finite, tool).await;
}

async fn assert_owner_wait(
    case: &str,
    wait: u64,
    policy: Option<Value>,
    finite: bool,
    tool: &'static str,
) {
    let fixture = WaitingFixture::with_policy(policy).await;
    let mut operation = Box::pin(fixture.bridge.call_tool(call(tool), fixture.context(1)));
    assert!(futures_util::poll!(operation.as_mut()).is_pending());
    fixture.control.entered.notified().await;
    tokio::time::advance(Duration::from_secs(wait)).await;
    assert!(
        futures_util::poll!(operation.as_mut()).is_pending(),
        "The bridge abandoned a valid owner wait in case {case} after {wait}s"
    );
    fixture.control.release.notify_one();
    let response = tokio::time::timeout(LIMIT, operation)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(response, CallToolResponse::Complete(_)));
    assert_eq!(fixture.control.calls.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.control.completed.load(Ordering::Relaxed), 1);

    if !finite {
        // Disabling the ordinary budget must not disable bounded connection recovery.
        let lock = fixture.bridge.reconnect.lock().await;
        fixture
            .bridge
            .cached()
            .unwrap()
            .stale
            .store(true, Ordering::Release);
        let mut attachment = Box::pin(
            fixture
                .bridge
                .call_tool(call("never-dispatched"), fixture.context(2)),
        );
        assert!(futures_util::poll!(attachment.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(120)).await;
        let result = futures_util::poll!(attachment.as_mut());
        assert!(
            matches!(result, std::task::Poll::Ready(Err(_))),
            "Disabled ordinary timeout also disabled the bridge attachment deadline"
        );
        assert_eq!(fixture.control.calls.load(Ordering::Relaxed), 1);
        drop(attachment);
        drop(lock);
    } else {
        // An enabled ordinary budget still bounds an owner that never replies.
        let mut stalled = Box::pin(
            fixture
                .bridge
                .call_tool(call("stalled-mutation"), fixture.context(2)),
        );
        assert!(futures_util::poll!(stalled.as_mut()).is_pending());
        fixture.control.entered.notified().await;
        tokio::time::advance(Duration::from_secs(7200)).await;
        assert!(
            matches!(
                futures_util::poll!(stalled.as_mut()),
                std::task::Poll::Ready(Err(_))
            ),
            "The bridge removed the finite owner request deadline"
        );
        assert_eq!(fixture.control.calls.load(Ordering::Relaxed), 2);
        assert_eq!(fixture.control.completed.load(Ordering::Relaxed), 1);
        drop(stalled);
    }
    fixture.close().await;
}

#[tokio::test(start_paused = true)]
async fn bridge_cancellation_reaches_the_owner_without_replaying_the_call() {
    let fixture = WaitingFixture::new().await;
    let context = fixture.context(1);
    let cancellation = context.ct.clone();
    let mut operation = Box::pin(fixture.bridge.call_tool(call("mutation"), context));
    assert!(futures_util::poll!(operation.as_mut()).is_pending());
    fixture.control.entered.notified().await;
    cancellation.cancel();
    let response = tokio::time::timeout(Duration::from_secs(1), operation)
        .await
        .expect("The bridge did not stop the cancelled request");
    assert!(response.is_err());
    tokio::time::timeout(Duration::from_secs(1), fixture.control.cancelled.notified())
        .await
        .expect("The owner did not receive cancellation of its pending request");
    assert_eq!(fixture.control.calls.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.control.completed.load(Ordering::Relaxed), 0);
    fixture.close().await;
}

#[tokio::test]
async fn rotated_owner_reconnects_concurrent_calls_and_retries_the_racing_401_once() {
    let (_temp, state) = state();
    let _lease = owner_lease(&state);
    let control = Arc::new(Control::default());
    let (old, old_record, old_calls, _) =
        fixture_owner(&state, TEST_TOKEN, Some(Arc::clone(&control))).await;
    let bridge = Arc::new(bridge_for(Arc::clone(&state), &old, old_record).await);
    let caller = Arc::clone(&bridge);
    let racing = tokio::spawn(async move {
        caller
            .forward(call("race"), Some(Instant::now() + LIMIT))
            .await
    });
    tokio::time::timeout(LIMIT, control.arrived.notified())
        .await
        .unwrap();
    let (_replacement, _, new_calls, new_inits) =
        fixture_owner(&state, "synthetic-replacement-owner-0123456789", None).await;
    let mut concurrent = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let caller = Arc::clone(&bridge);
        concurrent.spawn(async move {
            caller
                .forward(call("parallel"), Some(Instant::now() + LIMIT))
                .await
        });
    }
    control.release.notify_one();
    let answer = racing.await.unwrap();
    assert!(answer.is_ok(), "{answer:?}");
    while let Some(answer) = concurrent.join_next().await {
        let answer = answer.unwrap();
        assert!(answer.is_ok(), "{answer:?}");
    }
    assert_eq!(new_inits.load(Ordering::Relaxed), 1);
    assert_eq!(new_calls.load(Ordering::Relaxed), 5);
    assert_eq!(control.posts.load(Ordering::Relaxed), 1);
    assert_eq!(old_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn lost_acknowledgement_does_not_replay_and_the_next_call_reconnects() {
    let (_temp, state) = state();
    let _lease = owner_lease(&state);
    let control = Arc::new(Control {
        lose_reply: true,
        ..Default::default()
    });
    let (old, old_record, old_calls, _) =
        fixture_owner(&state, TEST_TOKEN, Some(Arc::clone(&control))).await;
    let bridge = bridge_for(Arc::clone(&state), &old, old_record).await;
    let response = bridge
        .forward(
            call("mutation"),
            Some(Instant::now() + Duration::from_secs(2)),
        )
        .await;
    let error = response.unwrap_err();
    assert!(
        matches!(error.message.as_ref(), REQUEST_FAILED | REQUEST_TIMED_OUT),
        "{error:?}"
    );
    assert_eq!(old_calls.load(Ordering::Relaxed), 1);
    assert_eq!(control.posts.load(Ordering::Relaxed), 1);
    assert!(bridge.current().unwrap().is_none());
    let (_replacement, _, calls, inits) =
        fixture_owner(&state, "synthetic-replacement-owner-0123456789", None).await;
    assert!(
        bridge
            .forward(call("next"), Some(Instant::now() + LIMIT))
            .await
            .is_ok()
    );
    assert_eq!(inits.load(Ordering::Relaxed), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
