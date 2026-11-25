//! Stdio clients share the HTTP owner's MTProto session instead of opening the
//! same authorization in several processes.
use super::*;
mod ownership;
#[cfg(test)]
#[path = "../../tests/unit/bridge_recovery.rs"]
mod recovery_tests;
mod startup;
#[cfg(test)]
#[path = "../../tests/unit/bridge.rs"]
mod tests;
use rmcp::{
    service::{RoleClient, RunningService, ServiceError},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
    },
};
use std::{
    error::Error as _,
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

const DISCOVERY: &str = "http-service.json";
const REQUEST_FAILED: &str =
    "Shared service request failed; inspect operation state before retrying a mutation";
const REQUEST_TIMED_OUT: &str =
    "Shared service request timed out; inspect operation state before retrying a mutation";

// Retain the whole record so a change to the trusted certificate also reconnects.
#[derive(PartialEq, Eq)]
struct DiscoveryIdentity(Value);

struct BridgeSession {
    remote: RunningService<RoleClient, ()>,
    identity: DiscoveryIdentity,
    tools: Vec<Tool>,
    instructions: Option<String>,
    stale: AtomicBool,
}

struct Bridge {
    state: Arc<State>,
    session: RwLock<Arc<BridgeSession>>,
    reconnect: Mutex<()>,
}

impl BridgeSession {
    async fn attach(state: &State) -> Result<Self> {
        let record = startup::ensure_service(state).await?;
        let (_, _, pid) = descriptor(&record, None)?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let identity = ownership::verified(pid, &executable)?;
        let remote = connect_record(&record, None).await?;
        let tools = tokio::time::timeout(Duration::from_secs(10), remote.list_all_tools())
            .await
            .context("Shared MCP tool discovery exceeded its time bound")??;
        anyhow::ensure!(
            record == state.read_json(DISCOVERY, Value::Null)?
                && identity == ownership::verified(pid, &executable)?,
            "Shared service ownership changed during initialization"
        );
        let instructions = remote
            .peer_info()
            .and_then(|owner| owner.instructions.clone());
        Ok(Self {
            remote,
            identity: DiscoveryIdentity(record),
            tools,
            instructions,
            stale: AtomicBool::new(false),
        })
    }
}

impl Bridge {
    fn new(state: Arc<State>, session: BridgeSession) -> Self {
        Self {
            state,
            session: RwLock::new(Arc::new(session)),
            reconnect: Mutex::new(()),
        }
    }

    fn cached(&self) -> Result<Arc<BridgeSession>> {
        self.session
            .read()
            .map(|value| Arc::clone(&value))
            .map_err(|_| anyhow::anyhow!("Shared MCP connection cache is unavailable"))
    }

    fn current(&self) -> Result<Option<Arc<BridgeSession>>> {
        let session = self.cached()?;
        let record = self.state.read_json(DISCOVERY, Value::Null)?;
        Ok((!session.stale.load(Ordering::Acquire)
            && !session.remote.is_closed()
            && session.identity.0 == record)
            .then_some(session))
    }

    async fn connection(&self) -> Result<Arc<BridgeSession>> {
        if let Some(session) = self.current()? {
            return Ok(session);
        }
        let _reconnect = self.reconnect.lock().await;
        if let Some(session) = self.current()? {
            return Ok(session);
        }
        let session = Arc::new(BridgeSession::attach(&self.state).await?);
        *self
            .session
            .write()
            .map_err(|_| anyhow::anyhow!("Shared MCP connection cache is unavailable"))? =
            Arc::clone(&session);
        Ok(session)
    }

    async fn forward(
        &self,
        request: CallToolRequestParams,
        deadline: tokio::time::Instant,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        let connect = || async {
            tokio::time::timeout_at(deadline, self.connection())
                .await
                .map_err(|_| request_error(REQUEST_TIMED_OUT))?
                .map_err(|_| request_error(REQUEST_FAILED))
        };
        let mut session = connect().await?;
        for attempt in 0..=1 {
            if tokio::time::Instant::now() >= deadline {
                return Err(request_error(REQUEST_TIMED_OUT));
            }
            match tokio::time::timeout_at(deadline, session.remote.call_tool_once(request.clone()))
                .await
            {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(ServiceError::McpError(error))) => return Err(error),
                Ok(Err(error)) => {
                    session.stale.store(true, Ordering::Release);
                    if attempt == 0 {
                        let stale_bearer = owner_bearer_rejected(&error);
                        let changed =
                            stale_bearer
                                && self.state.read_json(DISCOVERY, Value::Null).is_ok_and(
                                    |record| !record.is_null() && record != session.identity.0,
                                );
                        if changed || connection_refused(&error) {
                            let reconnected = connect().await?;
                            if !stale_bearer || reconnected.identity != session.identity {
                                session = reconnected;
                                continue;
                            }
                        }
                    }
                    return Err(request_error(
                        if matches!(error, ServiceError::Timeout { .. }) {
                            REQUEST_TIMED_OUT
                        } else {
                            REQUEST_FAILED
                        },
                    ));
                }
                Err(_) => {
                    session.stale.store(true, Ordering::Release);
                    return Err(request_error(REQUEST_TIMED_OUT));
                }
            }
        }
        Err(request_error(REQUEST_FAILED))
    }
}

