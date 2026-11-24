use super::State;
use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FileAccess {
    Read,
    Write,
}

impl State {
    /// Resolve a path through its existing ancestors, including symlink targets.
    /// Callers must still use exclusive creation for outputs; resolution is not a file lock.
    pub fn resolve_file(&self, path: &Path, access: FileAccess) -> Result<PathBuf> {
        if path.as_os_str().is_empty() {
            bail!("A file path must not be empty");
        }
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            bail!("Parent traversal is not allowed in a file path");
        }
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            self.roots
                .first()
                .context("No allowed file roots are configured")?
                .join(path)
        };
        let resolved = canonical_existing(&joined)?;
        if resolved.starts_with(&self.root) {
            bail!("The server state directory is protected");
        }
        if resolved.starts_with(&self.installation)
            && !resolved.starts_with(self.installation.join("files"))
        {
            bail!("The installation is protected; use its files directory");
        }
        if !self.roots.iter().any(|root| resolved.starts_with(root)) {
            bail!("The path is outside configured file roots");
        }
        if resolved.components().any(|part| matches!(part, Component::Normal(name) if [".env", ".git", ".ai", ".codex", ".agents"].iter().any(|blocked| name.to_string_lossy().eq_ignore_ascii_case(blocked)))) {
            bail!("The path names private configuration or repository data");
        }
        if access == FileAccess::Read && !resolved.is_file() {
            bail!("The source is not an existing regular file");
        }
        if resolved.exists() && resolved.is_dir() && access == FileAccess::Read {
            bail!("A regular file is required");
        }
        Ok(resolved)
    }
}

pub(crate) fn canonical_existing(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut tail = Vec::new();
    while !existing.exists() {
        if existing.symlink_metadata().is_ok() {
            bail!("The path contains a broken symlink");
        }
        tail.push(
            existing
                .file_name()
                .context("The path has no existing ancestor")?
                .to_owned(),
        );
        existing = existing
            .parent()
            .context("The path has no existing ancestor")?;
    }
    let mut result = existing
        .canonicalize()
        .context("Cannot resolve file path")?;
    for component in tail.into_iter().rev() {
        result.push(component);
    }
    Ok(result)
}
