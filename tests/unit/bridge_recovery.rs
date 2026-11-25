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
}

impl ServerHandler for ToolOwner {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult::default())
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
    let manager = Arc::new(http::session_manager(Duration::from_millis(100)));
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
    assert_eq!(manager.sessions.read().await.len(), 1);
    tokio::time::timeout(LIMIT, async {
        while !manager.sessions.read().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owner did not expire the actual MCP session");

    let result = bridge.forward(call("test"), Instant::now() + LIMIT).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(expires.load(Ordering::Relaxed), 1);
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
        .forward(call("reject"), Instant::now())
        .await
        .unwrap_err();
    assert_eq!(expired.message, REQUEST_TIMED_OUT);
    let expected = rejection();
    let error = bridge
        .forward(call("reject"), Instant::now() + LIMIT)
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
    let tool = ToolOwner::default();
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

#[tokio::test]
async fn rotated_owner_reconnects_concurrent_calls_and_retries_the_racing_401_once() {
    let (_temp, state) = state();
    let _lease = owner_lease(&state);
    let control = Arc::new(Control::default());
    let (old, old_record, old_calls, _) =
        fixture_owner(&state, TEST_TOKEN, Some(Arc::clone(&control))).await;
    let bridge = Arc::new(bridge_for(Arc::clone(&state), &old, old_record).await);
    let caller = Arc::clone(&bridge);
    let racing =
        tokio::spawn(async move { caller.forward(call("race"), Instant::now() + LIMIT).await });
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
                .forward(call("parallel"), Instant::now() + LIMIT)
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
        .forward(call("mutation"), Instant::now() + Duration::from_secs(2))
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
            .forward(call("next"), Instant::now() + LIMIT)
            .await
            .is_ok()
    );
    assert_eq!(inits.load(Ordering::Relaxed), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
