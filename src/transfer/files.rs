//! Anchored filesystem operations for long-lived HTTP body streams. A renamed
//! parent never redirects publication or cleanup to a different directory.
use crate::state::{FileAccess, State};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
#[cfg(all(test, unix))]
#[path = "../../tests/unit/transfer_files.rs"]
mod tests;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u32,
    #[cfg(windows)]
    index: u64,
}
pub(super) fn identity(file: &File) -> Result<Identity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        Ok(Identity {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: file owns a live OS handle and info is a writable native
        // structure. Its fields are read only after the API reports success.
        let result = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        ensure!(result != 0, "Cannot identify transfer file");
        let info = unsafe { info.assume_init() };
        Ok(Identity {
            volume: info.dwVolumeSerialNumber,
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        anyhow::bail!("File transfers are unsupported on this operating system")
    }
}

pub(crate) struct Directory {
    file: File,
    path: PathBuf,
    identity: Identity,
    target: Option<(PathBuf, FileAccess)>,
}
fn directory_options() -> std::fs::OpenOptions {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        // Deny delete-sharing while the request owns this directory. This
        // prevents renames/reparse replacements throughout the upload.
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}
impl Directory {
    pub(crate) fn open(state: &State, path: &Path) -> Result<Self> {
        Self::open_inner(state, path, None)
    }
    pub(crate) fn open_for(state: &State, parent: &Path, destination: &Path) -> Result<Self> {
        Self::open_inner(state, parent, Some((destination, FileAccess::Write)))
    }
    fn open_inner(state: &State, path: &Path, target: Option<(&Path, FileAccess)>) -> Result<Self> {
        Self::check_authorized_path(state, path, target, "Transfer directory changed")?;
        let file = directory_options()
            .open(path)
            .context("Cannot hold transfer directory")?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "Transfer parent is not a regular directory"
        );
        let directory = Self {
            identity: identity(&file)?,
            file,
            path: path.to_owned(),
            target: target.map(|(dest, access)| (dest.to_owned(), access)),
        };
        directory.verify(state)?;
        Ok(directory)
    }
    fn check_authorized_path(
        state: &State,
        path: &Path,
        target: Option<(&Path, FileAccess)>,
        message: &str,
    ) -> Result<()> {
        let resolved = match target {
            Some((destination, FileAccess::Write)) => {
                state.resolve_transfer_parent(path, destination)?
            }
            Some((source, access)) => state.resolve_transfer_parent_for(path, source, access)?,
            None => state.resolve_file(path, FileAccess::Write)?,
        };
        ensure!(resolved == path, "{message}");
        Ok(())
    }
    pub(crate) fn verify(&self, state: &State) -> Result<()> {
        ensure!(
            identity(&self.file)? == self.identity,
            "Held transfer directory identity changed"
        );
        Self::check_authorized_path(
            state,
            &self.path,
            self.target
                .as_ref()
                .map(|(target, access)| (target.as_path(), *access)),
            "Transfer directory moved outside its allowed root",
        )?;
        let current = directory_options().open(&self.path)?;
        ensure!(
            identity(&current)? == self.identity,
            "Transfer directory was replaced"
        );
        Ok(())
    }
    #[cfg(unix)]
    fn open_file(&self, name: &std::ffi::CStr) -> Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        // SAFETY: the descriptor and NUL-terminated name are owned through the
        // call, and a successful new descriptor is transferred exactly once.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        ensure!(fd >= 0, "Cannot open anchored transfer file");
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

pub(super) struct Upload {
    destination: PathBuf,
    file: File,
    #[cfg(unix)]
    temporary: Option<std::ffi::CString>,
    #[cfg(windows)]
    temporary: Option<tempfile::NamedTempFile>,
    // Rust drops fields in declaration order: the Windows rename guard must
    // outlive the staging file's pathname-based cleanup on every error path.
    directory: Directory,
}
impl Upload {
    pub fn new(state: &State, destination: &Path) -> Result<Self> {
        let sanitized = crate::state::sanitize_download_path(destination)?;
        let resolved = state.resolve_file(&sanitized, FileAccess::Write)?;
        ensure!(resolved == sanitized, "Transfer destination changed");
        let destination = &resolved;
        let parent = destination
            .parent()
            .context("Upload destination has no parent")?;
        let directory = Directory::open_for(state, parent, destination)?;
        #[cfg(unix)]
        {
            use std::os::{
                fd::{AsRawFd, FromRawFd},
                unix::ffi::OsStrExt,
            };
            let temporary =
                std::ffi::CString::new(format!(".upload-{:032x}.tmp", rand::random::<u128>()))?;
            // SAFETY: the owned directory descriptor is live, the name is
            // NUL-terminated, and O_EXCL refuses an existing file/symlink.
            let fd = unsafe {
                libc::openat(
                    directory.file.as_raw_fd(),
                    temporary.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            ensure!(fd >= 0, "Cannot create private upload staging file");
            // SAFETY: openat returned a fresh descriptor, transferred once.
            let file = unsafe { File::from_raw_fd(fd) };
            let upload = Self {
                directory,
                destination: destination.to_owned(),
                file,
                temporary: Some(temporary),
            };
            ensure!(
                !destination
                    .file_name()
                    .context("Upload filename is missing")?
                    .as_bytes()
                    .contains(&0),
                "Invalid upload filename"
            );
            Ok(upload)
        }
        #[cfg(windows)]
        {
            let temporary = tempfile::NamedTempFile::new_in(parent)?;
            crate::state::storage::restrict(temporary.path(), false)?;
            let file = temporary.reopen()?;
            Ok(Self {
                directory,
                destination: destination.to_owned(),
                file,
                temporary: Some(temporary),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = directory;
            anyhow::bail!("File transfers are unsupported on this operating system")
        }
    }
    pub fn writer(&self) -> Result<File> {
        Ok(self.file.try_clone()?)
    }
    pub fn publish(self, state: &State) -> Result<()> {
        self.directory.verify(state)?;
        ensure!(
            state.resolve_file(&self.destination, FileAccess::Write)? == self.destination,
            "Upload destination changed"
        );
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let target = std::ffi::CString::new(
                self.destination
                    .file_name()
                    .context("Upload filename is missing")?
                    .as_bytes(),
            )?;
            let temporary = self
                .temporary
                .as_ref()
                .context("Upload staging file is unavailable")?;
            let parent = self.directory.file.as_raw_fd();
            let expected = identity(&self.file)?;
            ensure!(
                identity(&self.directory.open_file(temporary)?)? == expected,
                "Upload staging file was replaced"
            );
            // linkat publishes into the held directory and refuses any existing
            // destination atomically. No absolute path is dereferenced here.
            let result =
                unsafe { libc::linkat(parent, temporary.as_ptr(), parent, target.as_ptr(), 0) };
            ensure!(
                result == 0,
                "Upload destination already exists or publication failed"
            );
            let matches = self
                .directory
                .open_file(&target)
                .and_then(|file| identity(&file))
                .is_ok_and(|current| current == expected);
            if !matches {
                unsafe {
                    libc::unlinkat(parent, target.as_ptr(), 0);
                }
                anyhow::bail!("Published upload identity changed");
            }
            // Drop always removes only our staging name relative to its held
            // directory, including after the pathname was renamed/replaced.
            Ok(())
        }
        #[cfg(windows)]
        {
            self.temporary
                .context("Upload staging file is unavailable")?
                .persist_noclobber(&self.destination)
                .context("Upload destination exists or publication failed")?;
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            anyhow::bail!("Unsupported transfer platform")
        }
    }
}
#[cfg(unix)]
impl Drop for Upload {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        if let Some(name) = &self.temporary {
            // SAFETY: unlinkat uses our live directory descriptor and one local
            // NUL-terminated filename, never a substituted path prefix.
            unsafe {
                libc::unlinkat(self.directory.file.as_raw_fd(), name.as_ptr(), 0);
            }
        }
    }
}

pub(crate) fn open_download(state: &State, path: &Path) -> Result<File> {
    ensure!(
        state.resolve_file(path, FileAccess::Read)? == path,
        "Download source changed"
    );
    let parent = path.parent().context("Download source has no parent")?;
    let directory = Directory::open_inner(state, parent, Some((path, FileAccess::Read)))?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(
            path.file_name()
                .context("Download filename is missing")?
                .as_bytes(),
        )?;
        let file = directory.open_file(&name)?;
        ensure!(
            file.metadata()?.is_file(),
            "Download source must be a regular file"
        );
        directory.verify(state)?;
        Ok(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let file = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Download source must be a regular file"
        );
        directory.verify(state)?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("Unsupported transfer platform")
    }
}
