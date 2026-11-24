#[cfg(not(windows))]
use {anyhow::Result, std::path::Path};

#[cfg(windows)]
#[path = "server_windows.rs"]
mod windows;
#[cfg(windows)]
pub(super) use windows::{running, start, stop};

// The original process-tree maintenance applies to the Windows launcher only.
#[cfg(not(windows))]
pub(super) fn running(_: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(not(windows))]
pub(super) fn stop(_: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(not(windows))]
pub(super) fn start(_: &Path, _: &Path) -> Result<u32> {
    anyhow::bail!("There is no Windows launcher to restart on this platform")
}
