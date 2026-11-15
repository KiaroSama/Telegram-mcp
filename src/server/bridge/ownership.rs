//! OS process identity is checked around attachment; a PID alone is reusable.
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context;
use anyhow::{Result, ensure};
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub image: PathBuf,
    started: String,
}

#[cfg(target_os = "linux")]
pub(super) fn identity(pid: u32) -> Result<Identity> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let image = root
        .join("exe")
        .canonicalize()
        .context("Cannot verify shared service executable")?;
    let stat = std::fs::read_to_string(root.join("stat"))?;
    let (_, tail) = stat
        .rsplit_once(')')
        .context("Invalid shared process identity")?;
    let started = tail
        .split_whitespace()
        .nth(19)
        .context("Incomplete shared process identity")?
        .into();
    Ok(Identity { image, started })
}

#[cfg(target_os = "macos")]
pub(super) fn identity(pid: u32) -> Result<Identity> {
    use std::os::unix::ffi::OsStrExt;
    let pid = i32::try_from(pid).context("Invalid shared service PID")?;
    let mut path = vec![0_u8; 4096];
    let mut details = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: both native calls receive writable buffers of the declared size;
    // proc_pidinfo's complete return size is checked before initialization.
    let (length, size) = unsafe {
        (
            libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32),
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                details.as_mut_ptr().cast(),
                std::mem::size_of::<libc::proc_bsdinfo>() as i32,
            ),
        )
    };
    ensure!(
        length > 0 && size as usize == std::mem::size_of::<libc::proc_bsdinfo>(),
        "Cannot verify shared service process identity"
    );
    // SAFETY: proc_pidinfo wrote the entire checked proc_bsdinfo structure.
    let details = unsafe { details.assume_init() };
    let end = path
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(length as usize);
    let image = PathBuf::from(std::ffi::OsStr::from_bytes(&path[..end])).canonicalize()?;
    Ok(Identity {
        image,
        started: format!("{}:{}", details.pbi_start_tvsec, details.pbi_start_tvusec),
    })
}

#[cfg(windows)]
pub(super) fn identity(pid: u32) -> Result<Identity> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    };
    // SAFETY: read-only query rights for the requested process; handle validity
    // is checked, and a guard closes it on every exit path.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    ensure!(!handle.is_null(), "Cannot inspect shared service process");
    struct Handle(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let handle = Handle(handle);
    let mut path = vec![0_u16; 32768];
    let mut length = path.len() as u32;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the held handle remains live, all output pointers reference
    // writable buffers, and the resulting string length is validated.
    unsafe {
        ensure!(
            QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) != 0
                && length > 0
                && (length as usize) < path.len(),
            "Cannot verify shared service executable"
        );
        ensure!(
            GetProcessTimes(handle.0, &mut created, &mut exit, &mut kernel, &mut user) != 0,
            "Cannot verify shared service creation identity"
        );
    }
    let image =
        PathBuf::from(std::ffi::OsString::from_wide(&path[..length as usize])).canonicalize()?;
    Ok(Identity {
        image,
        started: format!("{}:{}", created.dwHighDateTime, created.dwLowDateTime),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn identity(_: u32) -> Result<Identity> {
    anyhow::bail!("This platform cannot verify shared service process ownership")
}

pub(super) fn verified(pid: u32, expected: &std::path::Path) -> Result<Identity> {
    let identity = identity(pid)?;
    ensure!(
        identity.image == expected.canonicalize()?,
        "Shared service belongs to another executable; it was not replaced"
    );
    Ok(identity)
}
