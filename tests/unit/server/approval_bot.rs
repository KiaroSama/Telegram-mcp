use super::*;
use axum::{
    Json, Router,
    extract::{Path, State as AxumState},
    routing::post,
};
use std::collections::VecDeque;
use tokio::{sync::Notify, task::JoinHandle};

#[derive(Default)]
struct Wire {
    sent: StdMutex<Vec<(String, Value)>>,
    queue: StdMutex<VecDeque<Value>>,
    changed: Notify,
    updates: Notify,
    fail_send: bool,
}

impl Wire {
    async fn message(&self, method: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if let Some((_, body)) = self
                    .sent
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(m, v)| m == method && predicate(v))
                {
                    return body.clone();
                }
                changed.await;
            }
        })
        .await
        .expect("Expected a Bot API request")
    }

    fn push(&self, update: Value) {
        self.queue.lock().unwrap().push_back(update);
        self.updates.notify_one();
    }
}

async fn bot_api(
    Path((token, method)): Path<(String, String)>,
    AxumState(wire): AxumState<Arc<Wire>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_eq!(token, "bot7:fixture-only");
    let count = {
        let mut sent = wire.sent.lock().unwrap();
        if method != "getUpdates" {
            sent.push((method.clone(), body.clone()));
        }
        sent.len()
    };
    wire.changed.notify_waiters();
    let result = match method.as_str() {
        "getMe" => json!({"id":7,"is_bot":true,"username":"Resolved_Helper_Bot"}),
        "getUpdates" => {
            let _ = tokio::time::timeout(Duration::from_millis(100), wire.updates.notified()).await;
            json!(wire.queue.lock().unwrap().drain(..).collect::<Vec<_>>())
        }
        "sendMessage" if wire.fail_send => {
            return Json(json!({"ok":false,"description":"7:fixture-only"}));
        }
        "sendMessage" => json!({"message_id":count}),
        "editMessageText" | "answerCallbackQuery" | "setMyCommands" => json!(true),
        other => panic!("Unexpected Bot API method: {other}"),
    };
    Json(json!({"ok":true,"result":result}))
}

struct Fixture {
    server: Mcp,
    api: Arc<Probe>,
    wire: Arc<Wire>,
    _temp: TempDir,
    _worker: tokio_util::sync::DropGuard,
    http: JoinHandle<()>,
}

impl Fixture {
    async fn new(fail_send: bool, timeout: Duration) -> Self {
        let wire = Arc::new(Wire {
            fail_send,
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/{token}/{method}", post(bot_api))
            .with_state(Arc::clone(&wire));
        let http = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (mut server, api, temp) = service(json!({}), access::AccessPolicy::default());
        let runtime = Arc::get_mut(&mut server.runtime).unwrap();
        let bot = crate::safeguard::bot::ApprovalBot::for_loopback(
            Arc::clone(&runtime.state),
            address,
            timeout,
        );
        let worker = bot.start();
        runtime.guard.bot = Some(bot);
        Self {
            server,
            api,
            wire,
            _temp: temp,
            _worker: worker,
            http,
        }
    }

    fn execute(&self, tool: &'static str, args: Value) -> JoinHandle<Result<Value>> {
        let server = self.server.clone();
        tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(6), server.execute(tool, args))
                .await
                .unwrap()
        })
    }

    async fn prompt(&self) -> Value {
        self.wire
            .message("sendMessage", |body| {
                body["chat_id"] == 101
                    && body["text"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with("<b>Telegram MCP approval")
            })
            .await
    }

    fn callback(&self, id: i64, owner: i64, data: &str) {
        self.wire.push(json!({"update_id":id,"callback_query":{"id":id.to_string(),"from":{"id":owner},"data":data,"message":{"chat":{"id":owner,"type":"private"}}}}));
    }

    fn command(&self, id: i64, owner: i64, text: &str) {
        self.wire.push(json!({"update_id":id,"message":{"from":{"id":owner},"text":text,"chat":{"id":owner,"type":"private"}}}));
    }

    fn calls(&self, method: &str) -> usize {
        self.api
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == method)
            .count()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.http.abort();
    }
}

fn button(prompt: &Value, suffix: &str) -> String {
    prompt["reply_markup"]["inline_keyboard"][0]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|button| button["callback_data"].as_str())
        .find(|data| data.ends_with(suffix))
        .unwrap()
        .to_owned()
}

fn delete_args() -> Value {
    json!({"account":"default","chat_id":42,"message_id":20,"revoke":true})
}

#[tokio::test]
async fn owner_once_wait_releases_account_lock_and_ignores_non_owner() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    let pending = f.execute("delete_message", delete_args());
    let prompt = f.prompt().await;
    let once = button(&prompt, ":once");
    assert!(once.len() <= 64);
    f.callback(1, 303, &once);
    f.command(2, 303, "/status");
    f.command(3, 101, "/status");
    f.wire
        .message("sendMessage", |body| {
            body["chat_id"] == 101
                && body["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("Pending requests: 1")
        })
        .await;
    assert_eq!(f.calls("channels.deleteMessages"), 0);
    assert!(!pending.is_finished());
    let duplicate = f
        .execute("delete_message", delete_args())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(duplicate["performed"], false);
    tokio::time::timeout(
        Duration::from_millis(500),
        f.server
            .execute("get_history", json!({"account":"default","chat_id":42})),
    )
    .await
    .expect("A bot wait held the account lock")
    .unwrap();
    let slot = f.server.account("default").await.unwrap();
    let lock = slot.serial.lock().await;
    f.callback(4, 101, &once);
    f.wire
        .message("answerCallbackQuery", |body| {
            body["callback_query_id"] == "4"
        })
        .await;
    f.server
        .runtime
        .state
        .grant_approval("default", "delete_message", &json!("-1000000000042"))
        .unwrap();
    drop(lock);
    assert!(
        pending
            .await
            .unwrap()
            .unwrap()
            .get("approval_required")
            .is_none()
    );
    assert_eq!(f.calls("channels.deleteMessages"), 1);
    let sent = f.wire.sent.lock().unwrap();
    assert!(!sent.iter().any(|(method, value)| value["chat_id"] == 303
        || (method == "answerCallbackQuery" && value["callback_query_id"] == "1")));
    assert_eq!(
        sent.iter().filter(|(m, _)| m == "editMessageText").count(),
        2
    );
    assert_eq!(
        f.server
            .runtime
            .state
            .read_json("approval-grants.json", json!({}))
            .unwrap(),
        json!({})
    );
}

