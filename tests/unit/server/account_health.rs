use super::*;

struct HealthOwner {
    calls: StdMutex<Vec<Value>>,
}

#[async_trait]
impl Telegram for HealthOwner {
    async fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        assert_eq!(method, "local.session_health");
        self.calls.lock().unwrap().push(params);
        Ok(json!({"status":"healthy","authorized":true,"account":"default"}))
    }

    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        bail!("Session health must not resolve peers")
    }
}

#[tokio::test]
async fn pooled_session_health_routes_the_public_tool_to_its_existing_default_owner() {
    const CHILD: &str = "TELEGRAM_POOL_HEALTH_TEST_CONFIG";
    let Some(source) = std::env::var_os(CHILD) else {
        let mut failures = Vec::new();
        for count in [0, 2] {
            let temporary = tempfile::tempdir().unwrap();
            let source = temporary.path().join("accounts.env");
            let configuration = [
                "TELEGRAM_SESSION_NAME_ONE=one\n",
                "TELEGRAM_SESSION_NAME_TWO=two\n",
            ]
            .into_iter()
            .take(count)
            .collect::<String>();
            std::fs::write(&source, configuration).unwrap();
            let files = temporary.path().join("files");
            std::fs::create_dir(&files).unwrap();
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "server::tests::account_health::pooled_session_health_routes_the_public_tool_to_its_existing_default_owner", "--nocapture", "--test-threads=1"])
                .kill_on_drop(true);
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("TELEGRAM_")
                    || key.to_string_lossy().starts_with("MCP_")
                {
                    command.env_remove(key);
                }
            }
            command
                .env(CHILD, source)
                .env("TELEGRAM_STATE_DIR", temporary.path().join("state"))
                .env(
                    "TELEGRAM_FILE_ROOTS",
                    serde_json::to_string(&[files]).unwrap(),
                );
            let output = tokio::time::timeout(Duration::from_secs(20), command.output())
                .await
                .expect("Pooled health subprocess exceeded its deadline")
                .unwrap();
            if !output.status.success() {
                failures.push(format!(
                    "{count} configured accounts: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        return;
    };
    crate::account_manager::select_source(std::path::Path::new(&source)).unwrap();
    let (mcp, other, _temp) = service(json!({}), access::AccessPolicy::default());
    let owner = Arc::new(HealthOwner {
        calls: StdMutex::new(Vec::new()),
    });
    mcp.runtime.accounts.lock().await.insert(
        "default".into(),
        Arc::new(Account {
            api: owner.clone(),
            serial: Arc::new(Mutex::new(())),
            generation: None,
        }),
    );
    let args = json!({
        "config_key":"TELEGRAM_SESSION_STRINGS",
        "config_digest":crate::account_manager::config_digest("synthetic-pool-source-one;synthetic-pool-source-two")
    });
    let healthy = mcp
        .execute("check_account_session", args.clone())
        .await
        .unwrap();
    assert_eq!(healthy["status"], "healthy");
    assert_eq!(healthy["authorized"], true);
    assert_eq!(healthy["account"], "default");
    assert_eq!(*owner.calls.lock().unwrap(), vec![args.clone()]);
    assert!(other.calls.lock().unwrap().is_empty());

    mcp.runtime.accounts.lock().await.remove("default");
    let disconnected = mcp.execute("check_account_session", args).await.unwrap();
    assert_eq!(disconnected["status"], "unknown");
    assert_eq!(disconnected["authorized"], Value::Null);
    assert_eq!(disconnected["account"], "default");
    assert_eq!(owner.calls.lock().unwrap().len(), 1);
    assert!(other.calls.lock().unwrap().is_empty());
    assert!(!mcp.runtime.accounts.lock().await.contains_key("default"));
    assert!(crate::account_manager::label_for("TELEGRAM_SESSION_STRINGS").is_err());
}

struct ClosingOwner {
    closed: std::sync::atomic::AtomicBool,
    closes: std::sync::atomic::AtomicUsize,
    refuse_close: bool,
}

#[async_trait]
impl Telegram for ClosingOwner {
    async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
        bail!("A retired account must not be invoked")
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        bail!("A retired account must not resolve peers")
    }
    fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
    async fn close_connection(&self) -> Result<()> {
        self.closes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.refuse_close {
            bail!("synthetic worker refused to stop");
        }
        Ok(())
    }
}

fn closing_owner(closed: bool, refuse_close: bool) -> Arc<ClosingOwner> {
    Arc::new(ClosingOwner {
        closed: closed.into(),
        closes: 0.into(),
        refuse_close,
    })
}

