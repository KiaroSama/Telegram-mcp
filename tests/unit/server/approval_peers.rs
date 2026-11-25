//! Approval-peer reads use the actual MCP validation and dispatch boundary.
use super::*;

async fn isolated(name: &str, configurations: &[Option<&str>]) -> bool {
    const CHILD: &str = "TELEGRAM_APPROVAL_PEER_TEST_CHILD";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        return false;
    }
    for configured in configurations {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", name, "--nocapture"])
            .kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TELEGRAM_")
                || key.to_string_lossy().starts_with("MCP_")
            {
                command.env_remove(key);
            }
        }
        command.env(CHILD, name);
        if let Some(configured) = configured {
            if let Some(token) = configured.strip_prefix("bot-token:") {
                command.env("TELEGRAM_APPROVAL_BOT_TOKEN", token);
            } else {
                command.env("TELEGRAM_APPROVAL_CHAT_IDS", configured);
            }
        }
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .expect("Approval-peer regression subprocess timed out")
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    true
}

fn peer(id: i64) -> Value {
    if id < -1_000_000_000_000 {
        json!({"_":"inputPeerChannel","channel_id":(-id-1_000_000_000_000_i64).to_string(),"access_hash":"420"})
    } else if id < 0 {
        json!({"_":"inputPeerChat","chat_id":(-id).to_string()})
    } else {
        json!({"_":"inputPeerUser","user_id":id.to_string(),"access_hash":(id*10).to_string()})
    }
}

struct ReadProbe {
    state: Arc<State>,
    resolved: StdMutex<Vec<Value>>,
    calls: StdMutex<Vec<(String, Value)>>,
    replacement: Option<Value>,
}

#[async_trait]
impl Telegram for ReadProbe {
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.resolved.lock().unwrap().push(input.clone());
        if input.is_object() {
            return crate::tl::Schema::global().decode(
                "InputPeer",
                &crate::tl::Schema::global().encode_object(input)?,
            );
        }
        let text = input
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| input.to_string());
        if text == "me" {
            return Ok(json!({"_":"inputPeerSelf"}));
        }
        if let Some(saved) = self.state.alias_lookup("default", &text)? {
            if let Some(replacement) = &self.replacement {
                self.state
                    .alias_set("default", &text, replacement.clone(), true)?;
            }
            return Ok(saved["peer"].clone());
        }
        let id = match text.to_lowercase().as_str() {
            "@protected_user" => 7,
            "@protected_group" => -9,
            "@protected_channel" => -1_000_000_000_042,
            "@literal_bot" | "literal_bot" => 8,
            _ => text.parse::<i64>().context("Unexpected test peer")?,
        };
        Ok(peer(id))
    }

    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        crate::tl::Schema::global().encode_call(name, &args)?;
        self.calls.lock().unwrap().push((name.into(), args));
        match name {
            "messages.getHistory" => Ok(
                json!({"_":"messages.messages","messages":[{"_":"message","id":20,"date":1,"message":"private history fixture","peer_id":{"_":"peerUser","user_id":"7"}}],"users":[],"chats":[]}),
            ),
            "users.getUsers" => Ok(json!([{"_":"user","id":"8","self":true}])),
            _ => panic!("A read preflight must not dispatch a mutation: {name}"),
        }
    }
}

async fn read_service(replacement: Option<Value>) -> (Mcp, Arc<ReadProbe>, TempDir) {
    let (mcp, _, temporary) = service(json!({}), access::AccessPolicy::default());
    assert!(!mcp.runtime.policy.enabled());
    assert_eq!(
        mcp.runtime
            .tools
            .iter()
            .find(|tool| tool.name == "get_history")
            .unwrap()
            .annotations
            .as_ref()
            .unwrap()
            .read_only_hint,
        Some(true)
    );
    let api = Arc::new(ReadProbe {
        state: mcp.runtime.state.clone(),
        resolved: StdMutex::new(vec![]),
        calls: StdMutex::new(vec![]),
        replacement,
    });
    mcp.runtime.accounts.lock().await.insert(
        "default".into(),
        Arc::new(Account {
            api: api.clone(),
            serial: Arc::new(Mutex::new(())),
            generation: None,
        }),
    );
    (mcp, api, temporary)
}

