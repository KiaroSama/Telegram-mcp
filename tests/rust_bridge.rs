//! Native bridge cold-start/reuse lifecycle with real OS processes and sockets.
#[allow(dead_code)]
#[path = "rust_mcp_transport/common.rs"]
mod common;
use common::*;
use fs2::FileExt;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn discovery(fixture: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(fixture.state.join("http-service.json")).unwrap())
        .unwrap()
}
async fn health(record: &Value) -> Value {
    let mut url = url::Url::parse(record["url"].as_str().unwrap()).unwrap();
    url.set_path("/health");
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
        .get(url)
        .bearer_auth(record["token"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

struct Daemon {
    pid: u32,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    active: bool,
}
impl Daemon {
    fn owned(record: &Value) -> Self {
        let pid = record["pid"].as_u64().unwrap() as u32;
        #[cfg(windows)]
        let handle = unsafe {
            use windows_sys::Win32::System::Threading::*;
            let handle = OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            assert!(!handle.is_null());
            handle
        };
        Self {
            pid,
            #[cfg(windows)]
            handle,
            active: true,
        }
    }
    async fn stop(mut self, fixture: &Fixture) {
        // The private descriptor and authenticated response identify the exact
        // daemon created by this test's bridge; no global process search is used.
        #[cfg(unix)]
        assert_eq!(unsafe { libc::kill(self.pid as i32, libc::SIGINT) }, 0);
        #[cfg(windows)]
        assert_eq!(
            unsafe { windows_sys::Win32::System::Threading::GetProcessId(self.handle) },
            self.pid
        );
        #[cfg(windows)]
        assert_ne!(
            unsafe { windows_sys::Win32::System::Threading::TerminateProcess(self.handle, 1) },
            0
        );
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(fixture.state.join("http-service.lock"))
                .unwrap();
            if FileExt::try_lock_exclusive(&file).is_ok() {
                FileExt::unlock(&file).unwrap();
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Test-owned daemon did not release its lease"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.active = false;
        fixture.assert_no_telegram_sessions();
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if self.active {
            #[cfg(unix)]
            unsafe {
                libc::kill(self.pid as i32, libc::SIGKILL);
            }
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(self.handle, 1);
            }
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[tokio::test]
async fn bridge_cold_start_preserves_explicit_config_and_outlives_each_client() {
    let fixture = Fixture::new();
    // A forgotten --config in the spawned argv would load this invalid token.
    fixture.env_file("TELEGRAM_HTTP_TOKEN=short\n");
    let source = fixture.cwd.join("selected حساب.env");
    std::fs::write(&source, format!("MCP_PORT={}\n", port())).unwrap();
    let args = ["--config", source.to_str().unwrap(), "bridge"];
    let mut first = McpProcess::start(fixture.command(&args));
    first.initialize("2024-11-05").await;
    let record = discovery(&fixture);
    let daemon = Daemon::owned(&record);
    let status = health(&record).await;
    assert_eq!(status["pid"], record["pid"]);
    assert_eq!(status["ready"], true);
    let accounts = first
        .request(
            2,
            "tools/call",
            latest_meta(json!({"name":"list_accounts","arguments":{}})),
        )
        .await;
    assert_ne!(accounts["result"]["isError"], true);
    assert!(first.finish().await.is_empty());
    assert_eq!(health(&record).await["pid"], record["pid"]);
    let mut second = McpProcess::start(fixture.command(&args));
    second.initialize("2024-11-05").await;
    assert_catalogue(tools(&second.request(2, "tools/list", json!({})).await));
    assert_eq!(discovery(&fixture), record);
    assert!(second.finish().await.is_empty());
    assert_eq!(health(&record).await["pid"], record["pid"]);
    daemon.stop(&fixture).await;
}

#[tokio::test]
async fn concurrent_bridges_publish_one_shared_daemon_and_keep_independent_mcp_sessions() {
    let fixture = Fixture::new();
    fixture.env_file(&format!("MCP_PORT={}\n", port()));
    let mut first = McpProcess::start(fixture.command(&["bridge"]));
    let mut second = McpProcess::start(fixture.command(&["bridge"]));
    tokio::join!(
        first.initialize("2024-11-05"),
        second.initialize("2024-11-05")
    );
    let record = discovery(&fixture);
    let daemon = Daemon::owned(&record);
    let (left, right) = tokio::join!(
        first.request(2, "tools/list", json!({})),
        second.request(2, "tools/list", json!({}))
    );
    assert_catalogue(tools(&left));
    assert_catalogue(tools(&right));
    assert!(first.finish().await.is_empty());
    assert_eq!(health(&record).await["pid"], record["pid"]);
    let accounts = second
        .request(
            3,
            "tools/call",
            latest_meta(json!({"name":"list_accounts","arguments":{}})),
        )
        .await;
    assert_ne!(accounts["result"]["isError"], true);
    assert!(second.finish().await.is_empty());
    assert_eq!(discovery(&fixture), record);
    daemon.stop(&fixture).await;
}

async fn refused(fixture: &Fixture) -> std::process::Output {
    let child = fixture
        .command(&["bridge"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = tokio::time::timeout(DEADLINE, child.wait_with_output())
        .await
        .expect("Refused bridge startup did not stop")
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    output
}

#[tokio::test]
async fn bridge_refuses_an_unknown_listener_without_sending_credentials_or_replacing_it() {
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    fixture.env_file(&format!(
        "MCP_PORT={}\n",
        listener.local_addr().unwrap().port()
    ));
    let output = refused(&fixture).await;
    assert!(String::from_utf8_lossy(&output.stderr).contains("unverified listener"));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err(),
        "An unknown listener must receive no connection, bearer credential or MCP request"
    );
    assert!(!fixture.state.join("http-service.json").exists());
    fixture.assert_no_telegram_sessions();
}

#[tokio::test]
async fn bridge_failed_start_releases_owned_leases_and_never_opens_telegram_authorization() {
    let fixture = Fixture::new();
    fixture.env_file(&format!("MCP_PORT={}\nTELEGRAM_HTTP_TOKEN=short\n", port()));
    let output = refused(&fixture).await;
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("startup failed"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["bridge-start.lock", "http-service.lock"] {
        let path = fixture.state.join(name);
        if path.exists() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap();
            FileExt::try_lock_exclusive(&file).expect("Failed startup retained a service lease");
            FileExt::unlock(&file).unwrap();
        }
    }
    fixture.assert_no_telegram_sessions();
}

#[tokio::test]
async fn bridge_invalid_remote_endpoint_is_refused_before_any_daemon_is_launched() {
    let fixture = Fixture::new();
    fixture.env_file("MCP_HOST=192.0.2.1\n");
    let output = refused(&fixture).await;
    assert!(String::from_utf8_lossy(&output.stderr).contains("loopback"));
    assert!(!fixture.state.join("http-service.json").exists());
    assert!(!fixture.state.join("bridge-start.lock").exists());
    fixture.assert_no_telegram_sessions();
}