async fn insert_owner(mcp: &Mcp, label: &str, owner: Arc<ClosingOwner>) {
    mcp.runtime.accounts.lock().await.insert(
        label.into(),
        Arc::new(Account {
            api: owner,
            serial: Arc::new(Mutex::new(())),
            generation: None,
        }),
    );
}

#[tokio::test]
async fn live_cached_account_is_reused_without_shutdown() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let owner = closing_owner(false, false);
    insert_owner(&mcp, "zombie", owner.clone()).await;
    mcp.account("zombie").await.unwrap();
    assert_eq!(owner.closes.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(mcp.runtime.accounts.lock().await.contains_key("zombie"));
}

#[tokio::test]
async fn closed_account_is_shut_down_evicted_and_reopened() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let owner = closing_owner(true, false);
    insert_owner(&mcp, "zombie", owner.clone()).await;
    // With no configuration the reopen step reports it, proving eviction reached it.
    let error = mcp.account("zombie").await.err().unwrap().to_string();
    assert!(error.contains("not configured"), "{error}");
    assert_eq!(owner.closes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!mcp.runtime.accounts.lock().await.contains_key("zombie"));
    assert_eq!(
        Arc::strong_count(&owner),
        1,
        "the retired account was dropped"
    );
}

#[tokio::test]
async fn failed_shutdown_refuses_reopening_and_keeps_the_entry() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let owner = closing_owner(true, true);
    insert_owner(&mcp, "zombie", owner.clone()).await;
    let error = format!("{:#}", mcp.account("zombie").await.err().unwrap());
    assert!(error.contains("was not reopened"), "{error}");
    assert_eq!(owner.closes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(mcp.runtime.accounts.lock().await.contains_key("zombie"));
}

#[derive(Default)]
struct RetirementControl {
    entered: tokio::sync::Notify,
    finish: tokio::sync::Notify,
    closes: std::sync::atomic::AtomicUsize,
}

struct RetiringOwner {
    control: Arc<RetirementControl>,
    _lease: crate::session::PersistentSession,
}

#[async_trait]
impl Telegram for RetiringOwner {
    async fn invoke(&self, _: &str, _: Value) -> Result<Value> {
        bail!("A retired account must not be invoked")
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        bail!("A retired account must not resolve peers")
    }
    fn is_closed(&self) -> bool {
        true
    }
    async fn close_connection(&self) -> Result<()> {
        self.control
            .closes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.control.entered.notify_one();
        self.control.finish.notified().await;
        Ok(())
    }
}

async fn insert_retiring_owner(mcp: &Mcp) -> Arc<RetirementControl> {
    let control = Arc::new(RetirementControl::default());
    let lease =
        crate::session::PersistentSession::open(Arc::clone(&mcp.runtime.state), "zombie").unwrap();
    mcp.runtime.accounts.lock().await.insert(
        "zombie".into(),
        Arc::new(Account {
            api: Arc::new(RetiringOwner {
                control: Arc::clone(&control),
                _lease: lease,
            }),
            serial: Arc::new(Mutex::new(())),
            generation: None,
        }),
    );
    control
}