#[tokio::test]
async fn bot_token_seals_its_numeric_identity_even_without_owners() {
    if isolated(
        "server::tests::approval_peers::bot_token_seals_its_numeric_identity_even_without_owners",
        &[Some("bot-token:7:fixture-only")],
    )
    .await
    {
        return;
    }
    let (mcp, api, _temporary) = read_service(None).await;
    let error = mcp
        .execute(
            "get_history",
            json!({"account":"default","chat_id":"@protected_user"}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("touches_approval_channel"));
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unrestricted_reads_refuse_aliases_and_usernames_of_numeric_approval_peers() {
    if isolated(
        "server::tests::approval_peers::unrestricted_reads_refuse_aliases_and_usernames_of_numeric_approval_peers",
        &[Some(" +0007, -9, -1000000000042 ")],
    )
    .await
    {
        return;
    }
    for (alias, username, id) in [
        ("protected person", "@protected_user", 7),
        ("protected group", "@protected_group", -9),
        (
            "protected channel",
            "@protected_channel",
            -1_000_000_000_042,
        ),
    ] {
        for selector in [json!(id), json!(alias), json!(username)] {
            let (mcp, api, _temporary) = read_service(None).await;
            mcp.runtime
                .state
                .alias_set("default", alias, peer(id), false)
                .unwrap();
            let result = mcp
                .execute(
                    "get_history",
                    json!({"account":"default","chat_id":selector}),
                )
                .await;
            assert!(
                result.is_err(),
                "A selector for a configured approval peer returned history: {selector}"
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("touches_approval_channel")
            );
            assert!(api.calls.lock().unwrap().is_empty());
        }
    }
    // Public history arguments do not admit raw private InputPeer objects.
    let (mcp, api, _temporary) = read_service(None).await;
    let error = mcp
        .execute(
            "get_history",
            json!({"account":"default","chat_id":peer(7)}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Invalid argument type"));
    assert!(api.resolved.lock().unwrap().is_empty());
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn approval_peer_reads_pin_allowed_aliases_and_recheck_the_next_operation() {
    if isolated(
        "server::tests::approval_peers::approval_peer_reads_pin_allowed_aliases_and_recheck_the_next_operation",
        &[Some("7")],
    )
    .await
    {
        return;
    }
    let (mcp, api, _temporary) = read_service(Some(peer(7))).await;
    mcp.runtime
        .state
        .alias_set("default", "moving person", peer(8), false)
        .unwrap();
    let args = json!({"account":"default","chat_id":"moving person"});
    mcp.execute("get_history", args.clone()).await.unwrap();
    assert_eq!(api.calls.lock().unwrap()[0].1["peer"], peer(8));
    assert_eq!(
        api.resolved.lock().unwrap().as_slice(),
        &[json!("moving person")]
    );
    assert_eq!(
        mcp.runtime
            .state
            .alias_lookup("default", "moving person")
            .unwrap()
            .unwrap()["peer"],
        peer(7)
    );
    let error = mcp.execute("get_history", args).await.unwrap_err();
    assert!(error.to_string().contains("touches_approval_channel"));
    assert_eq!(api.calls.lock().unwrap().len(), 1);
    assert_eq!(api.resolved.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn approval_peer_reads_keep_configured_literal_names_protected() {
    if isolated(
        "server::tests::approval_peers::approval_peer_reads_keep_configured_literal_names_protected",
        &[Some(" @Literal_Bot ")],
    )
    .await
    {
        return;
    }
    for selector in ["@Literal_Bot", "literal_bot"] {
        let (mcp, api, _temporary) = read_service(None).await;
        let error = mcp
            .execute(
                "get_history",
                json!({"account":"default","chat_id":selector}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("touches_approval_channel"));
        assert!(api.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn unrestricted_reads_without_approval_peers_keep_the_direct_self_route() {
    if isolated(
        "server::tests::approval_peers::unrestricted_reads_without_approval_peers_keep_the_direct_self_route",
        &[None, Some(""), Some(" , \t, ")],
    )
    .await
    {
        return;
    }
    let (mcp, api, _temporary) = read_service(None).await;
    mcp.execute("get_history", json!({"account":"default","chat_id":"me"}))
        .await
        .unwrap();
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "messages.getHistory");
    assert_eq!(calls[0].1["peer"]["_"], "inputPeerSelf");
    assert_eq!(api.resolved.lock().unwrap().len(), 1);
}
