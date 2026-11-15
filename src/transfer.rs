//! Short-lived, scoped capabilities for remote clients to transfer files.
//! An upload URL authorizes one new file, never a directory or replacement.
mod files;
pub mod http;
use crate::state::{FileAccess, State};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
const TTL: Duration = Duration::from_secs(3600);
const MAX_PENDING: usize = 64;
#[derive(Clone)]
pub(crate) enum Ticket {
    Upload {
        path: PathBuf,
        max_bytes: u64,
    },
    Download {
        path: PathBuf,
        length: u64,
        modified: std::time::SystemTime,
        identity: files::Identity,
    },
}
struct Entry {
    ticket: Ticket,
    expires: Instant,
}
pub struct Transfers {
    pub(crate) state: Arc<State>,
    pending: Mutex<HashMap<String, Entry>>,
}
impl Transfers {
    pub fn new(state: Arc<State>) -> Self {
        Self {
            state,
            pending: Mutex::new(HashMap::new()),
        }
    }
    fn issue(&self, ticket: Ticket, base: &str, route: &str) -> Result<String> {
        let base = validate_base(base)?;
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Transfer registry is unavailable"))?;
        pending.retain(|_, entry| entry.expires > Instant::now());
        ensure!(
            pending.len() < MAX_PENDING,
            "Too many outstanding file transfers; wait for earlier URLs to expire"
        );
        let token = (0..32)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect::<String>();
        pending.insert(
            token.clone(),
            Entry {
                ticket,
                expires: Instant::now() + TTL,
            },
        );
        Ok(format!("{}/{route}/{token}", base.trim_end_matches('/')))
    }
    pub fn prepare(&self, filename: &str, base: &str, max_bytes: u64) -> Result<Value> {
        validate_filename(filename)?;
        ensure!(
            (1..=4 * 1024 * 1024 * 1024).contains(&max_bytes),
            "Upload budget must be within 1 byte and 4 GiB"
        );
        let root = self
            .state
            .file_roots()
            .first()
            .context("Configure a server file root before uploading")?;
        let folder = root.join("uploads");
        self.state.resolve_file(&folder, FileAccess::Write)?;
        crate::state::storage::private_directory(&folder)?;
        let path = self
            .state
            .resolve_file(&folder.join(filename), FileAccess::Write)?;
        ensure!(
            !path.exists(),
            "The upload destination already exists; choose a different filename"
        );
        let url = self.issue(
            Ticket::Upload {
                path: path.clone(),
                max_bytes,
            },
            base,
            "upload",
        )?;
        Ok(
            json!({"url":url,"method":"PUT","file_path":path,"max_bytes":max_bytes,"expires_in_seconds":TTL.as_secs(),"single_use":true,"url_is_temporary_credential":true}),
        )
    }
    pub fn download_link(&self, path: &Path, base: &str) -> Result<Value> {
        let path = self.state.resolve_file(path, FileAccess::Read)?;
        let file = files::open_download(&self.state, &path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file(),
            "A download link must identify a regular file"
        );
        let url = self.issue(
            Ticket::Download {
                path,
                length: metadata.len(),
                modified: metadata.modified()?,
                identity: files::identity(&file)?,
            },
            base,
            "media",
        )?;
        Ok(json!({"url":url,"expires_in_seconds":TTL.as_secs(),"url_is_temporary_credential":true}))
    }
    /// Adding a remote link is best-effort: a completed local file save remains
    /// successful when the HTTP registry is full or the path cannot be shared.
    pub fn attach_download(&self, result: &mut Value, base: &str) {
        if result.get("download_url").is_some() {
            return;
        }
        let Some(path) = result.get("path").and_then(Value::as_str) else {
            return;
        };
        let Ok(link) = self.download_link(Path::new(path), base) else {
            return;
        };
        result["download_url"] = link["url"].clone();
        result["download_expires_in_seconds"] = link["expires_in_seconds"].clone();
        result["download_url_is_temporary_credential"] = json!(true);
    }
    pub(crate) fn take_upload(&self, token: &str) -> Result<Ticket> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Transfer registry is unavailable"))?;
        let entry = pending
            .get(token)
            .filter(|entry| entry.expires > Instant::now())
            .context("Upload capability is invalid or expired")?;
        ensure!(
            matches!(entry.ticket, Ticket::Upload { .. }),
            "Capability is not an upload"
        );
        Ok(pending
            .remove(token)
            .expect("Checked transfer entry exists")
            .ticket)
    }
    pub(crate) fn find_download(&self, token: &str) -> Result<Ticket> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Transfer registry is unavailable"))?;
        let entry = pending
            .get(token)
            .filter(|entry| entry.expires > Instant::now())
            .context("Download capability is invalid or expired")?;
        ensure!(
            matches!(entry.ticket, Ticket::Download { .. }),
            "Capability is not a download"
        );
        Ok(entry.ticket.clone())
    }
}
pub fn validate_filename(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 180 && name == name.trim_matches([' ', '.']),
        "Upload filename is empty, too long or ends in a dot/space"
    );
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "Upload filename must be a plain portable filename"
    );
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    let device_port = port.is_some_and(|port| {
        matches!(
            port,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    });
    ensure!(
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) && !device_port,
        "Upload filename is reserved by Windows"
    );
    Ok(())
}
pub fn validate_base(base: &str) -> Result<String> {
    let parsed = url::Url::parse(base).context("Invalid public media base URL")?;
    let local = match parsed.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain("localhost")) => true,
        _ => false,
    };
    ensure!(
        parsed.scheme() == "https" || parsed.scheme() == "http" && local,
        "Public media URLs require HTTPS; loopback HTTP is allowed"
    );
    ensure!(
        parsed.host().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "Media base URL must have a host and no embedded credentials, query or fragment"
    );
    Ok(parsed.to_string())
}
pub fn global() -> Result<Arc<Transfers>> {
    static REGISTRY: OnceLock<Arc<Transfers>> = OnceLock::new();
    if let Some(registry) = REGISTRY.get() {
        return Ok(Arc::clone(registry));
    }
    let _ = REGISTRY.set(Arc::new(Transfers::new(State::global()?)));
    Ok(Arc::clone(
        REGISTRY.get().expect("Transfer registry initialized"),
    ))
}
pub fn base_url(state: &State) -> Result<String> {
    let discovery = state.read_json("http-service.json", Value::Null)?;
    ensure!(
        discovery["url"].is_string(),
        "File transfer URLs require the running HTTP service"
    );
    ensure!(
        discovery["pid"].as_u64() == Some(u64::from(std::process::id())),
        "The HTTP discovery record belongs to another process; start HTTP mode to issue transfer URLs"
    );
    if let Some(base) = crate::account_manager::current_config()?
        .values
        .get("TELEGRAM_MEDIA_BASE_URL")
        .filter(|v| !v.is_empty())
    {
        return validate_base(base);
    }
    let mut url = url::Url::parse(discovery["url"].as_str().expect("URL was checked"))?;
    url.set_path("/");
    url.set_query(None);
    validate_base(url.as_str())
}
pub fn prepare(args: &Value) -> Result<Value> {
    let registry = global()?;
    let base = base_url(&registry.state)?;
    let budget = crate::account_manager::current_config()?
        .values
        .get("TELEGRAM_UPLOAD_MAX_BYTES")
        .map(|v| v.parse::<u64>())
        .transpose()
        .context("Invalid TELEGRAM_UPLOAD_MAX_BYTES")?
        .unwrap_or(100 * 1024 * 1024);
    registry.prepare(crate::args::text(args, "filename")?, &base, budget)
}
/// HTTP discovery must belong to this exact running process. Stdio clients
/// cannot accidentally publish capabilities for a stale/foreign service.
pub fn attach_download(result: &mut Value) {
    let Ok(registry) = global() else {
        return;
    };
    let Ok(base) = base_url(&registry.state) else {
        return;
    };
    registry.attach_download(result, &base);
}