#[tokio::test(start_paused = true)]
async fn closed_account_retirement_serializes_waiters_without_blocking_other_accounts() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let control = insert_retiring_owner(&mcp).await;
    let serial = mcp.runtime.accounts.lock().await["zombie"].serial.clone();
    let admitted_cleanup = serial.lock_owned().await;
    let first_mcp = mcp.clone();
    let first = tokio::spawn(async move { first_mcp.account("zombie").await });
    let closed_before_cleanup =
        tokio::time::timeout(Duration::from_secs(1), control.entered.notified())
            .await
            .is_ok();
    drop(admitted_cleanup);
    if !closed_before_cleanup {
        control.entered.notified().await;
    }
    let second_mcp = mcp.clone();
    let mut second = tokio::spawn(async move { second_mcp.account("zombie").await });
    let early_second = tokio::time::timeout(Duration::from_secs(1), &mut second).await;
    let other = tokio::time::timeout(Duration::from_secs(1), mcp.account("other")).await;
    let lease_still_held =
        crate::session::PersistentSession::open(Arc::clone(&mcp.runtime.state), "zombie").is_err();
    control.finish.notify_one();
    let first_result = first.await.unwrap();
    let blocked = early_second.is_err();
    let second_result = match early_second {
        Ok(result) => result.unwrap(),
        Err(_) => second.await.unwrap(),
    };
    assert!(
        blocked,
        "A concurrent caller reached reopening before retirement finished"
    );
    assert!(
        !closed_before_cleanup,
        "Retirement interrupted admitted operation cleanup"
    );
    assert!(
        other.is_ok_and(|result| result.is_ok()),
        "Retirement blocked another account"
    );
    assert!(
        lease_still_held,
        "Retirement must retain its real session lease"
    );
    assert!(
        first_result
            .err()
            .unwrap()
            .to_string()
            .contains("not configured")
    );
    assert!(
        second_result
            .err()
            .unwrap()
            .to_string()
            .contains("not configured")
    );
    assert_eq!(control.closes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        crate::session::PersistentSession::open(Arc::clone(&mcp.runtime.state), "zombie").is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_account_retirement_preserves_the_owner_and_lease_for_retry() {
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let control = insert_retiring_owner(&mcp).await;
    let first_mcp = mcp.clone();
    let first = tokio::spawn(async move { first_mcp.account("zombie").await });
    control.entered.notified().await;
    first.abort();
    assert!(matches!(first.await, Err(error) if error.is_cancelled()));
    let retained = mcp.runtime.accounts.lock().await.contains_key("zombie");
    let lease_retained =
        crate::session::PersistentSession::open(Arc::clone(&mcp.runtime.state), "zombie").is_err();
    control.finish.notify_one();
    let result = mcp.account("zombie").await;
    assert!(
        retained,
        "A cancelled shutdown lost the retired owner from the cache"
    );
    assert!(
        lease_retained,
        "A cancelled shutdown lost ownership of its session lease"
    );
    assert_eq!(control.closes.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(result.err().unwrap().to_string().contains("not configured"));
    assert!(
        crate::session::PersistentSession::open(Arc::clone(&mcp.runtime.state), "zombie").is_ok()
    );
}

#[tokio::test]
async fn disconnected_accounts_still_refuse_changed_authorization_configuration() {
    for closed in [false, true] {
        let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
        let owner = closing_owner(closed, false);
        mcp.runtime.accounts.lock().await.insert(
            "zombie".into(),
            Arc::new(Account {
                api: owner.clone(),
                serial: Arc::new(Mutex::new(())),
                generation: Some("retained-authorization-revision".into()),
            }),
        );
        let result = mcp.account("zombie").await;
        let error = result.err().unwrap().to_string();
        assert!(
            error.contains("configuration changed"),
            "closed={closed}: {error}"
        );
        assert_eq!(owner.closes.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(mcp.runtime.accounts.lock().await.contains_key("zombie"));
    }
}

#[tokio::test]
async fn account_configuration_is_rechecked_after_retirement_before_eviction() {
    const CHILD: &str = "TELEGRAM_RETIREMENT_CONFIG_TEST";
    let Some(source) = std::env::var_os(CHILD) else {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("accounts.env");
        std::fs::write(&source, "TELEGRAM_SESSION_NAME_ZOMBIE=original\n").unwrap();
        let files = temporary.path().join("files");
        std::fs::create_dir(&files).unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "server::tests::account_health::account_configuration_is_rechecked_after_retirement_before_eviction", "--nocapture", "--test-threads=1"])
            .kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TELEGRAM_")
                || key.to_string_lossy().starts_with("MCP_")
            {
                command.env_remove(key);
            }
        }
        command
            .env(CHILD, source)
            .env("TELEGRAM_STATE_DIR", temporary.path().join("state"))
            .env(
                "TELEGRAM_FILE_ROOTS",
                serde_json::to_string(&[files]).unwrap(),
            );
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .expect("Retirement configuration subprocess exceeded its deadline")
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let source = std::path::Path::new(&source);
    crate::account_manager::select_source(source).unwrap();
    let (mcp, _, _temp) = service(json!({}), access::AccessPolicy::default());
    let control = insert_retiring_owner(&mcp).await;
    {
        let mut accounts = mcp.runtime.accounts.lock().await;
        Arc::get_mut(accounts.get_mut("zombie").unwrap())
            .unwrap()
            .generation = crate::backend::account_generation("zombie").unwrap();
    }
    let caller = mcp.clone();
    let call = tokio::spawn(async move { caller.account("zombie").await });
    control.entered.notified().await;
    std::fs::write(source, "TELEGRAM_SESSION_NAME_ZOMBIE=replacement\n").unwrap();
    control.finish.notify_one();
    let error = call.await.unwrap().err().unwrap().to_string();
    assert!(error.contains("configuration changed"), "{error}");
    assert!(mcp.runtime.accounts.lock().await.contains_key("zombie"));
    assert_eq!(control.closes.load(std::sync::atomic::Ordering::SeqCst), 1);
}
