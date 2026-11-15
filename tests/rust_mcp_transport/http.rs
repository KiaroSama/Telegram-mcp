use super::common::*;
use fs2::FileExt;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Child, task::JoinHandle};

struct HttpServer {
    child: Child,
    stdout: JoinHandle<Vec<u8>>,
    stderr: JoinHandle<Vec<u8>>,
    url: String,
    token: String,
    pid: u32,
}
impl HttpServer {
    async fn start(fixture: &Fixture) -> Self {
        let mut command = fixture.command(&["serve", "--http", "127.0.0.1:0"]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        let mut stdout = child.stdout.take().unwrap().take(2 * 1024 * 1024);
        let stdout = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        let mut stderr = child.stderr.take().unwrap().take(2 * 1024 * 1024);
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        let deadline = Instant::now() + DEADLINE;
        let discovery = loop {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("HTTP child exited before publishing discovery: {status}");
            }
            if let Ok(bytes) = std::fs::read(fixture.state.join("http-service.json"))
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && value["pid"].as_u64() == Some(u64::from(pid))
            {
                break value;
            }
            assert!(
                Instant::now() < deadline,
                "HTTP child did not publish its private discovery record"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let url = discovery["url"].as_str().unwrap().to_owned();
        let token = discovery["token"].as_str().unwrap().to_owned();
        assert!(token.len() >= 32);
        let address = url::Url::parse(&url).unwrap();
        assert_eq!(address.host_str(), Some("127.0.0.1"));
        assert!(address.port().is_some_and(|port| port > 0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(fixture.state.join("http-service.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.state.join("http-service.lock"))
            .unwrap();
        assert!(
            FileExt::try_lock_exclusive(&file).is_err(),
            "The live HTTP process must own its service lease"
        );
        Self {
            child,
            stdout,
            stderr,
            url,
            token,
            pid,
        }
    }
    fn health_url(&self) -> String {
        format!("{}/health", self.url.strip_suffix("/mcp").unwrap())
    }
    async fn stop(mut self, fixture: &Fixture) {
        #[cfg(unix)]
        {
            let pid = self.child.id().unwrap();
            // SAFETY: this PID belongs to the child created and still owned by
            // this test; no process-name matching or global signal is used.
            assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) }, 0);
        }
        #[cfg(not(unix))]
        {
            self.child.start_kill().unwrap();
        }
        let status = tokio::time::timeout(DEADLINE, self.child.wait())
            .await
            .expect("HTTP child did not stop")
            .unwrap();
        #[cfg(unix)]
        assert!(
            status.success(),
            "HTTP service must shut down cleanly after SIGINT"
        );
        #[cfg(not(unix))]
        assert!(
            !status.success(),
            "The test-owned HTTP process was explicitly terminated"
        );
        assert!(
            self.stdout.await.unwrap().is_empty(),
            "HTTP server must not print protocol data or logs on stdout"
        );
        assert!(
            self.stderr.await.unwrap().is_empty(),
            "HTTP server must keep routine logs in private state"
        );
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.state.join("http-service.lock"))
            .unwrap();
        FileExt::try_lock_exclusive(&file).expect("Stopped HTTP child retained the service lease");
        FileExt::unlock(&file).unwrap();
        #[cfg(unix)]
        {
            let value: Value = serde_json::from_slice(
                &std::fs::read(fixture.state.join("http-service.json")).unwrap(),
            )
            .unwrap();
            assert!(
                value.is_null(),
                "Graceful HTTP shutdown must invalidate its discovery token"
            );
        }
        fixture.assert_no_telegram_sessions();
    }
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DEADLINE)
        .build()
        .unwrap()
}

#[tokio::test]
async fn http_authentication_origin_policy_and_local_health_run_in_a_real_process() {
    let fixture = Fixture::new();
    let server = HttpServer::start(&fixture).await;
    let client = client();
    let url = server.health_url();
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("synthetic-wrong-owner-token")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&server.token)
            .header("Origin", "https://example.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    let response = client
        .get(&url)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let health: Value = response.json().await.unwrap();
    assert_eq!(health["ready"], true);
    assert_eq!(health["backend"], "grammers");
    assert_eq!(health["pid"], server.pid);
    assert_eq!(health["accounts"]["accounts"], json!([]));
    let unknown: Value = client
        .get(&url)
        .query(&[("account", "work")])
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unknown["status"], "unknown");
    fixture.assert_no_telegram_sessions();
    server.stop(&fixture).await;
}

#[tokio::test]
async fn stdio_bridge_uses_existing_authenticated_http_owner_without_opening_telegram() {
    let fixture = Fixture::new();
    let server = HttpServer::start(&fixture).await;
    let mut bridge = McpProcess::start(fixture.command(&["bridge"]));
    bridge.initialize("2024-11-05").await;
    let listed = bridge.request(2, "tools/list", json!({})).await;
    assert_catalogue(tools(&listed));
    let accounts = text_result(
        &bridge
            .request(
                3,
                "tools/call",
                json!({"name":"list_accounts","arguments":{}}),
            )
            .await,
    );
    assert_eq!(accounts["accounts"], json!([]));
    assert!(bridge.finish().await.is_empty());
    let health: Value = client()
        .get(server.health_url())
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["pid"], server.pid);
    fixture.assert_no_telegram_sessions();
    server.stop(&fixture).await;
}

#[tokio::test]
async fn http_owner_lease_rejects_a_second_process_and_releases_for_restart() {
    let fixture = Fixture::new();
    let server = HttpServer::start(&fixture).await;
    let second = cli(&fixture, &["serve", "--http", "127.0.0.1:0"], None).await;
    assert!(!second.status.success());
    assert!(second.stdout.is_empty());
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(fixture.state.join("http-service.json")).unwrap())
            .unwrap();
    assert_eq!(discovery["pid"], server.pid);
    assert!(discovery["token"].as_str() == Some(server.token.as_str()));
    server.stop(&fixture).await;
    let restarted = HttpServer::start(&fixture).await;
    let health: Value = client()
        .get(restarted.health_url())
        .bearer_auth(&restarted.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["ready"], true);
    restarted.stop(&fixture).await;
}
