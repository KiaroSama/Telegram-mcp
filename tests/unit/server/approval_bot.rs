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
    held_method: StdMutex<Option<String>>,
    release_held: tokio_util::sync::CancellationToken,
    held_owner: StdMutex<Option<i64>>,
    release_owner: tokio_util::sync::CancellationToken,
    ambiguous_owner: StdMutex<Option<i64>>,
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
    let hold = wire
        .held_method
        .lock()
        .unwrap()
        .as_deref()
        .is_some_and(|held| held == method);
    if hold {
        wire.release_held.cancelled().await;
    }
    let held_owner = method == "sendMessage"
        && wire
            .held_owner
            .lock()
            .unwrap()
            .is_some_and(|owner| body["chat_id"].as_i64() == Some(owner));
    if held_owner {
        wire.release_owner.cancelled().await;
    }
    let ambiguous_owner = method == "sendMessage"
        && wire
            .ambiguous_owner
            .lock()
            .unwrap()
            .is_some_and(|owner| body["chat_id"].as_i64() == Some(owner));
    let result = match method.as_str() {
        "getMe" => json!({"id":7,"is_bot":true,"username":"Resolved_Helper_Bot"}),
        "getUpdates" => {
            let _ = tokio::time::timeout(Duration::from_millis(100), wire.updates.notified()).await;
            json!(wire.queue.lock().unwrap().drain(..).collect::<Vec<_>>())
        }
        "sendMessage" if wire.fail_send => {
            return Json(json!({"ok":false,"description":"7:fixture-only"}));
        }
        "sendMessage" if ambiguous_owner => json!({"message_id":0}),
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
                        .starts_with("🔐 <b>Telegram MCP approval")
            })
            .await
    }

    fn callback(&self, id: i64, owner: i64, data: &str) {
        let sent = self.wire.sent.lock().unwrap();
        let message_id = sent
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (method, body))| {
                method == "sendMessage"
                    && body["chat_id"] == owner
                    && body["reply_markup"]["inline_keyboard"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_array)
                        .flatten()
                        .any(|b| b["callback_data"] == data)
            })
            .map_or(9999, |(i, _)| i + 1);
        drop(sent);
        self.callback_from(id, owner, owner, "private", message_id, data);
    }

    fn callback_from(
        &self,
        id: i64,
        owner: i64,
        chat: i64,
        kind: &str,
        message_id: usize,
        data: &str,
    ) {
        self.wire.push(json!({"update_id":id,"callback_query":{"id":id.to_string(),"from":{"id":owner},"data":data,"message":{"message_id":message_id,"chat":{"id":chat,"type":kind}}}}));
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
        self.wire.release_held.cancel();
        self.wire.release_owner.cancel();
        self.http.abort();
    }
}