fn request_error(message: &'static str) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(message, None)
}

fn owner_bearer_rejected(error: &ServiceError) -> bool {
    let ServiceError::TransportSend(error) = error else {
        return false;
    };
    matches!(
        error.error.downcast_ref::<StreamableHttpError<reqwest::Error>>(),
        Some(StreamableHttpError::AuthRequired(challenge))
            if challenge.www_authenticate_header == http::OWNER_BEARER_CHALLENGE
    )
}

fn connection_refused(error: &ServiceError) -> bool {
    let ServiceError::TransportSend(error) = error else {
        return false;
    };
    let Some(StreamableHttpError::Client(error)) = error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()
    else {
        return false;
    };
    error.is_connect()
        && std::iter::successors(error.source(), |&source| source.source()).any(|source| {
            source
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
        })
}
impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env());
        if let Ok(session) = self.cached() {
            info.instructions = session.instructions.clone();
        }
        info
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
        let session = self.cached().map_err(|_| request_error(REQUEST_FAILED))?;
        Ok(ListToolsResult {
            tools: session.tools.clone(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        self.forward(
            request,
            tokio::time::Instant::now() + Duration::from_secs(130),
        )
        .await
    }
}
pub async fn connect_shared() -> Result<RunningService<RoleClient, ()>> {
    connect_shared_at(None).await
}
pub async fn connect_shared_at(requested: Option<&str>) -> Result<RunningService<RoleClient, ()>> {
    let state = State::global()?;
    let record = state.read_json(DISCOVERY, Value::Null)?;
    connect_record(&record, requested).await
}

async fn connect_record(
    record: &Value,
    requested: Option<&str>,
) -> Result<RunningService<RoleClient, ()>> {
    let (parsed, token, pid) = descriptor(record, requested)?;
    let mut health_url = parsed.clone();
    health_url.set_path("/health");
    let health = crate::account_manager::authenticated_health_request(health_url, record).await?;
    anyhow::ensure!(
        health["ready"] == true
            && health["backend"] == "grammers"
            && health["pid"].as_u64() == Some(u64::from(pid)),
        "Shared service readiness identity does not match its private discovery record"
    );
    let client = super::local_http_client(record["certificate_pem"].as_str())?;
    let transport = StreamableHttpClientTransport::with_client(
        client,
        client_configuration(&parsed.to_string(), token),
    );
    let remote = tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .context("Shared MCP initialization exceeded its time bound")?
        .context("Cannot connect to the shared Telegram MCP service")?;
    Ok(remote)
}
fn client_configuration(uri: &str, token: &str) -> StreamableHttpClientTransportConfig {
    let mut config = StreamableHttpClientTransportConfig::with_uri(uri).auth_header(token);
    // The owner rejects unknown sessions before tools/call dispatch.
    config.reinit_on_expired_session = true;
    config.max_sse_event_size = 64 * 1024 * 1024;
    config
}

fn descriptor<'a>(record: &'a Value, requested: Option<&str>) -> Result<(url::Url, &'a str, u32)> {
    let url = record["url"]
        .as_str()
        .context("No shared service is running; start telegram-mcp serve --http 127.0.0.1:8765")?;
    let mut parsed = url::Url::parse(url).context("Invalid shared service endpoint")?;
    let explicit_port = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split('/').next())
        .and_then(|authority| authority.rsplit_once(':'))
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .is_some_and(|port| port > 0);
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain("localhost")) => true,
        _ => false,
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || !loopback
        || !explicit_port
        || parsed.path() != "/mcp"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        bail!(
            "Shared service discovery must use a loopback HTTP or HTTPS MCP endpoint with an explicit port"
        );
    }
    if let Some(requested) = requested {
        let mut requested = url::Url::parse(requested).context("Invalid requested MCP endpoint")?;
        if requested.host_str() == Some("localhost") {
            requested.set_host(Some("127.0.0.1"))?;
        }
        let mut identity = parsed.clone();
        if identity.host_str() == Some("localhost") {
            identity.set_host(Some("127.0.0.1"))?;
        }
        if requested != identity {
            bail!("The requested endpoint differs from the authenticated shared service discovery");
        }
    }
    if parsed.scheme() == "http" && parsed.host_str() == Some("localhost") {
        parsed.set_host(Some("127.0.0.1"))?;
    }
    let token = record["token"]
        .as_str()
        .context("Shared service discovery has no token")?;
    anyhow::ensure!(
        (32..=4096).contains(&token.len()) && token.bytes().all(|byte| (33..=126).contains(&byte)),
        "Shared service discovery has an invalid token"
    );
    let pid = record["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .context("Shared service discovery has no process identity")?;
    Ok((parsed, token, pid))
}
pub async fn run() -> Result<()> {
    let state = State::global()?;
    let session = BridgeSession::attach(&state).await?;
    let bridge = Bridge::new(state, session);
    let service = bridge.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
