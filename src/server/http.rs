use super::*;
use axum::{
    Router,
    extract::{Query, Request, State as AxumState},
    http::{StatusCode, header::WWW_AUTHENTICATE},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use axum_server::tls_rustls::RustlsConfig;
use fs2::FileExt;
use hmac::{Hmac, KeyInit, Mac};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub(super) const OWNER_BEARER_CHALLENGE: &str = "Bearer realm=\"telegram-mcp\"";

pub(super) fn session_manager(keep_alive: Duration) -> LocalSessionManager {
    let mut manager = LocalSessionManager::default();
    manager.session_config.keep_alive = Some(keep_alive);
    manager
}

#[derive(Clone)]
struct ServiceAuthentication {
    local: String,
    external: Option<String>,
}

fn health_signature(
    token: &str,
    purpose: &[u8],
    nonce: &str,
    target: &str,
    status: u16,
    body: &[u8],
) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes())
        .expect("HMAC accepts keys of every length");
    for part in [
        b"telegram-mcp-health-v2\0".as_slice(),
        purpose,
        b"\0",
        nonce.as_bytes(),
        b"\0",
        target.as_bytes(),
        b"\0",
        &status.to_be_bytes(),
        body,
    ] {
        mac.update(part);
    }
    mac
}

fn proof_hex(mac: Hmac<Sha256>) -> String {
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verify_proof(mac: Hmac<Sha256>, proof: &str) -> Result<()> {
    anyhow::ensure!(
        proof.len() == 64 && proof.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid health authentication proof"
    );
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&proof[index * 2..index * 2 + 2], 16)?;
    }
    mac.verify_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Health response identity could not be verified"))
}

pub(crate) fn health_request_proof(token: &str, nonce: &str, target: &str) -> String {
    proof_hex(health_signature(token, b"request", nonce, target, 0, &[]))
}

pub(crate) fn verify_health_response(
    token: &str,
    nonce: &str,
    target: &str,
    status: u16,
    body: &[u8],
    proof: &str,
) -> Result<()> {
    verify_proof(
        health_signature(token, b"response", nonce, target, status, body),
        proof,
    )
}

pub(crate) fn local_http_client(certificate_pem: Option<&str>) -> Result<reqwest::Client> {
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .connect_timeout(Duration::from_secs(5));
    if let Some(pem) = certificate_pem {
        anyhow::ensure!(
            pem.len() <= 256 * 1024,
            "Shared service certificate exceeds its size budget"
        );
        for certificate in reqwest::Certificate::from_pem_bundle(pem.as_bytes())
            .context("Shared service certificate is malformed")?
        {
            client = client.add_root_certificate(certificate);
        }
    }
    Ok(client.build()?)
}

/// A cancelled server future must not leave a detached signal waiter holding
/// the TLS shutdown handle alive.
pub(super) struct ShutdownSignal {
    task: tokio::task::JoinHandle<()>,
    listener: axum_server::Handle<std::net::SocketAddr>,
    sessions: tokio_util::sync::CancellationToken,
}
impl ShutdownSignal {
    pub(super) fn new(
        task: tokio::task::JoinHandle<()>,
        listener: axum_server::Handle<std::net::SocketAddr>,
        sessions: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            task,
            listener,
            sessions,
        }
    }
}
impl Drop for ShutdownSignal {
    fn drop(&mut self) {
        self.sessions.cancel();
        self.listener.shutdown();
        self.task.abort();
    }
}

