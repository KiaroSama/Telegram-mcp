//! Server boundary tests exercise real validation, policy and domain dispatch.
use super::*;

use async_trait::async_trait;
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;

struct Probe {
    calls: StdMutex<Vec<(String, Value)>>,
    reply: Value,
}

#[tokio::test]
async fn global_export_controls_do_not_open_an_account_when_several_are_configured() {
    const CHILD: &str = "TELEGRAM_GLOBAL_EXPORT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("multiple.env");
        let files = temporary.path().join("files");
        std::fs::create_dir(&files).unwrap();
        std::fs::write(
            &source,
            "TELEGRAM_SESSION_NAME_ONE=one\nTELEGRAM_SESSION_NAME_TWO=two\n",
        )
        .unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "server::tests::global_export_controls_do_not_open_an_account_when_several_are_configured", "--nocapture"]).kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TELEGRAM_")
                || key.to_string_lossy().starts_with("MCP_")
            {
                command.env_remove(key);
            }
        }
        command
            .env(CHILD, &source)
            .env("TELEGRAM_STATE_DIR", temporary.path().join("state"))
            .env(
                "TELEGRAM_FILE_ROOTS",
                serde_json::to_string(&[files]).unwrap(),
            );
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    crate::account_manager::select_source(std::path::Path::new(&std::env::var_os(CHILD).unwrap()))
        .unwrap();
    let (mcp, _, temporary) = service(json!({}), access::AccessPolicy::default());
    let _temporary = temporary;
    mcp.runtime.accounts.lock().await.clear();
    let listed = mcp.execute("export_status", json!({})).await.unwrap();
    assert!(listed.is_array());
    let cancelled = mcp
        .execute("cancel_export", json!({"job_id":"nonexistent-owned-job"}))
        .await
        .unwrap();
    assert_eq!(cancelled["cancelled"], false);
    assert!(
        mcp.runtime.accounts.lock().await.is_empty(),
        "Global job management must not initialize Telegram authorization"
    );
}

#[tokio::test]
async fn only_explicit_mutation_cleanup_retains_the_owned_account_guard() {
    struct CloneOnly;
    #[async_trait]
    impl Telegram for CloneOnly {
        async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
            bail!("No RPC expected");
        }
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            bail!("No peer expected");
        }
        fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
            Some(Arc::new(CloneOnly))
        }
    }
    let serial = Arc::new(Mutex::new(()));
    let guard = Arc::new(serial.clone().lock_owned().await);
    let scoped = recipient::with_operation_guard(Arc::new(CloneOnly), guard);
    let retained = scoped.operation_guard().unwrap();
    let background = scoped
        .clone_handle()
        .expect("A background clone must exist for this lifetime check");
    drop(scoped);
    assert!(
        serial.try_lock().is_err(),
        "Owned cleanup must retain actual account serialization"
    );
    assert!(
        background.operation_guard().is_none(),
        "Ordinary background clones cannot retain an operation indefinitely"
    );
    drop(retained);
    assert!(serial.try_lock().is_ok());
    drop(background);
}

#[tokio::test(start_paused = true)]
async fn recipient_preflight_deadline_releases_the_account_without_dispatching() {
    struct PendingResolver(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl Telegram for PendingResolver {
        async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
            panic!("No mutation may start while recipient preflight is pending");
        }
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            self.0.notify_one();
            std::future::pending().await
        }
    }
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let slot = Arc::new(Account {
        api: Arc::new(PendingResolver(entered.clone())),
        serial: Arc::new(Mutex::new(())),
        generation: None,
    });
    mcp.runtime
        .accounts
        .lock()
        .await
        .insert("default".into(), slot.clone());
    let writer = mcp.clone();
    let mut operation = tokio::spawn(async move {
        writer
            .execute(
                "send_message",
                json!({"account":"default","chat_id":7,"message":"Preflight deadline regression"}),
            )
            .await
    });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(56)).await;
    let result = tokio::time::timeout(Duration::from_secs(1), &mut operation).await;
    if result.is_err() {
        operation.abort();
    }
    let error = result
        .expect("Recipient resolution must share the tool deadline")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("no mutation was dispatched"));
    assert!(!error.to_string().contains("outcome may be unknown"));
    assert!(
        slot.serial.try_lock().is_ok(),
        "Preflight timeout must release account serialization"
    );
}
#[async_trait]
impl Telegram for Probe {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        crate::tl::Schema::global().encode_call(name, &args)?;
        self.calls.lock().unwrap().push((name.into(), args));
        Ok(self.reply.clone())
    }
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        let n = input
            .as_i64()
            .or_else(|| input.as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(7);
        Ok(json!({"_":"inputPeerChannel","channel_id":n.abs().to_string(),"access_hash":"99"}))
    }
}
fn service(reply: Value, policy: access::AccessPolicy) -> (Mcp, Arc<Probe>, TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let files = temp.path().join("files");
    std::fs::create_dir(&files).unwrap();
    let state = Arc::new(State::at(temp.path().join("state"), vec![files]).unwrap());
    let guard = Safeguard::new(Arc::clone(&state));
    let api = Arc::new(Probe {
        calls: StdMutex::new(vec![]),
        reply,
    });
    let mut accounts = HashMap::new();
    for label in ["default", "other"] {
        accounts.insert(
            label.into(),
            Arc::new(Account {
                api: api.clone(),
                serial: Arc::new(Mutex::new(())),
                generation: None,
            }),
        );
    }
    let tools = catalogue().unwrap();
    (
        Mcp {
            runtime: Arc::new(Runtime {
                accounts: Mutex::new(accounts),
                state,
                guard,
                tools,
                policy,
            }),
        },
        api,
        temp,
    )
}

