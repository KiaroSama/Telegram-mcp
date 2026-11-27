//! Ghost Mode keeps global settings distinct from account and resolved chat scopes.
use super::*;

async fn isolated(name: &str, account_counts: &[usize]) -> bool {
    const CHILD: &str = "TELEGRAM_GHOST_SCOPE_TEST_CHILD";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        return false;
    }
    for &count in account_counts {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("accounts.env");
        let configuration = [
            "TELEGRAM_SESSION_NAME_WORK=work\n",
            "TELEGRAM_SESSION_NAME_OTHER=other\n",
        ]
        .into_iter()
        .take(count)
        .collect::<String>();
        std::fs::write(&source, configuration).unwrap();
        let files = temporary.path().join("files");
        std::fs::create_dir(&files).unwrap();
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
        command
            .env(CHILD, name)
            .env("TELEGRAM_GHOST_SCOPE_TEST_CONFIG", source)
            .env("TELEGRAM_GHOST_SCOPE_TEST_COUNT", count.to_string())
            .env("TELEGRAM_STATE_DIR", temporary.path().join("state"))
            .env(
                "TELEGRAM_FILE_ROOTS",
                serde_json::to_string(&[files]).unwrap(),
            );
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .expect("Ghost scope subprocess timed out")
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

#[derive(Default)]
struct GhostProbe {
    resolved: StdMutex<Vec<Value>>,
    calls: StdMutex<Vec<(String, Value)>>,
}

#[async_trait]
impl Telegram for GhostProbe {
    async fn resolve_peer(&self, input: &Value) -> Result<Value> {
        self.resolved.lock().unwrap().push(input.clone());
        if input["_"] == "inputPeerChannel" {
            return Ok(input.clone());
        }
        if matches!(
            input.as_str(),
            Some("@work_chat" | "work_chat" | "-1000000000042")
        ) || input.as_i64() == Some(-1_000_000_000_042)
        {
            return Ok(json!({"_":"inputPeerChannel","channel_id":"42","access_hash":"99"}));
        }
        bail!("Synthetic peer cannot be resolved")
    }

    async fn invoke(&self, name: &str, args: Value) -> Result<Value> {
        assert_eq!(
            name, "account.updateStatus",
            "No message mutation is expected"
        );
        assert_eq!(args["offline"], true);
        self.calls.lock().unwrap().push((name.into(), args));
        Ok(json!(true))
    }
}

async fn fixture() -> (Mcp, Arc<GhostProbe>, Arc<GhostProbe>, usize) {
    let source = std::env::var_os("TELEGRAM_GHOST_SCOPE_TEST_CONFIG").unwrap();
    crate::account_manager::select_source(std::path::Path::new(&source)).unwrap();
    let count = std::env::var("TELEGRAM_GHOST_SCOPE_TEST_COUNT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mcp = Mcp::new().unwrap();
    let work = Arc::new(GhostProbe::default());
    let other = Arc::new(GhostProbe::default());
    for (label, api) in [("work", work.clone()), ("other", other.clone())]
        .into_iter()
        .take(count)
    {
        mcp.runtime.accounts.lock().await.insert(
            label.into(),
            Arc::new(Account {
                api,
                serial: Arc::new(Mutex::new(())),
                generation: None,
            }),
        );
    }
    (mcp, work, other, count)
}

#[tokio::test]
async fn global_ghost_controls_keep_omitted_account_global() {
    if isolated(
        "server::tests::ghost_scope::global_ghost_controls_keep_omitted_account_global",
        &[0, 1, 2],
    )
    .await
    {
        return;
    }
    let (mcp, work, other, count) = fixture().await;
    let state = &mcp.runtime.state;
    state
        .write_json(
            "ghost.json",
            &json!({"default":false,"accounts":{"work":true,"other":false},"chats":{}}),
        )
        .unwrap();
    let initial = mcp.execute("get_ghost_mode", json!({})).await.unwrap();
    assert_eq!(
        initial["effective"], false,
        "An omitted account asks for the global default"
    );
    assert!(initial["account"].is_null());
    state
        .write_json(
            "ghost.json",
            &json!({"default":false,"accounts":{"other":false},"chats":{}}),
        )
        .unwrap();
    let enabled = mcp
        .execute("set_ghost_mode", json!({"enabled":true}))
        .await
        .unwrap();
    assert_eq!(
        enabled["default"], true,
        "Global enable must update the default, not an inferred account"
    );
    let saved = state.read_json("ghost.json", Value::Null).unwrap();
    assert_eq!(saved["accounts"], json!({"other":false}));
    assert_eq!(
        mcp.runtime.accounts.lock().await.len(),
        count,
        "Global settings must not open an account"
    );
    assert_eq!(
        work.calls.lock().unwrap().len(),
        usize::from(count > 0),
        "Active accounts inheriting global Ghost Mode must be made offline"
    );
    assert!(
        other.calls.lock().unwrap().is_empty(),
        "An explicit account override remains effective"
    );
    assert!(work.resolved.lock().unwrap().is_empty());
    assert!(other.resolved.lock().unwrap().is_empty());
}

#[tokio::test]
async fn chat_ghost_controls_bind_the_selected_account_and_refuse_unknown_names() {
    if isolated("server::tests::ghost_scope::chat_ghost_controls_bind_the_selected_account_and_refuse_unknown_names", &[2]).await { return; }
    let (mcp, work, other, _) = fixture().await;
    mcp.runtime.state.set_ghost_mode(false, None, None).unwrap();
    let enabled = mcp
        .execute(
            "set_ghost_mode",
            json!({"account":"work","chat_id":"@work_chat","enabled":true}),
        )
        .await
        .unwrap();
    assert_eq!(enabled["effective"], true);
    let numeric = mcp
        .runtime
        .state
        .ghost_mode(Some("work"), Some(&json!("-1000000000042")));
    assert_eq!(
        numeric["effective"], true,
        "Named chat writes must preserve canonical peer identity"
    );
    let read = mcp
        .execute(
            "get_ghost_mode",
            json!({"account":"work","chat_id":"-1000000000042"}),
        )
        .await
        .unwrap();
    assert_eq!(read["effective"], true);
    assert!(!work.resolved.lock().unwrap().is_empty());
    assert!(other.resolved.lock().unwrap().is_empty());
    let before = mcp
        .runtime
        .state
        .read_json("ghost.json", Value::Null)
        .unwrap();
    for tool in ["get_ghost_mode", "set_ghost_mode"] {
        let args = if tool == "set_ghost_mode" {
            json!({"account":"work","chat_id":"@unresolvable","enabled":true})
        } else {
            json!({"account":"work","chat_id":"@unresolvable"})
        };
        assert!(
            mcp.execute(tool, args).await.is_err(),
            "An unresolved chat must not be reported as successfully scoped"
        );
    }
    assert_eq!(
        mcp.runtime
            .state
            .read_json("ghost.json", Value::Null)
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn chat_ghost_controls_require_an_explicit_account() {
    if isolated(
        "server::tests::ghost_scope::chat_ghost_controls_require_an_explicit_account",
        &[1],
    )
    .await
    {
        return;
    }
    let (mcp, work, _, _) = fixture().await;
    for tool in ["get_ghost_mode", "set_ghost_mode"] {
        let args = if tool == "set_ghost_mode" {
            json!({"chat_id":"@work_chat","enabled":true})
        } else {
            json!({"chat_id":"@work_chat"})
        };
        assert!(
            mcp.execute(tool, args).await.is_err(),
            "A chat setting needs its explicit account"
        );
    }
    assert!(work.resolved.lock().unwrap().is_empty());
    assert!(work.calls.lock().unwrap().is_empty());
    assert!(
        mcp.runtime
            .state
            .read_json("ghost.json", Value::Null)
            .unwrap()
            .is_null()
    );
}
