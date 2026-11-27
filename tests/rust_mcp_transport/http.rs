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
        Self::start_with(
            fixture,
            fixture.command(&["serve", "--http", "127.0.0.1:0"]),
            false,
        )
        .await
    }
    async fn start_with(
        fixture: &Fixture,
        mut command: tokio::process::Command,
        tls: bool,
    ) -> Self {
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
        assert_eq!(address.scheme(), if tls { "https" } else { "http" });
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

fn tls_files(fixture: &Fixture) -> (std::path::PathBuf, std::path::PathBuf, Vec<u8>) {
    let material =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let cert = fixture.cwd.join("test-server.pem");
    let key = fixture.cwd.join("test-server-key.pem");
    let certificate = material.cert.pem().into_bytes();
    std::fs::write(&cert, &certificate).unwrap();
    std::fs::write(&key, material.signing_key.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    (cert, key, certificate)
}

#[tokio::test]
async fn http_discovery_publishes_the_effective_owner_timeout_policy() {
    for (raw, ordinary, export) in [
        (None, json!(55.0), json!(120.0)),
        (Some("600.25"), json!(600.25), json!(600.25)),
        (Some("0"), Value::Null, Value::Null),
        (Some("1e300"), json!(55.0), json!(120.0)),
    ] {
        let fixture = Fixture::new();
        let mut command = fixture.command(&["serve", "--http", "127.0.0.1:0"]);
        if let Some(raw) = raw {
            command.env("TELEGRAM_TOOL_TIMEOUT_SECONDS", raw);
        }
        // Without a bot the owner's effective approval timeout stays 300s.
        command.env("TELEGRAM_APPROVAL_TIMEOUT_SECONDS", "5");
        let server = HttpServer::start_with(&fixture, command, false).await;
        let discovery: Value = serde_json::from_slice(
            &std::fs::read(fixture.state.join("http-service.json")).unwrap(),
        )
        .unwrap();
        server.stop(&fixture).await;
        assert_eq!(
            discovery["request_timeouts"],
            json!({"version":1,"ordinary_seconds":ordinary,"export_seconds":export,"approval_seconds":300.0}),
            "The owner published a different timeout policy for {raw:?}"
        );
    }
}

#[tokio::test]
async fn nonloopback_tls_uses_real_encryption_certificate_validation_and_existing_authentication() {
    use rmcp::{
        ServiceExt,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let fixture = Fixture::new();
    let (cert, key, pem) = tls_files(&fixture);
    let mut command = fixture.command(&["serve", "--http", "0.0.0.0:0"]);
    command
        .env(
            "TELEGRAM_HTTP_TOKEN",
            "synthetic-tls-owner-token-with-more-than-32-characters",
        )
        .env("TELEGRAM_HTTP_TLS_CERT", &cert)
        .env("TELEGRAM_HTTP_TLS_KEY", &key);
    let server = HttpServer::start_with(&fixture, command, true).await;
    let trusted = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(DEADLINE)
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap())
        .build()
        .unwrap();
    let health = server.health_url();
    assert_eq!(
        trusted.get(&health).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        trusted
            .get(&health)
            .bearer_auth("wrong-owner-token")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        trusted
            .get(&health)
            .bearer_auth(&server.token)
            .header("Origin", "https://example.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    let output: Value = trusted
        .get(&health)
        .bearer_auth(&server.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(output["ready"], true);
    assert_eq!(output["pid"], server.pid);
    let transport = StreamableHttpClientTransport::with_client(
        trusted.clone(),
        StreamableHttpClientTransportConfig::with_uri(server.url.clone())
            .auth_header(server.token.clone()),
    );
    let remote = tokio::time::timeout(DEADLINE, ().serve(transport))
        .await
        .expect("TLS MCP initialization timed out")
        .unwrap();
    let exposed = tokio::time::timeout(DEADLINE, remote.list_all_tools())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        exposed
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        catalogue_names()
    );
    remote.cancel().await.unwrap();
    let mut bridge = McpProcess::start(fixture.command(&["bridge"]));
    bridge.initialize("2024-11-05").await;
    let listed = bridge.request(2, "tools/list", json!({})).await;
    assert_catalogue(tools(&listed));
    assert!(bridge.finish().await.is_empty());
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.state.join("http-service.json")).unwrap())
            .unwrap();
    assert_eq!(record["pid"], server.pid);
    assert_eq!(record["url"], server.url);
    assert!(
        client()
            .get(&health)
            .bearer_auth(&server.token)
            .send()
            .await
            .is_err(),
        "An untrusted server certificate must be rejected"
    );
    let plaintext = health.replacen("https://", "http://", 1);
    assert!(
        client()
            .get(&plaintext)
            .bearer_auth(&server.token)
            .send()
            .await
            .is_err(),
        "TLS listener must not accept a cleartext bearer request"
    );
    let private_key_header = ["-----BEGIN ", "PRIVATE KEY-----"].concat();
    fixture.assert_logs_omit(&[&server.token, &private_key_header]);
    server.stop(&fixture).await;
}

#[tokio::test]
async fn nonloopback_plaintext_and_incomplete_tls_are_refused_before_discovery() {
    for variant in 0..3 {
        let fixture = Fixture::new();
        let (cert, key, _) = tls_files(&fixture);
        let mut command = fixture.command(&["serve", "--http", "0.0.0.0:0"]);
        command.env(
            "TELEGRAM_HTTP_TOKEN",
            "synthetic-tls-owner-token-with-more-than-32-characters",
        );
        if variant == 1 {
            command.env("TELEGRAM_HTTP_TLS_CERT", cert);
        } else if variant == 2 {
            command.env("TELEGRAM_HTTP_TLS_KEY", key);
        }
        let output = tokio::time::timeout(DEADLINE, command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(if variant == 0 {
                "requires TLS"
            } else {
                "Configure both"
            })
        );
        assert!(!fixture.state.join("http-service.json").exists());
        fixture.assert_no_telegram_sessions();
    }
}

#[tokio::test]
async fn malformed_or_mismatched_tls_material_is_rejected_before_binding_a_listener() {
    for mismatched in [false, true] {
        let fixture = Fixture::new();
        let (cert, key, _) = tls_files(&fixture);
        if mismatched {
            let other = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            std::fs::write(&key, other.signing_key.serialize_pem()).unwrap();
        } else {
            std::fs::write(&key, "untrusted-private-key-canary-invalid-pem").unwrap();
        }
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = occupied.local_addr().unwrap().to_string();
        let mut command = fixture.command(&["serve", "--http", &address]);
        command
            .env("TELEGRAM_HTTP_TLS_CERT", cert)
            .env("TELEGRAM_HTTP_TLS_KEY", key);
        let output = tokio::time::timeout(DEADLINE, command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("malformed or mismatched"));
        assert!(!error.contains("untrusted-private-key-canary-invalid-pem"));
        let private_key_header = ["-----BEGIN ", "PRIVATE KEY-----"].concat();
        assert!(!error.contains(&private_key_header));
        assert!(!fixture.state.join("http-service.json").exists());
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
async fn real_health_v2_proofs_bind_nonce_target_body_and_rotating_generation_credentials() {
    use hmac::{Hmac, KeyInit, Mac};
    use rmcp::{
        ServiceExt,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    use sha2::Sha256;

    // Build the public wire format independently of the private production
    // signer/verifier, including framing and the two-byte network-order status.
    fn sign(
        key: &str,
        purpose: &str,
        nonce: &str,
        target: &str,
        status: u16,
        body: &[u8],
    ) -> String {
        let mut wire =
            format!("telegram-mcp-health-v2\0{purpose}\0{nonce}\0{target}\0").into_bytes();
        wire.extend_from_slice(&status.to_be_bytes());
        wire.extend_from_slice(body);
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(&wire);
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    const EXTERNAL: &str = "synthetic-configured-external-bearer-over-thirty-two-characters";
    let fixture = Fixture::new();
    let mut command = fixture.command(&["serve", "--http", "127.0.0.1:0"]);
    command.env("TELEGRAM_HTTP_TOKEN", EXTERNAL);
    let server = HttpServer::start_with(&fixture, command, false).await;
    let first_generation = server.token.clone();
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(fixture.state.join("http-service.json")).unwrap())
            .unwrap();
    assert_eq!(discovery["health_protocol"], 2);
    assert!(
        first_generation != EXTERNAL,
        "Discovery reused a configured external bearer"
    );
    assert!(
        first_generation.len() == 64
            && first_generation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
    );

    let nonce = "0123456789abcdef".repeat(4);
    let request_proof = sign(&first_generation, "request", &nonce, "/health", 0, &[]);
    let client = client();
    let request = client
        .get(server.health_url())
        .header("x-telegram-health-nonce", &nonce)
        .header("x-telegram-health-proof", &request_proof)
        .build()
        .unwrap();
    assert!(
        !request
            .headers()
            .contains_key(reqwest::header::AUTHORIZATION)
    );
    let response = client.execute(request).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let status = response.status().as_u16();
    let proof = response
        .headers()
        .get("x-telegram-health-proof")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let body = response.bytes().await.unwrap();
    assert!(
        proof
            == sign(
                &first_generation,
                "response",
                &nonce,
                "/health",
                status,
                &body
            ),
        "The real server response did not authenticate the complete protocol2 transcript"
    );
    let answer: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(answer["ready"], true);
    assert_eq!(answer["pid"], server.pid);

    for (changed_nonce, target, changed_proof) in [
        ("f0".repeat(32), "/health", request_proof.clone()),
        (nonce.clone(), "/health?account=work", request_proof.clone()),
        (nonce.clone(), "/health", proof),
    ] {
        let url = format!("{}{target}", server.url.strip_suffix("/mcp").unwrap());
        let refused = client
            .get(url)
            .header("x-telegram-health-nonce", changed_nonce)
            .header("x-telegram-health-proof", changed_proof)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    // The explicitly configured credential remains usable by direct MCP clients.
    let transport = StreamableHttpClientTransport::with_client(
        client.clone(),
        StreamableHttpClientTransportConfig::with_uri(server.url.clone()).auth_header(EXTERNAL),
    );
    let remote = tokio::time::timeout(DEADLINE, ().serve(transport))
        .await
        .unwrap()
        .unwrap();
    assert!(!remote.list_all_tools().await.unwrap().is_empty());
    remote.cancel().await.unwrap();
    server.stop(&fixture).await;

    let mut command = fixture.command(&["serve", "--http", "127.0.0.1:0"]);
    command.env("TELEGRAM_HTTP_TOKEN", EXTERNAL);
    let restarted = HttpServer::start_with(&fixture, command, false).await;
    let second_generation = restarted.token.clone();
    assert!(
        second_generation != first_generation && second_generation != EXTERNAL,
        "A service restart retained its old generation credential"
    );
    let old = client
        .get(restarted.health_url())
        .header("x-telegram-health-nonce", &nonce)
        .header("x-telegram-health-proof", request_proof)
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), reqwest::StatusCode::UNAUTHORIZED);
    let fresh = client
        .get(restarted.health_url())
        .header("x-telegram-health-nonce", &nonce)
        .header(
            "x-telegram-health-proof",
            sign(&second_generation, "request", &nonce, "/health", 0, &[]),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(fresh.status(), reqwest::StatusCode::OK);
    let status = fresh.status().as_u16();
    let proof = fresh
        .headers()
        .get("x-telegram-health-proof")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let body = fresh.bytes().await.unwrap();
    assert!(
        proof
            == sign(
                &second_generation,
                "response",
                &nonce,
                "/health",
                status,
                &body
            )
    );
    fixture.assert_logs_omit(&[EXTERNAL, &first_generation, &second_generation]);
    restarted.stop(&fixture).await;
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
async fn stdio_bridge_preserves_owner_instructions_in_initialize_and_discovery() {
    use rmcp::{
        ServiceExt,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let fixture = Fixture::new();
    let server = HttpServer::start(&fixture).await;
    let configuration = StreamableHttpClientTransportConfig::with_uri(server.url.as_str())
        .auth_header(&server.token);
    let owner = ()
        .serve(StreamableHttpClientTransport::with_client(
            client(),
            configuration,
        ))
        .await
        .unwrap();
    let instructions = owner
        .peer_info()
        .unwrap()
        .instructions
        .clone()
        .expect("The owner must publish its account and approval guidance");
    assert!(!instructions.is_empty());
    let mut bridge = McpProcess::start(fixture.command(&["bridge"]));
    let initialized = bridge.initialize("2024-11-05").await;
    let discovered = bridge
        .request(2, "server/discover", latest_meta(json!({})))
        .await;
    assert!(bridge.finish().await.is_empty());
    owner.cancel().await.unwrap();
    server.stop(&fixture).await;
    assert_eq!(initialized["result"]["instructions"], instructions);
    assert_eq!(discovered["result"]["instructions"], instructions);
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