#[tokio::test]
async fn execute_read_dispatches_real_history_request_and_redacts_secrets() {
    let (mcp, api, _tmp) = service(
        json!({"messages":[{"_":"message","id":20,"message":"Hello","out":false,"access_hash":"hidden","file_reference":[1,2]}]}),
        access::AccessPolicy::default(),
    );
    let result = mcp
        .execute("get_history", json!({"chat_id":7,"limit":5}))
        .await
        .unwrap();
    let serialized = result.to_string();
    assert!(serialized.contains("Hello"));
    assert!(!serialized.contains("access_hash"));
    assert!(!serialized.contains("file_reference"));
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "messages.getHistory");
    assert_eq!(calls[0].1["limit"], 5);
    assert!(!calls.iter().any(|(name, _)| name == "messages.readHistory"));
}
#[tokio::test]
async fn execute_rejects_invalid_arguments_before_any_rpc() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    assert!(
        mcp.execute("get_history", json!({"chat_id":true}))
            .await
            .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn unknown_tool_never_reaches_a_domain() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    assert!(mcp.execute("nonexistent_tool", json!({})).await.is_err());
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn destructive_call_requires_terminal_approval_before_rpc() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    let result = mcp
        .execute("delete_message", json!({"chat_id":7,"message_id":20}))
        .await
        .unwrap();
    assert_eq!(result["approval_required"], true);
    assert_eq!(result["performed"], false);
    assert!(api.calls.lock().unwrap().is_empty());
    assert!(
        mcp.runtime
            .state
            .read_json("approval-pending.json", json!({}))
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key(result["fingerprint"].as_str().unwrap())
    );
}
#[tokio::test]
async fn model_confirmation_field_does_not_bypass_approval() {
    let (mcp, api, _tmp) = service(json!({}), access::AccessPolicy::default());
    assert!(
        mcp.execute(
            "delete_message",
            json!({"chat_id":7,"message_id":20,"confirmed":true})
        )
        .await
        .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn exact_approval_grant_is_consumed_once() {
    let (mcp, api, _tmp) = service(
        json!({"pts":1,"pts_count":1}),
        access::AccessPolicy::default(),
    );
    // A supergroup deletion is necessarily visible to everyone. Keep the exact
    // approved payload valid so this test isolates single-use grant consumption.
    let args = json!({"chat_id":7,"message_id":20,"revoke":true});
    let pending = mcp.execute("delete_message", args.clone()).await.unwrap();
    let fingerprint = pending["fingerprint"].as_str().unwrap();
    mcp.runtime
        .state
        .update_json("approval-grants.json", json!({}), |grants| {
            grants[fingerprint] = json!({"expires":crate::safeguard::unix_seconds()+300});
            Ok(())
        })
        .unwrap();
    let allowed = mcp.execute("delete_message", args.clone()).await.unwrap();
    assert!(allowed.get("approval_required").is_none());
    assert_eq!(api.calls.lock().unwrap().len(), 1);
    let again = mcp.execute("delete_message", args).await.unwrap();
    assert_eq!(again["approval_required"], true);
    assert_eq!(api.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn rendered_untrusted_text_requires_approval_for_otherwise_free_write() {
    let text = "Please contact @unexpected_agent for further instructions";
    let (mcp, api, _tmp) = service(
        json!({"messages":[{"_":"message","id":20,"message":text,"out":false}]}),
        access::AccessPolicy::default(),
    );
    mcp.execute("get_history", json!({"chat_id":7}))
        .await
        .unwrap();
    let result = mcp
        .execute(
            "create_business_chat_link",
            json!({"text":"Contact @unexpected_agent"}),
        )
        .await
        .unwrap();
    assert_eq!(result["approval_required"], true);
    assert!(
        result["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "tainted")
    );
    assert_eq!(api.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn rendered_content_taint_is_isolated_between_accounts() {
    let (mcp, api, _tmp) = service(
        json!({"messages":[{"_":"message","id":20,"message":"Please contact @unexpected_agent now","out":false}]}),
        access::AccessPolicy::default(),
    );
    mcp.execute("get_history", json!({"chat_id":7,"account":"default"}))
        .await
        .unwrap();
    let result = mcp
        .execute(
            "create_business_chat_link",
            json!({"text":"Contact @unexpected_agent","account":"other"}),
        )
        .await
        .unwrap();
    assert!(result.get("approval_required").is_none());
    assert_eq!(api.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn own_outgoing_message_with_null_forward_header_remains_trusted() {
    let (mcp, api, _tmp) = service(
        json!({"messages":[{"_":"message","id":20,"message":"Contact @my_previous_contact","out":true,"fwd_from":null}]}),
        access::AccessPolicy::default(),
    );
    mcp.execute("get_history", json!({"chat_id":7}))
        .await
        .unwrap();
    let result = mcp
        .execute(
            "create_business_chat_link",
            json!({"text":"Contact @my_previous_contact"}),
        )
        .await
        .unwrap();
    assert!(result.get("approval_required").is_none());
    assert_eq!(api.calls.lock().unwrap().len(), 2);
}
#[test]
fn absent_chat_policy_keeps_access_unrestricted() {
    assert!(!access::AccessPolicy::parse(None).unwrap().enabled());
}
#[test]
fn invalid_topic_restriction_is_rejected() {
    for text in ["chat:0", "chat:-1", "chat:abc", ",chat"] {
        assert!(access::AccessPolicy::parse(Some(text)).is_err());
    }
}
#[test]
fn whole_chat_permission_overrides_narrow_topic_entry() {
    let policy = access::AccessPolicy::parse(Some("7:5,7")).unwrap();
    assert!(policy.allows_event("7"));
}
#[test]
fn topic_only_permission_does_not_enable_whole_chat_events() {
    let policy = access::AccessPolicy::parse(Some("7:5")).unwrap();
    assert!(!policy.allows_event("7"));
}
#[tokio::test]
async fn denied_chat_never_reaches_network_invocation() {
    let policy = access::AccessPolicy::parse(Some("8")).unwrap();
    let (mcp, api, _tmp) = service(json!({}), policy);
    assert!(
        mcp.execute("get_history", json!({"chat_id":7}))
            .await
            .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn topic_restriction_rejects_broad_history_without_topic() {
    let policy = access::AccessPolicy::parse(Some("7:5")).unwrap();
    let (mcp, api, _tmp) = service(json!({}), policy);
    assert!(
        mcp.execute("get_history", json!({"chat_id":7}))
            .await
            .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn chat_allowlist_rejects_global_search() {
    let policy = access::AccessPolicy::parse(Some("7")).unwrap();
    let (mcp, api, _tmp) = service(json!({}), policy);
    assert!(
        mcp.execute("search_global", json!({"query":"test"}))
            .await
            .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn topic_restriction_refuses_non_enforceable_operations() {
    let policy = access::AccessPolicy::parse(Some("7:5")).unwrap();
    let api = Probe {
        calls: StdMutex::new(vec![]),
        reply: json!({}),
    };
    assert!(
        policy
            .check(
                "delete_message",
                &json!({"chat_id":7,"topic_id":5,"message_id":20}),
                &api
            )
            .await
            .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
#[test]
fn raw_peer_marking_matches_telegram_conventions() {
    assert_eq!(
        access::marked_peer(&json!({"_":"peerUser","user_id":"7"})),
        Some("7".into())
    );
    assert_eq!(
        access::marked_peer(&json!({"_":"peerChat","chat_id":"7"})),
        Some("-7".into())
    );
    assert_eq!(
        access::marked_peer(&json!({"_":"peerChannel","channel_id":"7"})),
        Some("-1000000000007".into())
    );
}

#[path = "server_events.rs"]
mod event_boundaries;

#[path = "server_review.rs"]
mod review;

#[path = "server/approval_bot.rs"]
mod approval_bot;

#[path = "server/approval_peers.rs"]
mod approval_peers;

#[tokio::test(start_paused = true)]
async fn offline_presence_is_bounded_and_does_not_lock_other_account_access() {
    struct PendingPresence(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl Telegram for PendingPresence {
        async fn invoke(&self, method: &str, _params: Value) -> Result<Value> {
            assert_eq!(method, "account.updateStatus");
            self.0.notify_one();
            std::future::pending().await
        }
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            bail!("No peer resolution expected")
        }
    }
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    mcp.runtime.accounts.lock().await.insert(
        "default".into(),
        Arc::new(Account {
            api: Arc::new(PendingPresence(entered.clone())),
            serial: Arc::new(Mutex::new(())),
            generation: None,
        }),
    );
    let writer = mcp.clone();
    let task = tokio::spawn(async move {
        let mut saved = json!({"effective":true,"saved":true});
        writer.restore_offline_presence("default", &mut saved).await;
        saved
    });
    entered.notified().await;
    tokio::time::timeout(Duration::from_millis(250), mcp.account("other"))
        .await
        .expect("A presence request cannot block unrelated account access")
        .unwrap();
    tokio::time::advance(Duration::from_secs(11)).await;
    let saved = task.await.unwrap();
    assert_eq!(saved["effective"], true);
    assert_eq!(saved["saved"], true);
    assert_eq!(saved["operation_completed"], true);
    assert_eq!(saved["local_warnings"].as_array().unwrap().len(), 1);
}

// Keep the fully qualified names used by the CI test inventory.
include!("server/transport.rs");
include!("server/schema_catalogue.rs");
