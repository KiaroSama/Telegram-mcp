//! Only the explicit bridge command starts a daemon. Shared API clients attach.
use super::{descriptor, ownership};
use crate::state::State;
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde_json::Value;
use std::{
    fs::File,
    net::SocketAddr,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

const START_LIMIT: Duration = Duration::from_secs(30);

fn lease(path: &Path, create: bool) -> Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    crate::state::storage::restrict(path, false)?;
    Ok(file)
}

fn occupied(file: &File) -> Result<bool> {
    match FileExt::try_lock_exclusive(file) {
        Ok(()) => {
            FileExt::unlock(file)?;
            Ok(false)
        }
        Err(error) if contended(&error) => Ok(true),
        Err(error) => Err(error.into()),
    }
}
fn contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn endpoint() -> Result<SocketAddr> {
    let host = std::env::var("MCP_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let host = if host == "localhost" {
        "127.0.0.1"
    } else {
        &host
    };
    let host: std::net::IpAddr = host
        .parse()
        .context("Bridge MCP_HOST must be a loopback IP address")?;
    ensure!(
        host.is_loopback(),
        "Bridge MCP_HOST must be a loopback IP address"
    );
    let port = std::env::var("MCP_PORT").unwrap_or_else(|_| "8765".into());
    ensure!(
        !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()),
        "Bridge MCP_PORT must be an integer from 1 to 65535"
    );
    let port: u16 = port
        .parse()
        .context("Bridge MCP_PORT must be an integer from 1 to 65535")?;
    ensure!(
        port > 0,
        "Bridge MCP_PORT must be an integer from 1 to 65535"
    );
    Ok(SocketAddr::new(host, port))
}

async fn live(
    state: &State,
    executable: &Path,
    expected_pid: Option<u32>,
) -> Result<Option<Value>> {
    let path = state.root().join("http-service.lock");
    if !path.exists() {
        return Ok(None);
    }
    let owner = lease(&path, false)?;
    if !occupied(&owner)? {
        return Ok(None);
    }
    let record = state.read_json("http-service.json", Value::Null)?;
    let (url, token, pid) = descriptor(&record, None)?;
    if let Some(expected) = expected_pid {
        ensure!(
            pid == expected,
            "Shared service publication belongs to another launch"
        );
    }
    let identity = ownership::verified(pid, executable)?;
    let mut health_url = url.clone();
    health_url.set_path("/health");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(2))
        .build()?;
    let mut response = client
        .get(health_url)
        .bearer_auth(token)
        .send()
        .await
        .context("The owned shared service is not ready; it was not replaced")?;
    ensure!(
        response.status().is_success(),
        "The owned shared service refused readiness; it was not replaced"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= 64 * 1024),
        "Shared service readiness response exceeds its byte bound"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 64 * 1024,
            "Shared service readiness response exceeds its byte bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    let health: Value =
        serde_json::from_slice(&bytes).context("Invalid shared service readiness response")?;
    ensure!(
        health["ready"] == true
            && health["backend"] == "grammers"
            && health["pid"].as_u64() == Some(u64::from(pid)),
        "Shared service readiness identity does not match its private discovery record"
    );
    ensure!(
        record == state.read_json("http-service.json", Value::Null)?
            && identity == ownership::verified(pid, executable)?
            && occupied(&owner)?,
        "Shared service ownership changed during attachment"
    );
    Ok(Some(record))
}

struct Launch(Option<Child>);
impl Launch {
    fn start(executable: &Path, address: SocketAddr, state: &State) -> Result<Self> {
        let config = crate::account_manager::current_config()?;
        let mut command = Command::new(executable);
        command
            .arg("--config")
            .arg(config.path)
            .args(["serve", "--http"])
            .arg(address.to_string())
            .env("TELEGRAM_STATE_DIR", state.root())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid is async-signal-safe, does not allocate, and runs
            // only in the child immediately before exec.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000 | 0x00000200);
        }
        Ok(Self(Some(command.spawn().context(
            "The native shared service could not be started",
        )?)))
    }
    fn pid(&self) -> u32 {
        self.0.as_ref().expect("Owned launch is live").id()
    }
    fn exited(&mut self) -> Result<bool> {
        Ok(self
            .0
            .as_mut()
            .context("Owned launch was already released")?
            .try_wait()?
            .is_some())
    }
    fn detach(mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = std::thread::Builder::new()
                .name("shared-service-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
    async fn cleanup(&mut self) -> Result<()> {
        let Some(child) = self.0.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_none() {
            child
                .kill()
                .context("Failed shared startup cleanup is unconfirmed")?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while child.try_wait()?.is_none() {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "Failed shared startup cleanup is unconfirmed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.0.take();
        Ok(())
    }
}
impl Drop for Launch {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = std::thread::Builder::new()
                .name("failed-start-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

pub(super) async fn ensure_service(state: &State) -> Result<Value> {
    let address = endpoint()?;
    let executable = std::env::current_exe()?.canonicalize()?;
    if let Some(record) = live(state, &executable, None).await? {
        return Ok(record);
    }
    let start = lease(&state.root().join("bridge-start.lock"), true)?;
    let deadline = tokio::time::Instant::now() + START_LIMIT;
    loop {
        match FileExt::try_lock_exclusive(&start) {
            Ok(()) => break,
            Err(error) if contended(&error) => {
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "Another bridge is starting the shared service; retry attachment later"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(record) = live(state, &executable, None).await? {
        return Ok(record);
    }
    // A failed loopback connect can take seconds on Windows. Ask the local
    // kernel whether this exact listen address is free, without contacting an
    // unknown listener. The startup lease serializes our own launches, and the
    // child still performs the authoritative bind after this handle is closed.
    match std::net::TcpListener::bind(address) {
        Ok(probe) => drop(probe),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            bail!("An unverified listener occupies the configured bridge port; it was not replaced")
        }
        _ => bail!(
            "The configured bridge port cannot be verified as available; no service was started"
        ),
    }
    let mut launch = Launch::start(&executable, address, state)?;
    let result = async {
        loop {
            if launch.exited()? {
                bail!("Shared service startup failed; inspect its private log");
            }
            if let Ok(Some(record)) = live(state, &executable, Some(launch.pid())).await {
                return Ok(record);
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "Shared service readiness timed out; no replacement was attempted"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    .await;
    match result {
        Ok(record) => {
            launch.detach();
            Ok(record)
        }
        Err(error) => {
            launch.cleanup().await?;
            Err(error)
        }
    }
}
