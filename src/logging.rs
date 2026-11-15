//! Private, size-bounded runtime logging. Only application events are enabled;
//! dependency protocol traces and tool arguments are never written to this log.
use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tracing_subscriber::fmt::MakeWriter;
#[cfg(test)]
#[path = "../tests/unit/logging.rs"]
mod tests;
struct Destination {
    path: PathBuf,
    file: Option<File>,
    bytes: u64,
    max: u64,
}
#[derive(Clone)]
struct Writer(Arc<Mutex<Destination>>);
impl<'a> MakeWriter<'a> for Writer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut target = self
            .0
            .lock()
            .map_err(|_| io::Error::other("Logging lock unavailable"))?;
        let text = std::str::from_utf8(bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "Runtime log event is not UTF-8")
        })?;
        let mut offset = 0;
        while offset < bytes.len() {
            let remaining = target.max.saturating_sub(target.bytes);
            let mut count = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(bytes.len() - offset);
            while count > 0 && !text.is_char_boundary(offset + count) {
                count -= 1;
            }
            if count == 0 {
                if target.bytes == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Log byte budget cannot contain one UTF-8 character",
                    ));
                }
                target.rotate()?;
                continue;
            }
            target
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("Runtime log is closed"))?
                .write_all(&bytes[offset..offset + count])?;
            target.bytes += count as u64;
            offset += count;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("Logging lock unavailable"))?
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("Runtime log is closed"))?
            .flush()
    }
}
impl Destination {
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
            drop(file);
        }
        for n in (1..3).rev() {
            let source = PathBuf::from(format!("{}.{}", self.path.display(), n));
            let destination = PathBuf::from(format!("{}.{}", self.path.display(), n + 1));
            if source.is_file() {
                if destination.exists() {
                    std::fs::remove_file(&destination)?;
                }
                std::fs::rename(source, destination)?;
            }
        }
        let previous = PathBuf::from(format!("{}.1", self.path.display()));
        if previous.exists() {
            std::fs::remove_file(&previous)?;
        }
        std::fs::rename(&self.path, previous)?;
        self.file = Some(private_open(&self.path)?);
        self.bytes = 0;
        Ok(())
    }
}
fn private_open(path: &std::path::Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    crate::state::storage::restrict(path, false)
        .map_err(|_| io::Error::other("Cannot restrict runtime log permissions"))?;
    Ok(file)
}
pub fn initialize() -> Result<()> {
    if std::env::var_os("TELEGRAM_MCP_LOG_FILE").is_some_and(|value| value.is_empty()) {
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter("off,telegram_mcp=info,operator_plugin=info")
            .with_writer(std::io::stderr)
            .try_init()
            .map_err(|_| anyhow::anyhow!("Runtime logging could not be initialized"))?;
        return Ok(());
    }
    let state = crate::state::State::global()?;
    let path = std::env::var_os("TELEGRAM_MCP_LOG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| state.root().join("mcp.log"));
    if !path.starts_with(state.root()) {
        bail!("Runtime log must remain in private state");
    }
    let parent = path
        .parent()
        .context("Runtime log has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    if !parent.canonicalize()?.starts_with(state.root()) {
        bail!("Runtime log directory escapes private state");
    }
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink() || !m.is_file())
    {
        bail!("Runtime log must be a regular file");
    }
    let max = std::env::var("TELEGRAM_MCP_LOG_MAX_BYTES")
        .ok()
        .map(|s| s.parse::<u64>())
        .transpose()
        .context("Invalid runtime log byte limit")?
        .unwrap_or(1024 * 1024);
    if !(4096..=100 * 1024 * 1024).contains(&max) {
        bail!("Runtime log limit must be between 4 KiB and 100 MiB");
    }
    let file = private_open(&path)?;
    let bytes = file.metadata()?.len();
    let writer = Writer(Arc::new(Mutex::new(Destination {
        path,
        file: Some(file),
        bytes,
        max,
    })));
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter("off,telegram_mcp=info,operator_plugin=info")
        .with_writer(writer)
        .try_init()
        .map_err(|_| anyhow::anyhow!("Runtime logging could not be initialized"))?;
    Ok(())
}