/// Keep semantic Tokio time stationary while real loopback I/O runs. The owned
/// blocking gate prevents automatic advancement; its independent wall watchdog
/// bounds every await and is released on success or assertion unwinding.
async fn with_manual_clock<T>(body: impl std::future::Future<Output = T>) -> T {
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (expired, mut deadline) = tokio::sync::oneshot::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        let result = blocked.recv_timeout(Duration::from_secs(30));
        if matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
            let _ = expired.send(());
        }
        result
    });
    ready.await.unwrap();
    let result = tokio::select! {
        biased;
        _ = &mut deadline => panic!("Controlled approval fixture exceeded its wall-clock budget"),
        result = body => result,
    };
    drop(release);
    assert!(matches!(
        blocker.await.unwrap(),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    result
}

fn prompt_code(prompt: &Value) -> String {
    button(prompt, ":once")
        .strip_prefix("ap:")
        .unwrap()
        .strip_suffix(":once")
        .unwrap()
        .to_owned()
}

impl Fixture {
    fn hold_owner(&self, owner: i64, ambiguous: bool) {
        *self.wire.held_owner.lock().unwrap() = Some(owner);
        if ambiguous {
            *self.wire.ambiguous_owner.lock().unwrap() = Some(owner);
        }
    }

    fn delivery(&self, code: &str) -> Option<crate::safeguard::bot::PromptDeliverySnapshot> {
        self.server
            .runtime
            .guard
            .bot
            .as_ref()
            .unwrap()
            .prompt_delivery_for_test(code)
    }

    async fn wait_delivery(
        &self,
        code: &str,
        ready: impl Fn(&crate::safeguard::bot::PromptDeliverySnapshot) -> bool,
    ) -> crate::safeguard::bot::PromptDeliverySnapshot {
        let began = std::time::Instant::now();
        loop {
            if let Some(snapshot) = self.delivery(code)
                && ready(&snapshot)
            {
                return snapshot;
            }
            assert!(
                began.elapsed() < Duration::from_secs(30),
                "Expected the actual Bot API response-consumption milestone"
            );
            tokio::task::yield_now().await;
        }
    }

    async fn answer(&self, id: i64, expected: &str) {
        let id = id.to_string();
        let body = self
            .wire
            .message("answerCallbackQuery", |body| {
                body["callback_query_id"] == id
            })
            .await;
        assert_eq!(body["text"], expected);
    }

    fn assert_pending_and_once_cleared(&self, code: &str) {
        assert!(self.delivery(code).is_none());
        for file in ["approval-pending.json", "approval-grants.json"] {
            assert_eq!(
                self.server
                    .runtime
                    .state
                    .read_json(file, json!({}))
                    .unwrap(),
                json!({}),
                "The completed request retained state in {file}"
            );
        }
    }

    fn assert_terminal_edits(&self, owners: &[i64], expected: &str) {
        assert_eq!(self.count("editMessageText", |_| true), owners.len());
        for owner in owners {
            assert_eq!(
                self.count("editMessageText", |body| {
                    body["chat_id"] == *owner
                        && body["text"] == expected
                        && body["reply_markup"]["inline_keyboard"] == json!([])
                }),
                1,
                "Each confirmed owner prompt must have exactly one terminal edit"
            );
        }
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

#[tokio::test]
async fn bypass_menu_is_bound_to_its_owner_chat_and_message() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    let state = &f.server.runtime.state;
    f.command(1, 101, "/bypass");
    let menu = f
        .wire
        .message("sendMessage", |body| {
            body["text"]
                .as_str()
                .is_some_and(|s| s.starts_with("🔒 Bypass mode: <b>OFF</b>"))
        })
        .await;
    let buttons: Vec<_> = menu["reply_markup"]["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row.as_array().unwrap())
        .collect();
    assert_eq!(buttons.len(), 8);
    for (name, key) in [
        ("⏱️ 1 hour", "1h"),
        ("⏱️ 6 hours", "6h"),
        ("📅 1 day", "1d"),
        ("📅 7 days", "7d"),
        ("🗓️ 30 days", "30d"),
        ("✏️ Custom time", "custom"),
        ("♾️ Until I turn it off", "inf"),
        ("🔒 Turn bypass off", "off"),
    ] {
        assert!(
            buttons
                .iter()
                .any(|b| b["text"] == name && b["callback_data"] == format!("mn:bp:{key}"))
        );
    }
    assert!(
        buttons
            .iter()
            .all(|b| (1..=64).contains(&b["callback_data"].as_str().unwrap().len()))
    );
    let message = f
        .wire
        .sent
        .lock()
        .unwrap()
        .iter()
        .position(|(_, body)| body == &menu)
        .unwrap()
        + 1;
    f.callback_from(2, 303, 101, "private", message, "mn:bp:inf");
    f.callback_from(3, 101, 101, "group", message, "mn:bp:inf");
    f.callback_from(5, 101, 101, "private", message, &"é".repeat(33));
    f.command(6, 101, "/status");
    f.wire
        .message("sendMessage", |body| {
            body["text"]
                .as_str()
                .is_some_and(|s| s.contains("Pending requests: "))
        })
        .await;
    assert_eq!(
        state.read_json("bypass.json", json!({"on":false})).unwrap()["on"],
        false
    );
    f.callback_from(7, 101, 101, "private", message, "mn:bp:inf");
    f.wire
        .message("answerCallbackQuery", |body| {
            body["callback_query_id"] == "7"
        })
        .await;
    assert_eq!(
        state.read_json("bypass.json", json!({})).unwrap(),
        json!({"on":true,"until":null,"by":101})
    );
    f.command(8, 101, "/bypass 1d12h");
    f.wire
        .message("sendMessage", |body| {
            body["text"]
                .as_str()
                .is_some_and(|s| s.contains("⏳ For 36 h"))
        })
        .await;
    assert!(state.read_json("bypass.json", json!({})).unwrap()["until"].is_number());
    f.command(9, 101, "/bypass off");
    f.wire
        .message("sendMessage", |body| {
            body["text"] == OFF_MENU && body["reply_markup"]["inline_keyboard"].is_array()
        })
        .await;
    // The first /bypass menu has the same body; wait for the reply to /bypass off.
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.count("sendMessage", |b| b["text"] == OFF_MENU) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        state.read_json("bypass.json", json!({})).unwrap(),
        json!({"on":false})
    );
    f.command(10, 101, "/accounts");
    let accounts = f
        .wire
        .message("sendMessage", |body| {
            body["reply_markup"]["inline_keyboard"][0][0]["callback_data"] == "mn:ac"
        })
        .await;
    assert!(accounts["text"].is_string());
    assert_eq!(accounts["parse_mode"], "HTML");
    assert_eq!(
        accounts["reply_markup"]["inline_keyboard"][0][0]["text"],
        "🔄 Refresh accounts"
    );
}

const OFF_MENU: &str = "🔒 Bypass mode: <b>OFF</b>\n🛡️ Protected actions remain blocked.";

impl Fixture {
    fn count(&self, method: &str, predicate: impl Fn(&Value) -> bool) -> usize {
        let sent = self.wire.sent.lock().unwrap();
        sent.iter()
            .filter(|(m, v)| m == method && predicate(v))
            .count()
    }
}

#[tokio::test]
async fn bypass_buttons_edit_in_place_on_every_click_and_after_restart() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    let state = &f.server.runtime.state;
    // A menu message the running bot never recorded, as after a restart.
    let message = 4242;
    for (id, key) in [(1, "1h"), (2, "inf"), (3, "inf"), (4, "off")] {
        f.callback_from(id, 101, 101, "private", message, &format!("mn:bp:{key}"));
        let tag = id.to_string();
        f.wire
            .message("answerCallbackQuery", |body| {
                body["callback_query_id"] == tag.as_str()
            })
            .await;
    }
    f.wire
        .message("editMessageText", |body| body["text"] == OFF_MENU)
        .await;
    let edits: Vec<Value> = f
        .wire
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == "editMessageText")
        .map(|(_, v)| v.clone())
        .collect();
    assert_eq!(edits.len(), 4, "every click re-renders the menu");
    for edit in &edits {
        assert_eq!(edit["chat_id"], 101);
        assert_eq!(edit["message_id"], message);
        assert_eq!(edit["parse_mode"], "HTML");
        assert_eq!(
            edit["reply_markup"]["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .count(),
            8
        );
    }
    assert_eq!(
        edits[0]["text"],
        "🔓 Bypass mode: <b>ON</b>\n⏳ For 1 h 0 min.\n🛡️ Protected actions remain blocked."
    );
    assert_eq!(
        edits[1]["text"],
        "🔓 Bypass mode: <b>ON</b>\n♾️ Until you turn it off.\n🛡️ Protected actions remain blocked."
    );
    assert_eq!(edits[3]["text"], OFF_MENU);
    assert_eq!(
        f.count("answerCallbackQuery", |b| b["text"] == "🔓 Bypass is on"),
        3
    );
    assert_eq!(
        f.count("answerCallbackQuery", |b| b["text"] == "🔒 Bypass is off"),
        1
    );
    assert_eq!(
        state.read_json("bypass.json", json!({})).unwrap(),
        json!({"on":false})
    );
}

#[tokio::test]
async fn bypass_custom_button_answers_and_sends_instructions() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    f.callback_from(1, 101, 101, "private", 77, "mn:bp:custom");
    f.wire
        .message("answerCallbackQuery", |body| {
            body["callback_query_id"] == "1" && body["text"] == "✏️ Send a custom duration"
        })
        .await;
    f.wire
        .message("sendMessage", |body| {
            body["chat_id"] == 101
                && body["text"]
                    == "✏️ Send /bypass 2h30m or /bypass 90m to set a custom time. Send /bypass off to restore prompts."
        })
        .await;
    assert_eq!(f.count("editMessageText", |_| true), 0);
}