async fn tls_configuration(
    address: std::net::SocketAddr,
    cert: Option<std::path::PathBuf>,
    key: Option<std::path::PathBuf>,
) -> Result<(Option<RustlsConfig>, Option<String>)> {
    match (cert, key) {
        (None, None) => {
            if !address.ip().is_loopback() {
                bail!(
                    "A non-loopback HTTP service requires TLS: configure TELEGRAM_HTTP_TLS_CERT and TELEGRAM_HTTP_TLS_KEY"
                );
            }
            Ok((None, None))
        }
        (Some(cert), Some(key)) => {
            use tokio::io::AsyncReadExt;
            let mut certificate = Vec::new();
            tokio::fs::File::open(cert)
                .await?
                .take(256 * 1024 + 1)
                .read_to_end(&mut certificate)
                .await?;
            let mut private_key = zeroize::Zeroizing::new(Vec::new());
            tokio::fs::File::open(key)
                .await?
                .take(64 * 1024 + 1)
                .read_to_end(&mut private_key)
                .await?;
            anyhow::ensure!(
                certificate.len() <= 256 * 1024 && private_key.len() <= 64 * 1024,
                "TLS certificate/key exceeds its size budget"
            );
            let public = String::from_utf8(certificate.clone())
                .context("TLS certificate is not PEM text")?;
            let configuration = RustlsConfig::from_pem(certificate, private_key.to_vec())
                .await
                .map_err(|_| anyhow::anyhow!("TLS certificate/key is malformed or mismatched"))?;
            Ok((Some(configuration), Some(public)))
        }
        _ => bail!("Configure both TELEGRAM_HTTP_TLS_CERT and TELEGRAM_HTTP_TLS_KEY"),
    }
}
pub(super) async fn serve(server: Mcp, address: std::net::SocketAddr) -> Result<()> {
    let configured = std::env::var("TELEGRAM_HTTP_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    if !address.ip().is_loopback() && configured.as_ref().is_none_or(|s| s.len() < 32) {
        bail!(
            "A non-loopback HTTP service requires TELEGRAM_HTTP_TOKEN with at least 32 characters"
        );
    }
    let token: String = {
        (0..32)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect()
    };
    if configured.as_ref().is_some_and(|value| value.len() < 32) {
        bail!("TELEGRAM_HTTP_TOKEN must contain at least 32 characters");
    }
    // Validate the complete matching pair before binding a listener or
    // publishing discovery. No TLS failure falls back to plaintext.
    let (tls, certificate_pem) = tls_configuration(
        address,
        std::env::var_os("TELEGRAM_HTTP_TLS_CERT")
            .filter(|value| !value.is_empty())
            .map(Into::into),
        std::env::var_os("TELEGRAM_HTTP_TLS_KEY")
            .filter(|value| !value.is_empty())
            .map(Into::into),
    )
    .await?;
    let scheme = if tls.is_some() { "https" } else { "http" };
    let state = Arc::clone(&server.runtime.state);
    let path = state.root().join("http-service.lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lease = options.open(path)?;
    FileExt::try_lock_exclusive(&lease)
        .context("A shared HTTP service already owns this state directory")?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    // Existing MCP connections are detached from axum's accept future. Cancel
    // their workers before this scope releases the service ownership lease.
    let sessions = shutdown.clone().drop_guard();
    let factory = server.clone();
    let mut config = StreamableHttpServerConfig::default()
        .enforce_origin_validation()
        .with_cancellation_token(shutdown.child_token());
    if let Ok(hosts) = std::env::var("TELEGRAM_HTTP_ALLOWED_HOSTS") {
        config =
            config.with_allowed_hosts(hosts.split(',').map(str::trim).filter(|s| !s.is_empty()));
    }
    let service: StreamableHttpService<Mcp, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(factory.clone()),
        session_manager(Duration::from_secs(3600)).into(),
        config,
    );
    let bot = server.runtime.guard.bot.clone();
    let router = Router::new()
        .nest_service("/mcp", service)
        .route("/health", axum::routing::get(health))
        .with_state(server)
        .merge(crate::transfer::http::router(crate::transfer::global()?));
    let router = authenticated_router(router, token.clone(), configured);
    let listener = tokio::net::TcpListener::bind(address).await?;
    let actual = listener.local_addr()?;
    let endpoint = if actual.ip().is_unspecified() {
        let ip = if actual.is_ipv6() {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        };
        format!(
            "{scheme}://{}/mcp",
            std::net::SocketAddr::new(ip, actual.port())
        )
    } else {
        format!("{scheme}://{actual}/mcp")
    };
    state.write_json("http-service.json",&json!({"url":endpoint,"token":token,"certificate_pem":certificate_pem,"health_protocol":2,"pid":std::process::id(),"started":chrono::Utc::now().to_rfc3339()}))?;
    let _approval_worker = bot.as_ref().map(crate::safeguard::bot::ApprovalBot::start);
    let outcome = if let Some(tls) = tls {
        async {
            let service = axum_server::from_tcp_rustls(listener.into_std()?, tls)?;
            let handle = axum_server::Handle::new();
            let drain = handle.clone();
            let sessions = shutdown.clone();
            let _signal = ShutdownSignal::new(
                tokio::spawn(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    sessions.cancel();
                    drain.graceful_shutdown(Some(Duration::from_secs(15)));
                }),
                handle.clone(),
                shutdown,
            );
            service
                .handle(handle)
                .serve(router.into_make_service())
                .await
        }
        .await
    } else {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = tokio::signal::ctrl_c().await;
                shutdown.cancel();
            })
            .await
    };
    drop(sessions);
    let _ = state.update_json("http-service.json", Value::Null, |record| {
        if record["token"] == token {
            *record = Value::Null;
        }
        Ok(())
    });
    drop(lease);
    outcome?;
    Ok(())
}
pub(super) fn authenticated_router(
    router: Router,
    local: String,
    external: Option<String>,
) -> Router {
    router.layer(middleware::from_fn_with_state(
        ServiceAuthentication { local, external },
        authorize,
    ))
}

