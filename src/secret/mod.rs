#![forbid(unsafe_code)]

mod export;
mod history;
mod migration;
mod runtime;

pub use export::ExportFormat;
pub use history::SecretHistory;
pub use migration::migrate_legacy;
pub(crate) use migration::{custody_dir, has_legacy, legacy_owner, open_store};
pub use runtime::SecretRuntime;
pub(crate) use runtime::{Prepared, session_binding};

use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAX_BYTES: usize = 64 * 1024 * 1024;

#[doc(hidden)]
pub const HISTORY_HANDLER_LABEL: &str = "mcp_history";

pub(crate) const CHAT_OFFSET: i64 = 2_000_000_000_000;

pub(crate) fn wire_chat_id(value: i64) -> Option<i32> {
    let wire = match value {
        ..=-1_000_000_000_000 => value + CHAT_OFFSET,
        1_000_000_000_000.. => value - CHAT_OFFSET,
        _ => value,
    };
    i32::try_from(wire).ok().filter(|id| *id != 0)
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Secret-chat path contains a symbolic link"
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "Secret-chat path contains a reparse point"
                );
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Cannot inspect secret-chat path"),
    }
}

fn plain_file(path: &Path) -> Result<bool> {
    let Some(metadata) = metadata(path)? else {
        return Ok(false);
    };
    ensure!(metadata.is_file(), "Secret-chat path is not a regular file");
    Ok(true)
}

fn private_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {
            crate::state::storage::restrict(path, true)?;
            crate::state::storage::sync_directory(
                path.parent()
                    .context("Secret-chat directory has no parent")?,
            )?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("Cannot create secret-chat directory"),
    }
    ensure!(
        metadata(path)?.is_some_and(|m| m.is_dir()),
        "Secret-chat directory is not a regular directory"
    );
    Ok(())
}

fn read_private(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    if !plain_file(path)? {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "Secret-chat file exceeds its byte budget"
    );
    Ok(Some(bytes))
}

fn write_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path.parent().context("Secret-chat output has no parent")?;
    plain_file(path)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    crate::state::storage::restrict(temp.path(), false)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    if replace {
        temp.persist(path).map_err(|error| error.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|error| error.error)?;
    }
    crate::state::storage::sync_directory(parent).context(
        "Secret-chat file was published but directory durability could not be confirmed",
    )?;
    Ok(())
}
