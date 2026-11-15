use super::*;
use axum::{
    Router,
    extract::{Query, Request, State as AxumState},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use axum_server::tls_rustls::RustlsConfig;
use fs2::FileExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use sha2::{Digest, Sha256};

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
) -> Result<Option<RustlsConfig>> {
    match (cert, key) {
        (None, None) => {
            if !address.ip().is_loopback() {
                bail!(
                    "A non-loopback HTTP service requires TLS: configure TELEGRAM_HTTP_TLS_CERT and TELEGRAM_HTTP_TLS_KEY"
                );
            }
            Ok(None)
        }
        (Some(cert), Some(key)) => RustlsConfig::from_pem_file(cert, key)
            .await
            .map(Some)
            .map_err(|_| {
                anyhow::anyhow!(
                    "TLS certificate/key could not be read or are malformed or mismatched"
                )
            }),
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
    let token = configured.unwrap_or_else(|| {
        (0..32)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect()
    });
    if token.len() < 32 {
        bail!("TELEGRAM_HTTP_TOKEN must contain at least 32 characters");
    }
    // Validate the complete matching pair before binding a listener or
    // publishing discovery. No TLS failure falls back to plaintext.
    let tls = tls_configuration(
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
    let factory = server.clone();
    let mut config = StreamableHttpServerConfig::default()
        .enforce_origin_validation()
        .with_cancellation_token(shutdown.child_token());
    if let Ok(hosts) = std::env::var("TELEGRAM_HTTP_ALLOWED_HOSTS") {
        config =
            config.with_allowed_hosts(hosts.split(',').map(str::trim).filter(|s| !s.is_empty()));
    }
    let service: StreamableHttpService<Mcp, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(factory.clone()), Default::default(), config);
    let router = Router::new()
        .nest_service("/mcp", service)
        .route("/health", axum::routing::get(health))
        .with_state(server)
        .merge(crate::transfer::http::router(crate::transfer::global()?))
        .layer(middleware::from_fn_with_state(token.clone(), authorize));
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
    state.write_json("http-service.json",&json!({"url":endpoint,"token":token,"pid":std::process::id(),"started":chrono::Utc::now().to_rfc3339()}))?;
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
async fn authorize(AxumState(token): AxumState<String>, request: Request, next: Next) -> Response {
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
    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let actual = Sha256::digest(provided.as_bytes());
    let expected = Sha256::digest(token.as_bytes());
    let mismatch = actual
        .iter()
        .zip(expected.iter())
        .fold(0u8, |n, (a, b)| n | (a ^ b));
    if mismatch != 0 {
        return StatusCode::UNAUTHORIZED.into_response();
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