#[tokio::test]
async fn bypass_thirty_day_command_shows_days_and_a_keyboard() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    f.command(1, 101, "/bypass 30d");
    let reply = f
        .wire
        .message("sendMessage", |body| {
            body["text"]
                .as_str()
                .is_some_and(|s| s.contains("<b>ON</b>"))
        })
        .await;
    assert_eq!(reply["parse_mode"], "HTML");
    assert_eq!(
        reply["text"],
        "🔓 Bypass mode: <b>ON</b>\n⏳ For 30 d 0 h 0 min.\n🛡️ Protected actions remain blocked."
    );
    assert!(reply["reply_markup"]["inline_keyboard"].is_array());
}

#[tokio::test]
async fn stale_revocation_callback_is_still_answered() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    f.callback_from(1, 101, 101, "private", 55, "mn:ra:y");
    f.wire
        .message("answerCallbackQuery", |body| {
            body["callback_query_id"] == "1"
                && body["text"] == "This menu is outdated. Send the command again."
        })
        .await;
}

#[tokio::test]
async fn accounts_refresh_edits_in_place_and_stays_clickable() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    for id in [1, 2] {
        f.callback_from(id, 101, 101, "private", 66, "mn:ac");
        let tag = id.to_string();
        f.wire
            .message("answerCallbackQuery", |body| {
                body["callback_query_id"] == tag.as_str()
                    && body["text"] == "🔄 Refreshing account list"
            })
            .await;
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.count("editMessageText", |body| body["message_id"] == 66) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.count("editMessageText", |b| b["message_id"] == 66
            && b["parse_mode"] == "HTML"),
        2
    );
}