#[tokio::test]
async fn owner_always_is_scoped_and_menu_revokes_chat_and_folder_grants() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    let pending = f.execute("delete_message", delete_args());
    let always = button(&f.prompt().await, ":always");
    f.callback(1, 202, &always);
    pending.await.unwrap().unwrap();
    let state = &f.server.runtime.state;
    assert!(state.approval_is_granted("default", "delete_message", &json!("-1000000000042")));
    assert!(!state.approval_is_granted("other", "delete_message", &json!("-1000000000042")));
    f.execute("delete_message", delete_args())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.calls("channels.deleteMessages"), 2);
    state
        .update_json("always-approvals.json", json!({}), |value| {
            value["folders"] = json!(["fixture <folder>"]);
            Ok(())
        })
        .unwrap();
    f.command(2, 101, "/always");
    let list = f
        .wire
        .message("sendMessage", |body| {
            body["text"]
                .as_str()
                .unwrap_or("")
                .starts_with("<b>Permanent chat approvals")
        })
        .await;
    let revoke = list["reply_markup"]["inline_keyboard"][0][0]["callback_data"]
        .as_str()
        .unwrap();
    f.callback(3, 101, revoke);
    f.command(4, 101, "/folders@resolved_helper_bot");
    let folders = f
        .wire
        .message("sendMessage", |body| {
            body["text"]
                .as_str()
                .unwrap_or("")
                .contains("fixture &lt;folder&gt;")
        })
        .await;
    assert_eq!(state.always_approvals()["grants"], json!([]));
    f.callback(
        5,
        101,
        folders["reply_markup"]["inline_keyboard"][0][0]["callback_data"]
            .as_str()
            .unwrap(),
    );
    f.wire
        .message("answerCallbackQuery", |body| {
            body["callback_query_id"] == "5"
        })
        .await;
    assert_eq!(state.always_approvals()["folders"], json!([]));
}

#[tokio::test]
async fn deny_and_timeout_never_dispatch_or_leave_a_pending_request() {
    for deny in [true, false] {
        let f = Fixture::new(false, Duration::from_millis(800)).await;
        let pending = f.execute("delete_message", delete_args());
        let prompt = f.prompt().await;
        if deny {
            f.callback(1, 101, &button(&prompt, ":deny"));
        }
        let expected = if deny { "denied" } else { "timed out" };
        assert!(
            pending
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert_eq!(f.calls("channels.deleteMessages"), 0);
        assert_eq!(
            f.server
                .runtime
                .state
                .read_json("approval-pending.json", json!({}))
                .unwrap(),
            json!({})
        );
        assert_eq!(
            f.wire
                .sent
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "editMessageText")
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn all_deliveries_failing_preserves_terminal_response_and_redacts_token() {
    let f = Fixture::new(true, Duration::from_secs(4)).await;
    let result = f
        .execute("delete_message", delete_args())
        .await
        .unwrap()
        .unwrap();
    let fingerprint = result["fingerprint"].as_str().unwrap();
    assert_eq!(result["approval_required"], true);
    assert_eq!(result["performed"], false);
    assert_eq!(
        result["human_action"],
        format!(
            "Run telegram-mcp approve {fingerprint} in your terminal, then repeat the exact operation."
        )
    );
    assert_eq!(f.calls("channels.deleteMessages"), 0);
    let status = f
        .server
        .execute("safeguard_status", json!({"account":"default"}))
        .await
        .unwrap();
    assert_eq!(
        status["channels"]["telegram_bot"],
        "configured (2 owner accounts)"
    );
    assert_eq!(status["approval_timeout_seconds"], 4);
    assert!(!format!("{result}{status}").contains("fixture-only"));
}

#[tokio::test]
async fn terminal_grant_unblocks_wait_and_bot_identity_is_sealed() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    let pending = f.execute("delete_message", delete_args());
    f.prompt().await;
    for target in [json!(7), json!("@Resolved_Helper_Bot")] {
        let error = f
            .server
            .execute("get_history", json!({"account":"default","chat_id":target}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("touches_approval_channel"));
    }
    let state = &f.server.runtime.state;
    let rows = state.read_json("approval-pending.json", json!({})).unwrap();
    let fingerprint = rows.as_object().unwrap().keys().next().unwrap();
    state
        .update_json("approval-grants.json", json!({}), |grants| {
            grants[fingerprint] = json!({"expires":crate::safeguard::unix_seconds()+60});
            Ok(())
        })
        .unwrap();
    pending.await.unwrap().unwrap();
    assert_eq!(f.calls("channels.deleteMessages"), 1);
}
