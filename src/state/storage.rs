use super::State;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use rand::Rng;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path};

const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        bail!("Private state directory must not be a symlink");
    }
    std::fs::create_dir_all(path).context("Cannot create private state directory")?;
    restrict(path, true)
}

pub(crate) fn restrict(path: &Path, directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .context("Cannot restrict private state permissions")?;
    }
    #[cfg(windows)]
    windows_acl::restrict(path, directory)?;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, directory);
        bail!("Private state is unsupported on this operating system");
    }
    Ok(())
}

fn options(read: bool, write: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(read).write(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT opens the link itself, never its target.
        options.custom_flags(0x0020_0000);
    }
    options
}

impl State {
    pub(crate) fn append_event_line(&self, value: &Value) -> Result<()> {
        let _lock = self.state_lock()?;
        let mut value = value.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        value
            .as_object_mut()
            .context("An event summary must be an object")?
            .insert("ts".to_owned(), serde_json::json!(now));
        let bytes = serde_json::to_vec(&value)?;
        if bytes.len() > 65_536 {
            bail!("Event summary exceeds the byte budget");
        }
        let max_bytes = super::events::positive_limit("TELEGRAM_EVENT_FEED_MAX_BYTES", 8_388_608);
        let max_age = super::events::positive_limit("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", 604_800);
        if bytes.len() as u64 + 1 > max_bytes {
            bail!("Event summary exceeds the configured feed byte budget");
        }
        let path = self.root.join("incoming_feed.jsonl");
        let rotated = self.root.join("incoming_feed.jsonl.1");
        if let Ok(meta) = rotated.symlink_metadata() {
            if !meta.is_file() || meta.file_type().is_symlink() {
                bail!("Rotated event feed is not a regular file");
            }
            if feed_expired(&rotated, max_age)? {
                std::fs::remove_file(&rotated)?;
            }
        }
        if let Ok(meta) = path.symlink_metadata() {
            if !meta.is_file() || meta.file_type().is_symlink() {
                bail!("Event feed is not a regular file");
            }
            if meta.len() + bytes.len() as u64 + 1 > max_bytes || feed_expired(&path, max_age)? {
                if rotated.exists() {
                    std::fs::remove_file(&rotated)?;
                }
                std::fs::rename(&path, &rotated)?;
            }
        }
        let mut file = options(false, true).create(true).append(true).open(&path)?;
        restrict(&path, false)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }

    pub(crate) fn maintain_event_feed(&self) -> Result<()> {
        let _lock = self.state_lock()?;
        let max_age = super::events::positive_limit("TELEGRAM_EVENT_FEED_MAX_AGE_SECONDS", 604_800);
        for name in ["incoming_feed.jsonl", "incoming_feed.jsonl.1"] {
            let path = self.root.join(name);
            if path.exists() && feed_expired(&path, max_age)? {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }
    fn local_path(&self, name: &str) -> Result<std::path::PathBuf> {
        let path = Path::new(name);
        if path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
            || path.components().count() != 1
        {
            bail!("A state record must have a single local filename");
        }
        Ok(self.root.join(path))
    }

    pub(crate) fn read_json_unlocked(&self, name: &str, default: Value) -> Result<Value> {
        let path = self.local_path(name)?;
        let mut file = match options(true, false).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(default),
            Err(_) => bail!("Cannot read private state record; existing data was preserved"),
        };
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() > MAX_STATE_BYTES {
            bail!("Invalid or oversized private state record");
        }
        restrict(&path, false)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!("Private state record exceeded its byte limit");
        }
        let text = std::str::from_utf8(&bytes).context("Private state record is not UTF-8")?;
        serde_json::from_str(text)
            .context("Private state record is invalid JSON; existing data was preserved")
    }

    pub(crate) fn state_lock(&self) -> Result<File> {
        let path = self.local_path("state.lock")?;
        let lock = options(true, true)
            .create(true)
            .truncate(false)
            .open(&path)
            .context("Cannot open private state lock")?;
        if !lock.metadata()?.is_file() {
            bail!("Private state lock is not a regular file");
        }
        restrict(&path, false)?;
        // Session persistence and local metadata share a short critical section.
        // Retry contention instead of turning an unrelated alias update into an
        // authorization storage failure. No await or network work holds this lock.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match FileExt::try_lock_exclusive(&lock) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(_) => bail!("Private state is busy; retry the operation"),
            }
        }
        Ok(lock)
    }

    pub fn read_json(&self, name: &str, default: Value) -> Result<Value> {
        let _lock = self.state_lock()?;
        self.read_json_unlocked(name, default)
    }

    pub fn write_json(&self, name: &str, value: &Value) -> Result<()> {
        let _lock = self.state_lock()?;
        self.write_json_unlocked(name, value)
    }

    pub fn update_json<T>(
        &self,
        name: &str,
        default: Value,
        update: impl FnOnce(&mut Value) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.state_lock()?;
        let mut value = self.read_json_unlocked(name, default)?;
        let result = update(&mut value)?;
        self.write_json_unlocked(name, &value)?;
        Ok(result)
    }

    pub(crate) fn write_json_unlocked(&self, name: &str, value: &Value) -> Result<()> {
        let destination = self.local_path(name)?;
        if destination
            .symlink_metadata()
            .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
        {
            bail!("Refusing to replace a nonregular private state record");
        }
        let bytes = serde_json::to_vec_pretty(value)?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!("Private state record exceeds its byte limit");
        }
        let temporary = self
            .root
            .join(format!(".pending-{:016x}.tmp", rand::rng().random::<u64>()));
        let result = (|| -> Result<()> {
            let mut file = options(false, true).create_new(true).open(&temporary)?;
            restrict(&temporary, false)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &destination)
                .context("Cannot atomically replace private state record")?;
            #[cfg(unix)]
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

fn feed_expired(path: &Path, max_age: u64) -> Result<bool> {
    let file = options(true, false).open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("Event feed is not a regular file");
    }
    let mut first = String::new();
    BufReader::new(file).take(65_537).read_line(&mut first)?;
    if first.len() > 65_536 {
        bail!("Event feed record exceeds its byte budget");
    }
    if first.trim().is_empty() {
        return Ok(metadata.modified()?.elapsed().unwrap_or_default().as_secs() > max_age);
    }
    let record: Value = serde_json::from_str(&first)
        .context("Event feed first record is invalid; existing bytes were preserved")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    Ok(record["ts"]
        .as_f64()
        .filter(|value| value.is_finite())
        .map(|timestamp| now - timestamp > max_age as f64)
        .unwrap_or(metadata.modified()?.elapsed().unwrap_or_default().as_secs() > max_age))
}

#[cfg(windows)]
mod windows_acl {
    use anyhow::{Result, bail};
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *const c_void) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    pub fn restrict(path: &Path, directory: bool) -> Result<()> {
        // A protected DACL grants access to the object's owner and SYSTEM only.
        // Owner Rights avoids trusting an environment-provided username or shell.
        let sddl = if directory {
            "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"
        } else {
            "D:P(A;;FA;;;OW)(A;;FA;;;SY)"
        };
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: strings are nul-terminated UTF-16; out-pointer is writable. The
        // descriptor is freed exactly once after SetFileSecurityW finishes.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            ) == 0
            {
                bail!("Cannot construct a private Windows security descriptor");
            }
            let ok = SetFileSecurityW(wide.as_ptr(), 0x8000_0004, descriptor);
            LocalFree(descriptor);
            if ok == 0 {
                bail!("Cannot restrict private Windows state permissions");
            }
        }
        Ok(())
    }
}
