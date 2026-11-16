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
            serial: Arc::new(Mutex::new(())),
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
            serial: Arc::new(Mutex::new(())),
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
            serial: Arc::new(Mutex::new(())),
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
        "recipient-regression",
        api.clone(),
        &json!({"to_chat_id":"finance team","from_chat_id":"me","user_id":"member alias"}),
    )
    .await
    .unwrap();
    assert_eq!(approval["to_chat_id"], "7");
    assert_eq!(approval["from_chat_id"], "7");
    assert_eq!(approval["user_id"], "7");
    api.target.store(8, std::sync::atomic::Ordering::SeqCst);
    let cloned = bound.clone_handle().unwrap();
    assert_eq!(
        cloned.resolve_peer(&json!("member alias")).await.unwrap()["user_id"],
        "7"
    );
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

#[tokio::test]
async fn private_reads_and_conditional_mutation_selectors_pin_aliases_before_policy_or_approval() {
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    struct MovingAlias {
        target: AtomicI64,
        alias_resolutions: AtomicUsize,
        calls: StdMutex<Vec<(String, Value)>>,
    }
    #[async_trait]
    impl Telegram for MovingAlias {
        async fn resolve_peer(&self, input: &Value) -> Result<Value> {
            let id = if input == "moving alias" {
                self.alias_resolutions.fetch_add(1, Ordering::SeqCst);
                // Model an external rename or another process retargeting its
                // local alias immediately after this operation resolved it.
                self.target.swap(8, Ordering::SeqCst)
            } else {
                input
                    .as_i64()
                    .or_else(|| input.as_str()?.parse().ok())
                    .context("Unexpected test selector")?
            };
            Ok(if id < -1_000_000_000_000 {
                json!({"_":"inputPeerChannel","channel_id":(-id-1_000_000_000_000_i64).to_string(),"access_hash":"90"})
            } else if id < 0 {
                json!({"_":"inputPeerChat","chat_id":(-id).to_string()})
            } else {
                json!({"_":"inputPeerUser","user_id":id.to_string(),"access_hash":(id*10).to_string()})
            })
        }
        async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
            crate::tl::Schema::global().encode_call(name, &args)?;
            self.calls.lock().unwrap().push((name.into(), args.clone()));
            Ok(match name {
                "users.getFullUser" => {
                    let id = &args["id"]["user_id"];
                    json!({"full_user":{"id":id,"about":format!("private biography {}", id.as_str().unwrap())},"users":[{"_":"user","id":id,"first_name":"Owned"}]})
                }
                "users.getUsers" => {
                    json!([{"_":"user","id":args["id"][0]["user_id"],"first_name":"Owned","status":{"_":"userStatusOffline","was_online":7}}])
                }
                // This raw resolver observes the replacement owner. A contact
                // handler bypassing the private peer pin must fail this case.
                "contacts.resolveUsername" => {
                    let id = self.target.load(Ordering::SeqCst).to_string();
                    json!({"peer":{"_":"peerUser","user_id":id},"users":[{"_":"user","id":id,"access_hash":"80"}]})
                }
                "contacts.addContact" => json!(true),
                _ => panic!("A gated mutation was dispatched before approval: {name}"),
            })
        }
    }
    for (name, args, selector, restricted) in [
        (
            "get_full_user",
            json!({"username":"moving alias"}),
            "",
            true,
        ),
        (
            "get_user_status",
            json!({"user_id":"moving alias"}),
            "",
            true,
        ),
        (
            "resolve_username",
            json!({"username":"moving alias"}),
            "!retargeted",
            true,
        ),
        (
            "add_contact",
            json!({"username":"8","first_name":"Outside scope"}),
            "!denied",
            true,
        ),
        (
            "add_contact",
            json!({"phone":"+15550100","first_name":"Unproved identity"}),
            "!phone-only",
            true,
        ),
        (
            "add_contact",
            json!({"username":"moving alias","first_name":"Owned","last_name":"Contact","phone":"+15550100"}),
            "",
            true,
        ),
        (
            "set_privacy_settings",
            json!({"key":"status","base_policy":"nobody","allow_users":["moving alias"]}),
            "/allow_users/0",
            false,
        ),
        (
            "set_privacy_settings",
            json!({"key":"status","base_policy":"everyone","disallow_users":["moving alias"]}),
            "/disallow_users/0",
            false,
        ),
        (
            "set_privacy_settings",
            json!({"key":"status","rules":[{"rule":"users_allowed","users":["moving alias"]}]}),
            "/rules/0/users/0",
            false,
        ),
        (
            "set_privacy_settings",
            json!({"key":"status","rules":[{"rule":"users_disallowed","users":["moving alias"]}]}),
            "/rules/0/users/0",
            false,
        ),
        (
            "delete_revoked_invite_links",
            json!({"chat_id":-9,"admin":"moving alias"}),
            "/admin",
            false,
        ),
        (
            "export_recent_actions",
            json!({"chat_id":-1_000_000_000_009_i64,"admins":["moving alias"]}),
            "/admins/0",
            false,
        ),
        (
            "update_folder",
            json!({"folder_id":2,"patch":{"include_chat_ids":["moving alias"]}}),
            "/patch/include_chat_ids/0",
            false,
        ),
        (
            "update_folder",
            json!({"folder_id":2,"patch":{"pinned_chat_ids":["moving alias"]}}),
            "/patch/pinned_chat_ids/0",
            false,
        ),
        (
            "update_folder",
            json!({"folder_id":2,"patch":{"exclude_chat_ids":["moving alias"]}}),
            "/patch/exclude_chat_ids/0",
            false,
        ),
    ] {
        let policy = access::AccessPolicy::parse(restricted.then_some("7")).unwrap();
        let (mcp, _, _temp) = service(json!({}), policy);
        let api = Arc::new(MovingAlias {
            target: AtomicI64::new(7),
            alias_resolutions: AtomicUsize::new(0),
            calls: StdMutex::new(vec![]),
        });
        mcp.runtime.accounts.lock().await.insert(
            "default".into(),
            Arc::new(Account {
                api: api.clone(),
                serial: Arc::new(Mutex::new(())),
                generation: None,
            }),
        );
        let mut args = args;
        args["account"] = json!("default");
        let result = mcp.execute(name, args).await;
        if selector.starts_with('!') {
            let error = result.expect_err("A changed or unproved identity cannot pass its policy");
            if selector == "!retargeted" {
                assert!(
                    error.to_string().contains("Username identity changed"),
                    "{error:#}"
                );
                let calls = api.calls.lock().unwrap();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].0, "contacts.resolveUsername");
                assert_eq!(api.target.load(Ordering::SeqCst), 8);
                assert_eq!(api.alias_resolutions.load(Ordering::SeqCst), 1);
            } else {
                let expected = if selector == "!phone-only" {
                    "permitted identity"
                } else {
                    "scope"
                };
                assert!(error.to_string().contains(expected), "{error:#}");
                assert!(api.calls.lock().unwrap().is_empty());
                assert_eq!(api.alias_resolutions.load(Ordering::SeqCst), 0);
            }
            continue;
        }
        let result = result.unwrap_or_else(|e| panic!("{name} {selector}: {e:#}"));
        if !selector.is_empty() {
            assert_eq!(result["approval_required"], true, "{name} {selector}");
            let pending = mcp
                .runtime
                .state
                .read_json("approval-pending.json", json!({}))
                .unwrap();
            let request = &pending[result["fingerprint"].as_str().unwrap()];
            assert_eq!(
                request["arguments"].pointer(selector),
                Some(&json!("7")),
                "{name} {selector}"
            );
            assert!(api.calls.lock().unwrap().is_empty(), "{name} {selector}");
        } else {
            let calls = api.calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "{name}");
            if name == "add_contact" {
                assert_eq!(calls[0].0, "contacts.addContact");
                assert_eq!(calls[0].1["id"]["user_id"], "7");
                assert_eq!(calls[0].1["first_name"], "Owned");
                assert_eq!(calls[0].1["last_name"], "Contact");
                assert_eq!(calls[0].1["phone"], "+15550100");
            } else if name == "get_full_user" {
                assert_eq!(calls[0].0, "users.getFullUser");
                assert_eq!(calls[0].1["id"]["user_id"], "7");
                assert_eq!(result["full_user"]["id"], "7");
                assert_eq!(result["full_user"]["about"], "private biography 7");
            } else {
                assert_eq!(calls[0].0, "users.getUsers");
                assert_eq!(calls[0].1["id"][0]["user_id"], "7");
                assert_eq!(result["id"], "7");
            }
        }
        assert_eq!(
            api.target.load(Ordering::SeqCst),
            8,
            "Alias must really retarget"
        );
        assert_eq!(
            api.alias_resolutions.load(Ordering::SeqCst),
            1,
            "{name} {selector}"
        );
    }
}
