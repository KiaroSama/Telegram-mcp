//! One byte, matching PowerShell FileStream.Lock(0, 1) on every platform.
use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub const DEFAULT_LOCK_GRACE_SECONDS: f64 = 20.0;
pub const MAX_LOCK_GRACE_SECONDS: f64 = 300.0;

/// Parse `TELEGRAM_LOCK_GRACE_SECONDS`: default 20s, inclusive `0..=300` finite bound.
/// Zero performs a single immediate lock attempt; nonempty invalid values fail closed.
pub fn parse_lock_grace_seconds(raw: Option<&str>) -> Result<Duration> {
    let Some(text) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Duration::from_secs_f64(DEFAULT_LOCK_GRACE_SECONDS));
    };
    let parsed = text
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=MAX_LOCK_GRACE_SECONDS).contains(v))
        .context("TELEGRAM_LOCK_GRACE_SECONDS must be a finite number between 0 and 300")?;
    Ok(Duration::from_secs_f64(parsed))
}

pub fn lock_grace_seconds(snapshot: Option<&super::ConfigSnapshot>) -> Result<Duration> {
    let raw = snapshot
        .and_then(|s| s.values.get("TELEGRAM_LOCK_GRACE_SECONDS").cloned())
        .or_else(|| std::env::var("TELEGRAM_LOCK_GRACE_SECONDS").ok());
    parse_lock_grace_seconds(raw.as_deref())
}

/// Wait monotonically and cancellably for an exclusive OS lease up to `grace`.
/// Zero grace executes exactly one immediate attempt without sleeping.
pub async fn acquire_with_grace<F, T>(
    grace: Duration,
    cancel: Option<&tokio_util::sync::CancellationToken>,
    mut try_acquire: F,
) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let deadline = tokio::time::Instant::now() + grace;
    let poll_interval = Duration::from_millis(50);
    loop {
        if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
            bail!("Lock acquisition cancelled before acquiring lease");
        }
        match try_acquire() {
            Ok(guard) => return Ok(guard),
            Err(error) => {
                if grace.is_zero() || tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let step = remaining.min(poll_interval);
                if let Some(token) = cancel {
                    tokio::select! {
                        biased;
                        () = token.cancelled() => bail!("Lock acquisition cancelled before acquiring lease"),
                        () = tokio::time::sleep(step) => {}
                    }
                } else {
                    tokio::time::sleep(step).await;
                }
            }
        }
    }
}

static LOCAL: Mutex<()> = Mutex::new(());
pub(in crate::account_manager) struct ConfigWriteGuard {
    pub path: PathBuf,
    file: File,
    _local: MutexGuard<'static, ()>,
}

pub(in crate::account_manager) fn lock(path: &Path) -> Result<ConfigWriteGuard> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = path
        .parent()
        .context("Configuration file has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    // Admit new Windows transactions only where the namespace barrier is real.
    // This uses the native primitive directly; the account fault seam exercises
    // failures after otherwise-supported publication, rather than admission.
    #[cfg(windows)]
    crate::state::storage::sync_directory(parent)
        .context("Account configuration requires an accessible NTFS directory on Windows")?;
    let local = LOCAL
        .lock()
        .map_err(|_| anyhow::anyhow!("Account configuration is busy"))?;
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let lock_path = PathBuf::from(name);
    if lock_path
        .symlink_metadata()
        .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
    {
        bail!("Account configuration lease must be a regular file");
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(&lock_path)?;
    crate::state::storage::restrict(&lock_path, false)?;
    set_lock(&file, true).context("Account configuration is busy")?;
    Ok(ConfigWriteGuard {
        path,
        file,
        _local: local,
    })
}
impl Drop for ConfigWriteGuard {
    fn drop(&mut self) {
        let _ = set_lock(&self.file, false);
    }
}

#[cfg(unix)]
fn set_lock(file: &File, locked: bool) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: zero is a valid initial flock; all fields needed by F_SETLK are set.
    let mut record: libc::flock = unsafe { std::mem::zeroed() };
    record.l_type = if locked { libc::F_WRLCK } else { libc::F_UNLCK } as _;
    record.l_whence = libc::SEEK_SET as _;
    record.l_start = 0;
    record.l_len = 1;
    // SAFETY: the descriptor is live and the flock pointer is valid for this call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &record) } < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn set_lock(file: &File, locked: bool) -> std::io::Result<()> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn UnlockFileEx(
            file: *mut c_void,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    let mut offset = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    // SAFETY: the live handle and initialized OVERLAPPED describe byte 0 only;
    // the calls are synchronous (FAIL_IMMEDIATELY and no overlapped event).
    let success = unsafe {
        if locked {
            LockFileEx(file.as_raw_handle(), 3, 0, 1, 0, &mut offset)
        } else {
            UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut offset)
        }
    };
    if success == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
