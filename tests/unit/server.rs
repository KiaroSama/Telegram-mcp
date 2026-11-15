//! Server boundary tests exercise real validation, policy and domain dispatch.
use super::*;

#[tokio::test]
async fn cancelling_tls_server_scope_aborts_the_owned_signal_waiter() {
    use tokio::io::AsyncReadExt;

    struct Finished(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Finished {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    let (started, running) = tokio::sync::oneshot::channel();
    let (finished, stopped) = tokio::sync::oneshot::channel();
    let material = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        material.cert.pem().into_bytes(),
        material.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = axum_server::Handle::new();
    let serving = tokio::spawn(
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(handle.clone())
            .serve(axum::Router::new().into_make_service()),
    );
    let address = tokio::time::timeout(Duration::from_secs(5), handle.listening())
        .await
        .unwrap()
        .unwrap();
    // This connection remains inside the TLS handshake until cancellation.
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while handle.connection_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let sessions = tokio_util::sync::CancellationToken::new();
    let signal = super::http::ShutdownSignal::new(
        tokio::spawn(async move {
            let _finished = Finished(Some(finished));
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        }),
        handle.clone(),
        sessions.clone(),
    );
    running.await.unwrap();
    drop(signal);
    tokio::time::timeout(Duration::from_secs(5), stopped)
        .await
        .expect("Cancelled TLS server retained its signal task")
        .unwrap();
    assert!(sessions.is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .expect("Cancelled TLS server retained its listener")
        .unwrap()
        .unwrap();
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
        .await
        .expect("Cancelled TLS handshake retained its socket");
    assert!(match closed {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
        ),
        _ => false,
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while handle.connection_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Cancelled TLS server retained a connection task");
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}
use async_trait::async_trait;
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;

struct Probe {
    calls: StdMutex<Vec<(String, Value)>>,
    reply: Value,
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
                serial: Mutex::new(()),
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
fn schema(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn basic_schema() -> serde_json::Map<String, Value> {
    schema(
        json!({"type":"object","properties":{"name":{"type":"string"},"limit":{"type":"integer","default":20}},"required":["name"],"additionalProperties":false}),
    )
}

#[test]
fn all_catalogue_tools_have_native_implementations() {
    let tools = catalogue().unwrap();
    let actual: std::collections::BTreeSet<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(actual.len(), tools.len());
    assert!(actual.contains("send_message"));
    assert!(actual.contains("get_history"));
    assert!(actual.contains("get_poll_results"));
    // Device authorization settings remain cloud API tools; the separate
    // end-to-end encrypted messaging backend is the deferred integration.
    for deferred in [
        "create_secret_chat",
        "send_secret_message",
        "send_secret_media",
        "delete_secret_chat",
    ] {
        assert!(!actual.contains(deferred));
    }
    for tool in tools {
        assert!(
            tool.annotations.is_some(),
            "{} is missing capability annotations",
            tool.name
        );
        assert!(
            tool.input_schema.get("properties").is_some(),
            "{} has no argument schema",
            tool.name
        );
    }
}
#[test]
fn schema_defaults_are_applied_before_dispatch() {
    let mut args = json!({"name":"Test"});
    validate_arguments(&mut args, &basic_schema()).unwrap();
    assert_eq!(args["limit"], 20);
}
#[test]
fn schema_explicit_values_are_not_overwritten_by_defaults() {
    let mut args = json!({"name":"Test","limit":7});
    validate_arguments(&mut args, &basic_schema()).unwrap();
    assert_eq!(args["limit"], 7);
}
#[test]
fn schema_requires_object_arguments() {
    let mut args = json!([]);
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_missing_required_argument() {
    let mut args = json!({});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_unknown_arguments() {
    let mut args = json!({"name":"Test","typo":true});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_booleans_as_integers() {
    let mut args = json!({"name":"Test","limit":true});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_rejects_string_for_strict_integer_field() {
    let mut args = json!({"name":"Test","limit":"20"});
    assert!(validate_arguments(&mut args, &basic_schema()).is_err());
}
#[test]
fn schema_accepts_explicit_optional_null() {
    let s = schema(
        json!({"properties":{"account":{"anyOf":[{"type":"string"},{"type":"null"}]}},"additionalProperties":false}),
    );
    let mut args = json!({"account":null});
    validate_arguments(&mut args, &s).unwrap();
}
#[test]
fn schema_anyof_rejects_nonmatching_type() {
    let s = schema(
        json!({"properties":{"id":{"anyOf":[{"type":"string"},{"type":"integer"}]}},"additionalProperties":false}),
    );
    let mut args = json!({"id":false});
    assert!(validate_arguments(&mut args, &s).is_err());
}
#[test]
fn schema_numeric_bounds_are_enforced() {
    let s = schema(json!({"properties":{"limit":{"type":"integer","minimum":1,"maximum":200}}}));
    for n in [0, 201] {
        let mut args = json!({"limit":n});
        assert!(validate_arguments(&mut args, &s).is_err());
    }
    let mut args = json!({"limit":200});
    validate_arguments(&mut args, &s).unwrap();
}
#[test]
fn schema_array_items_are_checked_recursively() {
    let s = schema(json!({"properties":{"ids":{"type":"array","items":{"type":"integer"}}}}));
    let mut args = json!({"ids":[1,"2"]});
    assert!(validate_arguments(&mut args, &s).is_err());
}
#[test]
fn schema_rejects_oversized_argument_arrays() {
    let s = schema(json!({"properties":{"ids":{"type":"array"}}}));
    let mut args = json!({"ids":vec![0;100001]});
    assert!(validate_arguments(&mut args, &s).is_err());
}
#[test]
fn content_preserves_image_block_payload_and_metadata() {
    let result = content(
        json!({"content":[{"type":"image","data":"AAECAw==","mimeType":"image/png"}],"metadata":{"source":"native"}}),
    );
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["content"][0]["type"], "image");
    assert_eq!(value["content"][0]["data"], "AAECAw==");
    assert_eq!(value["content"][0]["mimeType"], "image/png");
    assert!(
        value["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["text"].as_str().is_some_and(|s| s.contains("native")))
    );
}
#[test]
fn invalid_native_content_is_returned_as_error() {
    let result = content(json!({"content":[{"type":"unknown"}]}));
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["isError"], true);
}
#[test]
fn scalar_content_is_returned_as_text() {
    let result = content(json!({"ok":true,"id":"9223372036854775807"}));
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["content"][0]["type"], "text");
    assert!(
        value["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("9223372036854775807")
    );
}
#[test]
fn redaction_removes_internal_credentials_recursively() {
    let mut value = json!({"access_hash":"a","rows":[{"id":"7","auth_key":"b","file_reference":[1],"nested":{"api_hash":"c","phone_code_hash":"d","text":"Visible text"}}]});
    redact_internal(&mut value);
    assert!(value.get("access_hash").is_none());
    assert!(value["rows"][0].get("auth_key").is_none());
    assert!(value["rows"][0].get("file_reference").is_none());
    assert!(value["rows"][0]["nested"].get("api_hash").is_none());
    assert_eq!(value["rows"][0]["id"], "7");
    assert_eq!(value["rows"][0]["nested"]["text"], "Visible text");
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
#[test]
fn selector_read_only_uses_annotation_values() {
    let tools = super::catalogue::catalogue_for_mode("read-only").unwrap();
    assert!(tools.iter().any(|t| t.name == "get_history"));
    assert!(
        tools
            .iter()
            .all(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
    );
    assert!(!tools.iter().any(|t| t.name == "send_message"));
}
#[test]
fn selector_combined_inclusion_and_exclusion_preserves_read_policy() {
    let tools = super::catalogue::catalogue_for_mode("read-only+send_message-get_me").unwrap();
    assert!(tools.iter().any(|t| t.name == "send_message"));
    assert!(!tools.iter().any(|t| t.name == "get_me"));
    assert!(tools.iter().all(|t| t.name == "send_message"
        || t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true)));
}
#[test]
fn selector_exclusion_wins_over_explicit_inclusion() {
    let tools =
        super::catalogue::catalogue_for_mode("read-only+send_message-send_message").unwrap();
    assert!(!tools.iter().any(|t| t.name == "send_message"));
}
#[test]
fn selector_trims_outer_whitespace_and_normalizes_case() {
    let tools = super::catalogue::catalogue_for_mode("  READ-ONLY+SEND_MESSAGE-GET_ME  ").unwrap();
    assert!(tools.iter().any(|t| t.name == "send_message"));
    assert!(!tools.iter().any(|t| t.name == "get_me"));
}
#[test]
fn selector_all_mode_can_exclude_writes() {
    let tools = super::catalogue::catalogue_for_mode("all-delete_message").unwrap();
    assert!(!tools.iter().any(|t| t.name == "delete_message"));
    assert!(tools.iter().any(|t| t.name == "send_message"));
}
#[test]
fn selector_all_mode_rejects_additive_syntax() {
    assert!(super::catalogue::catalogue_for_mode("all+send_message").is_err());
}
#[test]
fn selector_unknown_names_are_not_silently_ignored() {
    for mode in [
        "read-only+unknown_tool",
        "all-unknown_tool",
        "read-only+send_message-unknown_tool",
    ] {
        assert!(
            super::catalogue::catalogue_for_mode(mode).is_err(),
            "Accepted {mode}"
        );
    }
}
#[test]
fn selector_malformed_or_empty_lists_are_rejected() {
    for mode in [
        "read-only+",
        "all-",
        "read-only+send_message,",
        "read-only+send_message,,get_me",
        "read-only+send message",
        "read-only+send_message-",
        "read-only-get_me+send_message",
        "allover",
        "",
    ] {
        assert!(
            super::catalogue::catalogue_for_mode(mode).is_err(),
            "Accepted {mode}"
        );
    }
}
#[test]
fn selector_duplicate_inclusions_do_not_duplicate_tool_descriptors() {
    let tools =
        super::catalogue::catalogue_for_mode("read-only+send_message,send_message").unwrap();
    assert_eq!(tools.iter().filter(|t| t.name == "send_message").count(), 1);
}
#[test]
fn all_nullable_defaults_match_declared_argument_types() {
    for tool in super::catalogue::catalogue_for_mode("all").unwrap() {
        for (key, spec) in tool.input_schema["properties"].as_object().unwrap() {
            if spec.get("default") == Some(&Value::Null) {
                let s = schema(json!({"properties":{key:spec},"additionalProperties":false}));
                let mut args = json!({key:null});
                assert!(
                    validate_arguments(&mut args, &s).is_ok(),
                    "{} default null contradicts argument type for {key}",
                    tool.name
                );
            }
        }
    }
}

#[path = "server_events.rs"]
mod event_boundaries;

#[path = "server_review.rs"]
mod review;
