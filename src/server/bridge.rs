//! Stdio clients share the HTTP owner's MTProto session instead of opening the
//! same authorization in several processes.
use super::*;
mod ownership;
mod startup;
#[cfg(test)]
#[path = "../../tests/unit/bridge.rs"]
mod tests;
use rmcp::{
    service::{RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
#[derive(Clone)]
struct Bridge {
    remote: Arc<RunningService<RoleClient, ()>>,
    tools: Arc<Vec<Tool>>,
}
impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env());
        if let Some(owner) = self.remote.peer_info() {
            info.instructions = owner.instructions.clone();
        }
        info
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
        tokio::time::timeout(Duration::from_secs(130),self.remote.call_tool_once(request)).await
            .map_err(|_|rmcp::ErrorData::internal_error("Shared service request timed out; inspect operation state before retrying a mutation",None))?
            .map_err(|_|rmcp::ErrorData::internal_error("Shared service request failed; inspect operation state before retrying a mutation",None))
    }
}
pub async fn connect_shared() -> Result<RunningService<RoleClient, ()>> {
    connect_shared_at(None).await
}
pub async fn connect_shared_at(requested: Option<&str>) -> Result<RunningService<RoleClient, ()>> {
    let state = State::global()?;
    let record = state.read_json("http-service.json", Value::Null)?;
    let (parsed, token, pid) = descriptor(&record, requested)?;
    let mut health_url = parsed.clone();
    health_url.set_path("/health");
    let health = crate::account_manager::authenticated_health_request(health_url, &record).await?;
    anyhow::ensure!(
        health["ready"] == true
            && health["backend"] == "grammers"
            && health["pid"].as_u64() == Some(u64::from(pid)),
        "Shared service readiness identity does not match its private discovery record"
    );
    let client = super::local_http_client(record["certificate_pem"].as_str())?;
    let mut configuration =
        StreamableHttpClientTransportConfig::with_uri(parsed.to_string()).auth_header(token);
    configuration.reinit_on_expired_session = false;
    // The SDK bounds ordinary JSON bodies internally; streamed media responses
    // use the explicit per-event limit below.
    configuration.max_sse_event_size = 64 * 1024 * 1024;
    let transport = StreamableHttpClientTransport::with_client(client, configuration);
    let remote = tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .context("Shared MCP initialization exceeded its time bound")?
        .context("Cannot connect to the shared Telegram MCP service")?;
    Ok(remote)
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
    let record = startup::ensure_service(&state).await?;
    let (_, _, pid) = descriptor(&record, None)?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let identity = ownership::verified(pid, &executable)?;
    let remote = connect_shared().await?;
    if record != state.read_json("http-service.json", Value::Null)?
        || identity != ownership::verified(pid, &executable)?
    {
        let _ = remote.cancel().await;
        bail!("Shared service ownership changed during initialization");
    }
    let tools = remote.list_all_tools().await?;
    let bridge = Bridge {
        remote: Arc::new(remote),
        tools: Arc::new(tools),
    };
    let service = bridge.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
