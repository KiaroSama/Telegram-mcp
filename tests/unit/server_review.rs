use super::*;

#[tokio::test]
async fn concurrent_sends_check_bulk_approval_after_taking_account_lock() {
    let (mcp, api, _tmp) = service(
        json!({"_":"updates","updates":[],"users":[],"chats":[],"date":1,"seq":1}),
        access::AccessPolicy::default(),
    );
    mcp.runtime.state.write_json("sent-recipients.json", &json!({"default":["1","2","3","4","5","6","-1000000000001","-1000000000002","-1000000000003","-1000000000004","-1000000000005","-1000000000006"]})).unwrap();
    let slot = mcp.runtime.accounts.lock().await["default"].clone();
    let lock = slot.serial.lock().await;
    let mut requests = vec![];
    for peer in 1..=6 {
        let service = mcp.clone();
        requests.push(tokio::spawn(async move {
            service.execute("send_message", json!({"account":"default","chat_id":peer,"message":"Owned concurrency regression"})).await.unwrap()
        }));
    }
    // All six tasks queue at the actual account serialization boundary. Under
    // the old ordering they have already passed the state-dependent check.
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&slot) < 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all six calls must queue on the owned account");
    drop(lock);
    let mut approvals = 0;
    for request in requests {
        let result = request.await.unwrap();
        if result["approval_required"] == true {
            assert!(
                result["reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("bulk_send"))
            );
            approvals += 1;
        }
    }
    assert_eq!(approvals, 1);
    assert_eq!(api.calls.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn completed_send_survives_recipient_bookkeeping_failure() {
    struct DiskFailure {
        state: Arc<State>,
        sent: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl Telegram for DiskFailure {
        async fn resolve_peer(&self, _: &Value) -> Result<Value> {
            Ok(json!({"_":"inputPeerUser","user_id":"7","access_hash":"99"}))
        }
        async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
            crate::tl::Schema::global().encode_call(name, &args)?;
            assert_eq!(name, "messages.sendMessage");
            self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.state.write_json(
                "sent-recipients.json",
                &json!({"default":"broken after successful send"}),
            )?;
            Ok(
                json!({"_":"updateShortSentMessage","id":77,"pts":1,"pts_count":1,"date":1,"out":true}),
            )
        }
    }
    let (mcp, _, _tmp) = service(json!({}), access::AccessPolicy::default());
    mcp.runtime
        .state
        .write_json("sent-recipients.json", &json!({"default":["7"]}))
        .unwrap();
    let api = Arc::new(DiskFailure {
        state: mcp.runtime.state.clone(),
        sent: std::sync::atomic::AtomicUsize::new(0),
    });
    mcp.runtime.accounts.lock().await.insert(
        "default".into(),
        Arc::new(Account {
            api: api.clone(),
            serial: Mutex::new(()),
            generation: None,
        }),
    );
    let result = mcp
        .execute(
            "send_message",
            json!({"account":"default","chat_id":7,"message":"Owned outcome regression"}),
        )
        .await
        .unwrap();
    assert_eq!(api.sent.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(result["operation_completed"], true);
    assert!(
        result["local_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("Do not repeat"))
    );
    assert!(result.to_string().contains("77"));
}

struct AliasProbe {
    target: std::sync::atomic::AtomicI64,
    resolve_count: std::sync::atomic::AtomicUsize,
    flip_after_resolve: bool,
    sent: StdMutex<Vec<Value>>,
}
#[async_trait]
impl Telegram for AliasProbe {
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.resolve_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let id = input
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| input.as_i64())
            .unwrap_or_else(|| self.target.load(std::sync::atomic::Ordering::SeqCst));
        if self.flip_after_resolve {
            self.target.store(8, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(json!({"_":"inputPeerUser","user_id":id.to_string(),"access_hash":(id*10).to_string()}))
    }
    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        crate::tl::Schema::global().encode_call(name, &args)?;
        assert_eq!(name, "messages.sendMessage");
        self.sent.lock().unwrap().push(args["peer"].clone());
        Ok(json!({"_":"updateShortSentMessage","id":77,"pts":1,"pts_count":1,"date":1,"out":true}))
    }
}
async fn alias_service(
    policy: access::AccessPolicy,
    flip: bool,
) -> (Mcp, Arc<AliasProbe>, TempDir) {
    let (mcp, _, temp) = service(json!({}), policy);
    let api = Arc::new(AliasProbe {
        target: std::sync::atomic::AtomicI64::new(7),
        resolve_count: std::sync::atomic::AtomicUsize::new(0),
        flip_after_resolve: flip,
        sent: StdMutex::new(vec![]),
    });
    mcp.runtime.accounts.lock().await.insert(
        "default".into(),
        Arc::new(Account {
            api: api.clone(),
            serial: Mutex::new(()),
            generation: None,
        }),
    );
    (mcp, api, temp)
}
#[tokio::test]
async fn alias_replacement_does_not_inherit_approved_recipient_history() {
    let (mcp, api, _temp) = alias_service(access::AccessPolicy::default(), false).await;
    mcp.runtime
        .state
        .write_json(
            "sent-recipients.json",
            &json!({"default":["finance team","7"]}),
        )
        .unwrap();
    let args =
        json!({"account":"default","chat_id":"finance team","message":"Owned identity regression"});
    assert!(
        mcp.execute("send_message", args.clone())
            .await
            .unwrap()
            .get("approval_required")
            .is_none()
    );
    api.target.store(8, std::sync::atomic::Ordering::SeqCst);
    let changed = mcp.execute("send_message", args).await.unwrap();
    assert_eq!(changed["approval_required"], true);
    assert_eq!(api.sent.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn allowlist_and_send_use_the_same_resolved_recipient_reference() {
    let (mcp, api, _temp) =
        alias_service(access::AccessPolicy::parse(Some("7")).unwrap(), true).await;
    mcp.runtime
        .state
        .write_json(
            "sent-recipients.json",
            &json!({"default":["finance team","7"]}),
        )
        .unwrap();
    let result=mcp.execute("send_message",json!({"account":"default","chat_id":"finance team","message":"Owned binding regression"})).await.unwrap();
    assert!(result.get("approval_required").is_none());
    let sent = api.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["user_id"], "7");
    assert_eq!(sent[0]["access_hash"], "70");
    assert_eq!(
        api.resolve_count.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}
#[tokio::test]
async fn exact_approval_for_an_alias_cannot_be_replayed_after_replacement() {
    let (mcp, api, _temp) = alias_service(access::AccessPolicy::default(), false).await;
    let args =
        json!({"account":"default","chat_id":"finance team","message":"Owned grant regression"});
    let pending = mcp.execute("send_message", args.clone()).await.unwrap();
    let fingerprint = pending["fingerprint"].as_str().unwrap();
    mcp.runtime
        .state
        .update_json("approval-grants.json", json!({}), |grants| {
            grants[fingerprint] = json!({"expires":crate::safeguard::unix_seconds()+300});
            Ok(())
        })
        .unwrap();
    api.target.store(8, std::sync::atomic::Ordering::SeqCst);
    let changed = mcp.execute("send_message", args).await.unwrap();
    assert_eq!(changed["approval_required"], true);
    assert_ne!(changed["fingerprint"], pending["fingerprint"]);
    assert!(api.sent.lock().unwrap().is_empty());
}

#[derive(Clone)]
struct RoutedProbe {
    calls: Arc<StdMutex<Vec<(String, Value)>>>,
    target: Arc<std::sync::atomic::AtomicI64>,
    fence: Option<Arc<State>>,
}
#[async_trait]
impl Telegram for RoutedProbe {
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        if input == "me" {
            return Ok(json!({"_":"inputPeerSelf"}));
        }
        if input.is_object() {
            return Ok(input.clone());
        }
        let id = input
            .as_i64()
            .or_else(|| input.as_str().and_then(|s| s.parse().ok()))
            .unwrap_or_else(|| self.target.load(std::sync::atomic::Ordering::SeqCst));
        Ok(if id < 0 {
            json!({"_":"inputPeerChat","chat_id":(-id).to_string()})
        } else {
            json!({"_":"inputPeerUser","user_id":id.to_string(),"access_hash":"70"})
        })
    }
    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        crate::tl::Schema::global().encode_call(name, &args)?;
        self.calls.lock().unwrap().push((name.into(), args));
        if name == "users.getUsers" {
            return Ok(json!([{"_":"user","id":"7","self":true}]));
        }
        if let Some(state) = &self.fence {
            std::fs::write(state.root().join("account-rename-transaction.json"), b"{}")?;
        }
        Ok(json!({"_":"updateShortSentMessage","id":77,"pts":1,"pts_count":1,"date":1,"out":true}))
    }
    fn clone_handle(&self) -> Option<Arc<dyn Telegram>> {
        Some(Arc::new(self.clone()))
    }
    fn cache_key(&self) -> Option<String> {
        Some("owned-recipient-route".into())
    }
    fn alias_matches(&self, query: &str) -> Result<Value> {
        Ok(json!([{ "alias":query, "peer":"7" }]))
    }
}
async fn routed_service(
    policy: access::AccessPolicy,
    fence: bool,
) -> (Mcp, Arc<RoutedProbe>, TempDir) {
    let (mcp, _, temp) = service(json!({}), policy);
    let api = Arc::new(RoutedProbe {
        calls: Default::default(),
        target: Arc::new(std::sync::atomic::AtomicI64::new(7)),
        fence: fence.then(|| mcp.runtime.state.clone()),
    });
    mcp.runtime.accounts.lock().await.insert(
        "default".into(),
        Arc::new(Account {
            api: api.clone(),
            serial: Mutex::new(()),
            generation: None,
        }),
    );
    (mcp, api, temp)
}

#[tokio::test]
async fn forward_checks_destination_independently_of_source_with_the_same_bare_id() {
    let (mcp, api, _temp) =
        routed_service(access::AccessPolicy::parse(Some("7,-7")).unwrap(), false).await;
    mcp.runtime
        .state
        .write_json("sent-recipients.json", &json!({"default":["7"]}))
        .unwrap();
    let args = json!({"account":"default","from_chat_id":7,"to_chat_id":-7,"message_ids":[11]});
    let pending = mcp.execute("forward_messages", args.clone()).await.unwrap();
    assert_eq!(pending["approval_required"], true);
    assert!(api.calls.lock().unwrap().is_empty());
    let fingerprint = pending["fingerprint"].as_str().unwrap();
    mcp.runtime
        .state
        .update_json("approval-grants.json", json!({}), |grants| {
            grants[fingerprint] = json!({"expires":crate::safeguard::unix_seconds()+300});
            Ok(())
        })
        .unwrap();
    mcp.execute("forward_messages", args).await.unwrap();
    {
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "messages.forwardMessages");
        assert_eq!(calls[0].1["from_peer"]["_"], "inputPeerUser");
        assert_eq!(calls[0].1["from_peer"]["user_id"], "7");
        assert_eq!(calls[0].1["to_peer"]["_"], "inputPeerChat");
        assert_eq!(calls[0].1["to_peer"]["chat_id"], "7");
    }
    assert_eq!(
        mcp.runtime
            .state
            .read_json("sent-recipients.json", Value::Null)
            .unwrap()["default"],
        json!(["7", "-7"])
    );
    let (denied, api, _temp2) =
        routed_service(access::AccessPolicy::parse(Some("7")).unwrap(), false).await;
    assert!(
        denied
            .execute(
                "forward_messages",
                json!({"account":"default","from_chat_id":7,"to_chat_id":-7,"message_ids":[11]})
            )
            .await
            .is_err()
    );
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn authenticated_self_works_with_numeric_and_literal_allowlists() {
    for scope in ["7", "me"] {
        let (mcp, api, _temp) =
            routed_service(access::AccessPolicy::parse(Some(scope)).unwrap(), false).await;
        mcp.runtime
            .state
            .write_json("sent-recipients.json", &json!({"default":["7"]}))
            .unwrap();
        let result = mcp
            .execute(
                "send_message",
                json!({"account":"default","chat_id":"me","message":"Self identity regression"}),
            )
            .await
            .unwrap();
        assert!(result.get("approval_required").is_none());
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "users.getUsers");
        assert_eq!(calls[1].1["peer"]["_"], "inputPeerSelf");
    }
}

#[tokio::test]
async fn owned_backend_clones_keep_private_recipient_and_self_pins() {
    let (_mcp, api, _temp) = routed_service(access::AccessPolicy::default(), false).await;
    let (bound, approval) = recipient::bind(
        api.clone(),
        &json!({"to_chat_id":"finance team","from_chat_id":"me"}),
    )
    .await
    .unwrap();
    assert_eq!(approval["to_chat_id"], "7");
    assert_eq!(approval["from_chat_id"], "7");
    api.target.store(8, std::sync::atomic::Ordering::SeqCst);
    let cloned = bound.clone_handle().unwrap();
    assert_eq!(
        cloned.resolve_peer(&json!("finance team")).await.unwrap()["user_id"],
        "7"
    );
    assert_eq!(
        cloned
            .pinned_identity(&json!({"_":"inputPeerSelf"}))
            .as_deref(),
        Some("7")
    );
    assert_eq!(cloned.cache_key().as_deref(), Some("owned-recipient-route"));
    assert_eq!(
        cloned.alias_matches("finance team").unwrap()[0]["alias"],
        "finance team"
    );
    assert_eq!(
        cloned.pinned_identity(&json!({"_":"inputPeerUser","user_id":"8","access_hash":"70"})),
        None
    );
}

#[tokio::test]
async fn completed_sends_still_count_toward_bulk_when_local_writes_are_fenced() {
    let (mcp, api, _temp) = routed_service(access::AccessPolicy::default(), true).await;
    mcp.runtime
        .state
        .write_json(
            "sent-recipients.json",
            &json!({"default":["1","2","3","4","5","6"]}),
        )
        .unwrap();
    for peer in 1..=5 {
        let result = mcp
            .execute(
                "send_message",
                json!({"account":"default","chat_id":peer,"message":"Accepted remote send"}),
            )
            .await
            .unwrap();
        assert_eq!(result["operation_completed"], true);
    }
    let sixth = mcp
        .execute(
            "send_message",
            json!({"account":"default","chat_id":6,"message":"Must require bulk approval"}),
        )
        .await;
    assert_eq!(
        api.calls.lock().unwrap().len(),
        5,
        "Local bookkeeping failure cannot erase successful remote sends from the bulk guard"
    );
    assert!(sixth.is_err() || sixth.unwrap()["approval_required"] == true);
}

fn incoming_pair(mcp: &Mcp) {
    for chat in [42, 99] {
        mcp.runtime
            .state
            .record_incoming(crate::state::IncomingEvent {
                account: "default".into(),
                chat_id: chat.to_string(),
                message_id: "1".into(),
                name: format!("chat {chat}"),
                username: None,
                bot: false,
            })
            .unwrap();
    }
}
#[tokio::test]
async fn legacy_pending_summary_counts_only_allowed_chats() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::parse(Some("42")).unwrap());
    incoming_pair(&mcp);
    let result = mcp
        .execute(
            "wait_for_new_message",
            json!({"account":"default","timeout":0.2}),
        )
        .await
        .unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["has_more"], false);
    assert_eq!(result["pending_chats"][0]["chat_id"], "42");
    assert!(!result.to_string().contains("99"));
    assert_eq!(
        mcp.runtime.state.incoming_feed_status().unwrap()["pending_chats"],
        2
    );
}
#[tokio::test]
async fn legacy_settled_wait_returns_allowed_burst_and_keeps_denied_burst_pending() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::parse(Some("42")).unwrap());
    incoming_pair(&mcp);
    let result = mcp
        .execute(
            "wait_for_settled_message",
            json!({"account":"default","settle_ms":1,"max_wait_ms":500}),
        )
        .await
        .unwrap();
    assert_eq!(result["chat_id"], "42");
    assert_eq!(
        mcp.runtime.state.incoming_feed_status().unwrap()["pending_chats"],
        1
    );
    let remaining = mcp
        .runtime
        .state
        .wait_incoming(Some("default"), None, Duration::from_millis(10), None, 10)
        .await
        .unwrap();
    assert_eq!(remaining["total"], 1);
    assert_eq!(remaining["pending_chats"][0]["chat_id"], "99");
}