#[tokio::test]
async fn owner_callbacks_are_answered_while_menu_delivery_is_stalled() {
    for (data, method, notice, expected_on) in [
        ("mn:bp:inf", "editMessageText", "🔓 Bypass is on", true),
        ("mn:bp:off", "editMessageText", "🔒 Bypass is off", false),
        (
            "mn:ac",
            "editMessageText",
            "🔄 Refreshing account list",
            false,
        ),
        (
            "mn:bp:custom",
            "sendMessage",
            "✏️ Send a custom duration",
            false,
        ),
    ] {
        let f = Fixture::new(false, Duration::from_secs(4)).await;
        if data == "mn:bp:off" {
            crate::safeguard::bypass::BypassState::Indefinite { owner_id: 101 }
                .write(&f.server.runtime.state)
                .unwrap();
        }
        *f.wire.held_method.lock().unwrap() = Some(method.to_owned());
        f.callback_from(1, 101, 101, "private", 77, data);
        f.wire.message(method, |_| true).await;
        f.wire
            .message("answerCallbackQuery", |body| {
                body["callback_query_id"] == "1" && body["text"] == notice
            })
            .await;
        assert!(!f.wire.release_held.is_cancelled());
        assert_eq!(
            f.server
                .runtime
                .state
                .read_json("bypass.json", json!({"on": false}))
                .unwrap()["on"],
            expected_on,
            "The acknowledgement must describe the persisted setting: {data}"
        );
        f.wire.release_held.cancel();
    }
}