async fn authorize(
    AxumState(authentication): AxumState<ServiceAuthentication>,
    request: Request,
    next: Next,
) -> Response {
    // These routes authenticate their own expiring, narrowly scoped capability.
    // The service bearer is never placed in a URL or shared with file clients.
    if let Some((route, capability)) = request
        .uri()
        .path()
        .strip_prefix('/')
        .and_then(|path| path.split_once('/'))
        && matches!(route, "upload" | "media")
        && capability.len() == 64
        && capability.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return next.run(request).await;
    }
    if request.uri().path() == "/health"
        && request.headers().contains_key("x-telegram-health-nonce")
    {
        let nonce = request
            .headers()
            .get("x-telegram-health-nonce")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let proof = request
            .headers()
            .get("x-telegram-health-proof")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let target = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/")
            .to_owned();
        if nonce.len() != 64
            || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
            || verify_proof(
                health_signature(&authentication.local, b"request", &nonce, &target, 0, &[]),
                proof,
            )
            .is_err()
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if request.headers().contains_key(axum::http::header::ORIGIN) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let response = next.run(request).await;
        let (mut parts, body) = response.into_parts();
        let Ok(body) = axum::body::to_bytes(body, 65_536).await else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let proof = proof_hex(health_signature(
            &authentication.local,
            b"response",
            &nonce,
            &target,
            parts.status.as_u16(),
            &body,
        ));
        parts.headers.insert(
            "x-telegram-health-proof",
            proof.parse().expect("Hex proof is an HTTP header"),
        );
        return Response::from_parts(parts, axum::body::Body::from(body));
    }
    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let local: bool = provided
        .as_bytes()
        .ct_eq(authentication.local.as_bytes())
        .into();
    let external = authentication
        .external
        .as_ref()
        .is_some_and(|expected| bool::from(provided.as_bytes().ct_eq(expected.as_bytes())));
    if !local && !external {
        return (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, OWNER_BEARER_CHALLENGE)],
        )
            .into_response();
    }
    if request.headers().contains_key(axum::http::header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}
async fn health(
    AxumState(server): AxumState<Mcp>,
    Query(query): Query<HashMap<String, String>>,
) -> axum::Json<Value> {
    let Some(label) = query.get("account") else {
        return axum::Json(
            json!({"ready":true,"backend":"grammers","pid":std::process::id(),"accounts":crate::backend::accounts_snapshot().ok()}),
        );
    };
    let slot = server.runtime.accounts.lock().await.get(label).cloned();
    let Some(slot) = slot else {
        return axum::Json(
            json!({"status":"unknown","account":label,"reason":"Account is not owned by this service"}),
        );
    };
    let params =
        json!({"config_key":query.get("config_key"),"config_digest":query.get("config_digest")});
    let result = match tokio::time::timeout(
        Duration::from_secs(10),
        slot.api.invoke("local.session_health", params),
    )
    .await
    {
        Ok(Ok(mut result)) => {
            if result.get("status").and_then(Value::as_str).is_none() {
                result["status"] = json!(match result["authorized"].as_bool() {
                    Some(true) => "healthy",
                    Some(false) => "unauthorized",
                    None => "unknown",
                });
            }
            result
        }
        Ok(Err(_)) => json!({"status":"network","account":label}),
        Err(_) => json!({"status":"busy","account":label}),
    };
    axum::Json(result)
}
