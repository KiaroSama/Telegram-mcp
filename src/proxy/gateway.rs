use super::*;
use anyhow::{bail, ensure};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
};

pub(super) async fn start(
    state: Arc<State>,
    snapshot: &ConfigSnapshot,
    label: String,
    status: Arc<Mutex<Value>>,
) -> Result<Route> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("Cannot bind the private proxy gateway")?;
    let address = listener.local_addr()?;
    let username = format!("{:032x}", rand::random::<u128>());
    let password = format!("{:032x}", rand::random::<u128>());
    let proxy_url = Some(format!("socks5://{username}:{password}@{address}"));
    *status
        .lock()
        .map_err(|_| anyhow::anyhow!("Proxy route status is unavailable"))? = json!({"route":"pending","proxy":null,"gateway":true,"account":label,"verification":"not_probed"});
    let context = Arc::new(ContextState {
        state,
        label,
        snapshot: ConfigSnapshot {
            values: snapshot.values.clone(),
            digest: snapshot.digest.clone(),
            path: snapshot.path.clone(),
            process_keys: snapshot.process_keys.clone(),
        },
        status: Arc::clone(&status),
        failed_until: Mutex::new(None),
    });
    let task = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        let permits = Arc::new(Semaphore::new(16));
        let authentication = Arc::new(Semaphore::new(64));
        loop {
            tokio::select! {
                accepted=accept_with(|| listener.accept())=>{
                    let (socket,peer) = accepted;
                    if !peer.ip().is_loopback(){continue}
                    let Ok(authentication)=Arc::clone(&authentication).try_acquire_owned()else{continue};
                    let context=Arc::clone(&context);let username=username.clone();let password=password.clone();let permits=Arc::clone(&permits);
                    tasks.spawn(async move{let _=serve(socket,context,&username,&password,authentication,permits).await;});
                },
                Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
            }
        }
    });
    Ok(Route {
        proxy_url,
        status,
        task: Some(task),
    })
}
// Socket accept errors can be temporary (aborted handshakes or descriptor pressure).
// The listener is owned for the route lifetime; cancellation is handled by Route::drop.
async fn accept_with<F, Fut>(mut accept: F) -> (TcpStream, SocketAddr)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<(TcpStream, SocketAddr)>>,
{
    loop {
        match accept().await {
            Ok(connection) => return connection,
            Err(_) => {
                tracing::warn!(
                    operation = "proxy_accept",
                    "Private gateway accept failed; retrying"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}
async fn authenticate(
    socket: &mut TcpStream,
    username: &str,
    password: &str,
) -> Result<SocketAddr> {
    let mut hello = [0; 2];
    socket.read_exact(&mut hello).await?;
    ensure!(
        hello[0] == 5 && hello[1] > 0,
        "Invalid private SOCKS5 greeting"
    );
    let mut methods = vec![0; hello[1] as usize];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&2) {
        socket.write_all(&[5, 255]).await?;
        bail!("The private proxy gateway requires authentication")
    }
    socket.write_all(&[5, 2]).await?;
    let version = socket.read_u8().await?;
    ensure!(
        version == 1,
        "Invalid private SOCKS5 authentication version"
    );
    let mut user = vec![0; socket.read_u8().await? as usize];
    socket.read_exact(&mut user).await?;
    let mut pass = vec![0; socket.read_u8().await? as usize];
    socket.read_exact(&mut pass).await?;
    if !equal(&user, username.as_bytes()) || !equal(&pass, password.as_bytes()) {
        socket.write_all(&[1, 1]).await?;
        bail!("Private proxy gateway authentication failed")
    }
    socket.write_all(&[1, 0]).await?;
    let mut request = [0; 4];
    socket.read_exact(&mut request).await?;
    ensure!(
        request[0] == 5 && request[1] == 1 && request[2] == 0,
        "Only SOCKS5 CONNECT is supported by the private gateway"
    );
    let ip = match request[3] {
        1 => {
            let mut ip = [0; 4];
            socket.read_exact(&mut ip).await?;
            IpAddr::V4(Ipv4Addr::from(ip))
        }
        4 => {
            let mut ip = [0; 16];
            socket.read_exact(&mut ip).await?;
            IpAddr::V6(Ipv6Addr::from(ip))
        }
        _ => bail!("The native sender must provide a numerical datacenter address"),
    };
    let port = socket.read_u16().await?;
    ensure!(port > 0, "Datacenter port must be positive");
    Ok(SocketAddr::new(ip, port))
}
async fn serve(
    mut socket: TcpStream,
    context: Arc<ContextState>,
    user: &str,
    password: &str,
    authentication: tokio::sync::OwnedSemaphorePermit,
    permits: Arc<Semaphore>,
) -> Result<()> {
    let target = tokio::time::timeout(
        Duration::from_secs(5),
        authenticate(&mut socket, user, password),
    )
    .await
    .context("Private proxy authentication timed out")??;
    drop(authentication);
    let _permit = permits
        .try_acquire_owned()
        .context("Private proxy gateway has 16 authenticated tunnels already")?;
    let result = select(&context, target).await;
    let mut upstream = match result {
        Ok(stream) => stream,
        Err(error) => {
            socket.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(error);
        }
    };
    socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    // Once this acknowledgement is sent, an owner request may be in flight.
    // No failure below is retried on another route.
    tokio::io::copy_bidirectional(&mut socket, &mut upstream)
        .await
        .context("Proxy tunnel closed")?;
    Ok(())
}
async fn select(context: &Arc<ContextState>, target: SocketAddr) -> Result<tunnel::Stream> {
    if context
        .failed_until
        .lock()
        .map_err(|_| anyhow::anyhow!("Proxy route state is unavailable"))?
        .is_some_and(|until| until > Instant::now())
    {
        bail!("Every proxy route failed recently; retry after the 30 second backoff")
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let choices = super::candidates(&context.state, &context.snapshot, &context.label)?;
    for candidate in &choices {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let started = Instant::now();
        let attempt = async {
            let dc = if candidate
                .proxy
                .as_ref()
                .is_some_and(|proxy| proxy.kind == "mtproto")
            {
                datacenter(&context.state, &context.label, target)?
            } else {
                2
            };
            let mut probe = tunnel::connect(candidate.proxy.as_ref(), target, dc).await?;
            probe_stream(&mut probe).await?;
            drop(probe);
            tunnel::connect(candidate.proxy.as_ref(), target, dc).await
        };
        let budget = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .min(Duration::from_secs(10));
        match tokio::time::timeout(budget, attempt).await {
            Ok(Ok(stream)) => {
                let context = Arc::clone(context);
                let candidate = candidate.clone();
                let latency = started.elapsed().as_millis() as u64;
                // Disk telemetry cannot discard a working connection or block this Tokio worker.
                let _ = tokio::task::spawn_blocking(move || {
                    context.record(&candidate, latency, target)
                })
                .await;
                return Ok(stream);
            }
            _ => {
                let context = Arc::clone(context);
                let candidate = candidate.clone();
                let _ =
                    tokio::task::spawn_blocking(move || context.record_failure(&candidate)).await;
            }
        }
    }
    *context
        .failed_until
        .lock()
        .map_err(|_| anyhow::anyhow!("Proxy route state is unavailable"))? =
        Some(Instant::now() + Duration::from_secs(30));
    bail!(
        "No configured proxy, direct connection or pool route reached Telegram within the connection budget"
    )
}
fn datacenter(state: &State, label: &str, target: SocketAddr) -> Result<i16> {
    let value = state.read_json(&format!("{label}.session.json"), Value::Null)?;
    if let Some(options) = value["dc_options"].as_array() {
        for option in options {
            if ["ipv4", "ipv6"].iter().any(|key| {
                option[*key]
                    .as_str()
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                    == Some(target)
            }) {
                let dc = option["id"].as_i64().context("Datacenter id is invalid")?;
                return i16::try_from(dc).context("Datacenter id exceeds MTProxy range");
            }
        }
    }
    for option in grammers_session::SessionData::default().dc_options.values() {
        if SocketAddr::V4(option.ipv4) == target || SocketAddr::V6(option.ipv6) == target {
            return i16::try_from(option.id).context("Datacenter id exceeds MTProxy range");
        }
    }
    bail!("MTProxy destination is not a known session datacenter; no DC id was guessed")
}

pub(crate) async fn probe_stream(stream: &mut tunnel::Stream) -> Result<()> {
    let nonce = rand::random::<[u8; 16]>();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("System clock predates the Unix epoch")?;
    let id = (now.as_secs() << 32) | (((u64::from(now.subsec_nanos()) << 32) / 1_000_000_000) & !3);
    let mut body = Vec::new();
    body.extend(0xbe7e8ef1u32.to_le_bytes());
    body.extend(nonce);
    let mut message = vec![0u8; 8];
    message.extend(id.to_le_bytes());
    message.extend((body.len() as i32).to_le_bytes());
    message.extend(body);
    stream.write_all(&mtproxy::full_frame(&message, 0)?).await?;
    let response = mtproxy::read_full(stream, 0).await?;
    ensure!(
        response.len() >= 40 && response[..8] == [0; 8],
        "Proxy did not return an unauthenticated Telegram response"
    );
    let length = i32::from_le_bytes(response[16..20].try_into()?);
    ensure!(
        length >= 20 && length as usize == response.len() - 20,
        "Proxy returned an invalid Telegram response length"
    );
    ensure!(
        response[20..24] == 0x05162463u32.to_le_bytes() && equal(&response[24..40], &nonce),
        "Proxy response is not the req_pq nonce's resPQ"
    );
    validate_res_pq(&response[20..20 + length as usize])?;
    Ok(())
}

fn validate_res_pq(body: &[u8]) -> Result<()> {
    ensure!(
        body.len() >= 48 && body.len() <= 4096,
        "Proxy resPQ response is truncated or oversized"
    );
    let (header, length) = if body[36] == 254 {
        ensure!(body.len() >= 40, "Invalid resPQ byte length");
        (
            4,
            usize::from(body[37]) | (usize::from(body[38]) << 8) | (usize::from(body[39]) << 16),
        )
    } else {
        ensure!(body[36] < 254, "Invalid resPQ byte length");
        (1, usize::from(body[36]))
    };
    ensure!(
        (1..=512).contains(&length),
        "Proxy resPQ factorization payload has an invalid size"
    );
    let vector = 36 + (header + length).div_ceil(4) * 4;
    ensure!(
        body.len() >= vector + 8,
        "Proxy resPQ fingerprint vector is missing"
    );
    ensure!(
        body[vector..vector + 4] == 0x1cb5c415u32.to_le_bytes(),
        "Proxy resPQ fingerprint constructor is invalid"
    );
    let count = i32::from_le_bytes(body[vector + 4..vector + 8].try_into()?);
    ensure!(
        (1..=64).contains(&count) && body.len() == vector + 8 + count as usize * 8,
        "Proxy resPQ fingerprint vector has an invalid length"
    );
    Ok(())
}
#[cfg(test)]
#[path = "../../tests/unit/proxy_gateway.rs"]
mod tests;