#[tokio::test]
async fn failed_bypass_change_answers_without_claiming_success_or_refreshing() {
    let f = Fixture::new(false, Duration::from_secs(4)).await;
    let state = &f.server.runtime.state;
    let prior = crate::safeguard::bypass::BypassState::Indefinite { owner_id: 101 };
    prior.write(state).unwrap();
    std::fs::create_dir(state.root().join("account-rename-transaction.json")).unwrap();
    f.callback_from(1, 101, 101, "private", 77, "mn:bp:off");
    f.wire
        .message("answerCallbackQuery", |body| {
            body["callback_query_id"] == "1"
                && body["text"]
                    == "⚠️ Could not save bypass state. Check /bypass before relying on it."
        })
        .await;
    assert_eq!(
        crate::safeguard::bypass::BypassState::read(state, crate::safeguard::unix_time()),
        prior
    );
    assert_eq!(f.count("editMessageText", |_| true), 0);
    assert_eq!(f.count("sendMessage", |_| true), 0);
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
                    .contains("⏳ Pending requests: 1")
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

#[tokio::test(start_paused = true)]
async fn deny_and_timeout_never_dispatch_or_leave_a_pending_request() {
    with_manual_clock(async {
        for deny in [true, false] {
            let f = Fixture::new(false, Duration::from_millis(800)).await;
            let mut pending = Box::pin(tokio::time::timeout(
                Duration::from_secs(6),
                f.server.execute("delete_message", delete_args()),
            ));
            let prompt = tokio::select! {
                result = &mut pending => panic!("Approval completed before its prompt: {result:?}"),
                prompt = f.prompt() => prompt,
            };
            let code = prompt_code(&prompt);
            let delivered = tokio::select! {
                result = &mut pending => panic!("Approval completed before both deliveries: {result:?}"),
                delivered = f.wait_delivery(&code, |row| row.registered.len() == 2) => delivered,
            };
            assert_eq!(
                delivered.deadline.duration_since(tokio::time::Instant::now()),
                Duration::from_millis(800),
                "Real fixture I/O consumed the controlled approval budget"
            );
            assert!(futures_util::poll!(pending.as_mut()).is_pending());
            if deny {
                f.callback(1, 101, &button(&prompt, ":deny"));
                f.answer(1, "✅ Decision recorded").await;
            } else {
                tokio::time::advance(Duration::from_millis(799)).await;
                // Poll the actual operation, not its JoinHandle: this processes
                // its woken timers before claiming the deadline has not fired.
                assert!(
                    futures_util::poll!(pending.as_mut()).is_pending(),
                    "Approval expired before its exact deadline"
                );
                tokio::time::advance(Duration::from_millis(1)).await;
            }
            let expected = if deny { "denied" } else { "timed out" };
            assert!(pending.await.unwrap().unwrap_err().to_string().contains(expected));
            assert_eq!(f.calls("channels.deleteMessages"), 0);
            f.assert_pending_and_once_cleared(&code);
            let terminal = if deny {
                "❌ Denied. The operation did not run."
            } else {
                "⌛ Timed out. The operation did not run."
            };
            f.assert_terminal_edits(&[101, 202], terminal);
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn delivered_owner_decision_survives_another_owner_delivery_failure() {
    with_manual_clock(async {
        // A successful peer delivery and an ambiguous peer response must both
        // preserve the first verified decision from an already-confirmed owner.
        for ambiguous in [false, true] {
            for deny in [false, true] {
                let f = Fixture::new(false, Duration::from_secs(60)).await;
                f.hold_owner(202, ambiguous);
                let pending = f.execute("delete_message", delete_args());
                let prompt = f.prompt().await;
                let code = prompt_code(&prompt);
                f.wire
                    .message("sendMessage", |body| body["chat_id"] == 202)
                    .await;
                let delivered = f
                    .wait_delivery(&code, |row| row.acknowledged.contains_key(&101))
                    .await;
                assert!(!delivered.acknowledged.contains_key(&202));
                assert!(!f.wire.release_owner.is_cancelled());
                let action = button(&prompt, if deny { ":deny" } else { ":once" });

                // Keep owner, private-chat and message binding checks active
                // when a confirmed owner is eligible during partial delivery.
                let message_id = usize::try_from(delivered.acknowledged[&101]).unwrap();
                f.callback_from(1, 303, 303, "private", message_id, &action);
                f.callback_from(2, 101, 202, "private", message_id, &action);
                f.callback_from(3, 101, 101, "group", message_id, &action);
                // A sent-but-unconfirmed owner cannot borrow that authority.
                f.callback(4, 202, &action);
                f.answer(4, "This request is no longer pending").await;
                for id in ["1", "2", "3"] {
                    assert_eq!(
                        f.count("answerCallbackQuery", |body| body["callback_query_id"]
                            == id),
                        0,
                        "An untrusted owner/chat callback must not be acknowledged"
                    );
                }
                f.callback_from(
                    5,
                    101,
                    101,
                    "private",
                    usize::try_from(delivered.acknowledged[&101] + 1000).unwrap(),
                    &action,
                );
                f.answer(5, "This request is no longer pending").await;
                assert!(f.delivery(&code).unwrap().accepting_reply);
                assert_eq!(f.calls("channels.deleteMessages"), 0);

                assert_eq!(
                    f.server
                        .runtime
                        .state
                        .read_json("approval-grants.json", json!({}))
                        .unwrap(),
                    json!({})
                );
                f.callback(6, 101, &action);
                f.answer(6, "✅ Decision recorded").await;
                assert!(!f.delivery(&code).unwrap().accepting_reply);
                assert!(!pending.is_finished());
                f.callback(7, 101, &action);
                f.answer(7, "This request is no longer pending").await;
                assert_eq!(f.calls("channels.deleteMessages"), 0);

                f.wire.release_owner.cancel();
                let result = pending.await.unwrap();
                let terminal = if deny {
                    assert!(result.unwrap_err().to_string().contains("Approval denied"));
                    "❌ Denied. The operation did not run."
                } else {
                    let result = result.unwrap();
                    assert!(result.get("approval_required").is_none());
                    "✅ Approved. The server will recheck the operation before running it."
                };
                assert_eq!(f.calls("channels.deleteMessages"), usize::from(!deny));
                f.assert_pending_and_once_cleared(&code);
                let owners: &[i64] = if ambiguous { &[101] } else { &[101, 202] };
                f.assert_terminal_edits(owners, terminal);
                f.callback(8, 101, &action);
                f.answer(8, "This request is no longer pending").await;
                assert_eq!(f.calls("channels.deleteMessages"), usize::from(!deny));
                f.assert_pending_and_once_cleared(&code);
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn ambiguous_prompt_delivery_refuses_late_approval_before_returning() {
    with_manual_clock(async {
        let f = Fixture::new(false, Duration::from_secs(60)).await;
        f.hold_owner(202, true);
        *f.wire.held_method.lock().unwrap() = Some("editMessageText".to_owned());
        let mut pending = f.execute("delete_message", delete_args());
        let prompt = f.prompt().await;
        let code = prompt_code(&prompt);
        f.wire.message("sendMessage", |body| body["chat_id"] == 202).await;
        f.wait_delivery(&code, |row| row.acknowledged.contains_key(&101)).await;
        f.wire.release_owner.cancel();
        let terminal = "⚠️ Delivery could not be confirmed. The operation did not run.";
        tokio::select! {
            result = &mut pending => panic!("Ambiguous delivery returned without clearing its confirmed prompt: {result:?}"),
            _ = f.wire.message("editMessageText", |body| body["chat_id"] == 101 && body["text"] == terminal) => {},
        }
        // The request still exists while its terminal edit is held, so rejection
        // here proves finalization, rather than relying on Request::drop later.
        assert!(!f.delivery(&code).unwrap().accepting_reply);
        let once = button(&prompt, ":once");
        f.callback(1, 101, &once);
        f.answer(1, "This request is no longer pending").await;
        assert_eq!(f.calls("channels.deleteMessages"), 0);
        assert_eq!(
            f.server.runtime.state.read_json("approval-grants.json", json!({})).unwrap(),
            json!({})
        );
        assert!(!pending.is_finished());
        f.wire.release_held.cancel();
        assert!(pending.await.unwrap().unwrap_err().to_string().contains("Approval delivery was ambiguous"));
        f.assert_pending_and_once_cleared(&code);
        f.assert_terminal_edits(&[101], terminal);
        assert_eq!(f.calls("channels.deleteMessages"), 0);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn cancelling_partial_prompt_delivery_clears_requests_and_one_shot_grants() {
    with_manual_clock(async {
        for approve_first in [false, true] {
            let f = Fixture::new(false, Duration::from_secs(60)).await;
            f.hold_owner(202, false);
            let pending = f.execute("delete_message", delete_args());
            let prompt = f.prompt().await;
            let code = prompt_code(&prompt);
            f.wire
                .message("sendMessage", |body| body["chat_id"] == 202)
                .await;
            f.wait_delivery(&code, |row| row.acknowledged.contains_key(&101))
                .await;
            let once = button(&prompt, ":once");
            if approve_first {
                f.callback(1, 101, &once);
                f.answer(1, "✅ Decision recorded").await;
                assert_eq!(
                    f.server
                        .runtime
                        .state
                        .read_json("approval-grants.json", json!({}))
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .len(),
                    1,
                    "This branch must cancel a real recorded one-shot grant"
                );
            }
            assert!(!pending.is_finished());
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            f.assert_pending_and_once_cleared(&code);
            f.wire.release_owner.cancel();
            f.callback(2, 101, &once);
            f.answer(2, "This request is no longer pending").await;
            f.assert_pending_and_once_cleared(&code);
            assert_eq!(f.calls("channels.deleteMessages"), 0);
            f.assert_terminal_edits(&[], "");
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn restricted_prompt_rejects_forged_always_before_all_deliveries_finish() {
    with_manual_clock(async {
        let f = Fixture::new(false, Duration::from_secs(60)).await;
        f.hold_owner(202, false);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("unapproved.bin");
        assert!(!target.starts_with(f.server.runtime.state.installation_root()));
        let pending = f.execute(
            "download_media",
            json!({
                "account":"default", "chat_id":42, "message_id":20, "file_path":target,
            }),
        );
        let prompt = f.prompt().await;
        let code = prompt_code(&prompt);
        assert!(!prompt["reply_markup"].to_string().contains(":always"));
        f.wire
            .message("sendMessage", |body| body["chat_id"] == 202)
            .await;
        let delivered = f
            .wait_delivery(&code, |row| row.acknowledged.contains_key(&101))
            .await;
        // Forge only the action, retaining the real confirmed owner/message binding.
        f.callback_from(
            1,
            101,
            101,
            "private",
            usize::try_from(delivered.acknowledged[&101]).unwrap(),
            &format!("ap:{code}:always"),
        );
        f.answer(1, "This request is no longer pending").await;
        assert!(f.delivery(&code).unwrap().accepting_reply);
        assert_eq!(
            f.server.runtime.state.always_approvals()["grants"],
            json!([])
        );
        assert_eq!(
            f.server
                .runtime
                .state
                .read_json("approval-grants.json", json!({}))
                .unwrap(),
            json!({})
        );
        f.callback(2, 101, &button(&prompt, ":deny"));
        f.answer(2, "✅ Decision recorded").await;
        assert!(!f.delivery(&code).unwrap().allow_always);
        f.wire.release_owner.cancel();
        assert!(
            pending
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("Approval denied")
        );
        f.assert_pending_and_once_cleared(&code);
        f.assert_terminal_edits(&[101, 202], "❌ Denied. The operation did not run.");
        assert!(!target.exists());
        assert!(f.api.calls.lock().unwrap().is_empty());
        assert_eq!(
            f.server.runtime.state.always_approvals()["grants"],
            json!([])
        );
        assert_eq!(
            f.server
                .runtime
                .state
                .read_json("folder-grants.json", json!({"folders":[]}))
                .unwrap(),
            json!({"folders":[]})
        );
    })
    .await;
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
