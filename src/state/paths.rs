use super::State;
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FileAccess {
    Read,
    Write,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ScopedPathGrant {
    pub path: PathBuf,
    pub access: FileAccess,
    pub allow_descendants: bool,
}

tokio::task_local! {
    static TEMPORARY_PATH_GRANTS: Vec<ScopedPathGrant>;
}

pub(crate) async fn with_temporary_grants<F, T>(grants: Vec<ScopedPathGrant>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TEMPORARY_PATH_GRANTS.scope(grants, fut).await
}

fn active_temporary_grants() -> Vec<ScopedPathGrant> {
    TEMPORARY_PATH_GRANTS
        .try_with(Clone::clone)
        .unwrap_or_default()
}

fn matching_temporary_grant(resolved: &Path, access: FileAccess) -> Option<PathBuf> {
    active_temporary_grants().into_iter().find_map(|grant| {
        if grant.access != access
            || !grant.path.is_absolute()
            || grant.path.components().any(|part| {
                matches!(part, Component::ParentDir | Component::CurDir)
                    || matches!(part, Component::Normal(seg) if seg == "." || seg == "..")
            })
            || validate_path_syntax(&grant.path).is_err()
        {
            return None;
        }
        // Compare against the already-canonical approved target without re-canonicalizing
        // so post-approval symlink substitution cannot re-authorize a new target.
        let allowed = resolved == grant.path
            || (grant.allow_descendants
                && access == FileAccess::Write
                && resolved.starts_with(&grant.path));
        allowed.then_some(grant.path)
    })
}

pub const DANGEROUS_SUFFIXES: &[&str] = &[
    ".hta", ".cmd", ".bat", ".com", ".exe", ".scr", ".pif", ".msi", ".msp", ".ps1", ".vbs", ".vbe",
    ".js", ".jse", ".wsf", ".wsh", ".reg", ".lnk", ".url",
];

/// Validate a file suffix against the portable safe-download policy.
/// A suffix must be one dot followed by 1-7 ASCII alphanumerics and not on the denylist.
pub fn safe_suffix(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches(['.', ' ']);
    let Some(rest) = trimmed.strip_prefix('.') else {
        return ".bin".to_owned();
    };
    if !(1..=7).contains(&rest.len()) || !rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return ".bin".to_owned();
    }
    let lower = format!(".{}", rest.to_ascii_lowercase());
    if DANGEROUS_SUFFIXES.contains(&lower.as_str()) {
        return ".bin".to_owned();
    }
    lower
}

/// Apply `safe_suffix` to the final filename component, accounting for Windows
/// trailing-dot/space normalization. Rejects invalid or unsafe characters.
pub fn sanitize_download_filename(name: &str) -> Result<String> {
    ensure!(
        !name.is_empty() && name != "." && name != "..",
        "Download filename is empty or invalid"
    );
    ensure!(
        !name
            .chars()
            .any(|c| matches!(c, '\0' | '/' | '\\' | '*' | '?' | '[' | ']' | ':' | '~')),
        "Download filename contains forbidden characters"
    );
    let had_trailing_dot = name.ends_with('.');
    let effective = name.trim_end_matches(['.', ' ']);
    ensure!(
        !effective.is_empty() && effective != "." && effective != "..",
        "Download filename resolves to an empty name"
    );
    if let Some(dot) = effective.rfind('.')
        && dot > 0
    {
        let (stem, ext) = effective.split_at(dot);
        let stem = stem.trim_end_matches(['.', ' ']);
        ensure!(!stem.is_empty(), "Download filename stem is empty");
        return Ok(format!("{stem}{}", safe_suffix(ext)));
    }
    if had_trailing_dot {
        return Ok(format!("{effective}.bin"));
    }
    Ok(effective.to_owned())
}

/// Validate caller-supplied path syntax before any filesystem write or resolution.
pub fn validate_path_syntax(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("A file path must not be empty");
    }
    let raw = path.to_string_lossy();
    if raw.starts_with('~') {
        bail!("Tilde expansion is not allowed in a file path");
    }
    if raw.contains('\0') {
        bail!("NUL bytes are not allowed in a file path");
    }
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::ParentDir => bail!("Parent traversal is not allowed in a file path"),
            Component::Prefix(prefix) => {
                if matches!(prefix.kind(), std::path::Prefix::Disk(_))
                    && !matches!(components.peek(), Some(Component::RootDir))
                {
                    bail!(
                        "Drive-relative paths and alternate data streams are not allowed in a file path"
                    );
                }
            }
            Component::Normal(segment) => {
                let text = segment.to_string_lossy();
                if text
                    .chars()
                    .any(|c| matches!(c, '*' | '?' | '[' | ']' | ':'))
                {
                    bail!("Wildcards and alternate data streams are not allowed in a file path");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Apply `safe_suffix` to the final component of a destination path before creating files.
pub fn sanitize_download_path(path: &Path) -> Result<PathBuf> {
    validate_path_syntax(path)?;
    let name = path
        .file_name()
        .context("Download destination has no filename")?
        .to_str()
        .context("Download filename must be valid UTF-8")?;
    let sanitized = sanitize_download_filename(name)?;
    Ok(path.with_file_name(sanitized))
}

pub(crate) fn parse_roots_setting(raw: &str) -> Result<Vec<PathBuf>> {
    if raw.trim_start().starts_with('[') {
        serde_json::from_str::<Vec<String>>(raw)
            .context("TELEGRAM_FILE_ROOTS must contain an array of paths")?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
            .into_iter()
            .map(Ok)
            .collect()
    } else {
        Ok(std::env::split_paths(raw).collect())
    }
}

fn normalize_existing_roots(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut normalized = Vec::new();
    for path in paths {
        if let Ok(canonical) = path.canonicalize()
            && canonical.is_dir()
            && !normalized.contains(&canonical)
        {
            normalized.push(canonical);
        }
    }
    normalized
}

fn reject_tilde_tail(resolved: &Path, matched_root: &Path) -> Result<()> {
    if let Ok(relative) = resolved.strip_prefix(matched_root) {
        for component in relative.components() {
            if let Component::Normal(segment) = component
                && segment.to_string_lossy().contains('~')
            {
                bail!("Tilde-bearing destinations are not allowed in a file path");
            }
        }
    }
    if resolved
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains('~'))
    {
        bail!("Tilde-bearing destinations are not allowed in a file path");
    }
    Ok(())
}

impl State {
    pub(crate) fn installation_root(&self) -> &Path {
        &self.installation
    }

    /// Refresh configured roots from the live `.env` / process configuration when
    /// this state was constructed from the environment.
    pub fn refresh_env_file_roots(&self) -> Result<Vec<PathBuf>> {
        if !self.env_roots_enabled {
            return Ok(self.roots.clone());
        }
        let configured = std::env::var("TELEGRAM_FILE_ROOTS")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                crate::account_manager::current_config()
                    .ok()
                    .and_then(|snap| snap.values.get("TELEGRAM_FILE_ROOTS").cloned())
                    .filter(|v| !v.trim().is_empty())
            });
        match configured {
            Some(raw) => Ok(normalize_existing_roots(parse_roots_setting(&raw)?)),
            None => Ok(self.roots.clone()),
        }
    }

    /// Folders permanently allowed via `always-approvals.json` or `folder-grants.json`.
    pub fn always_allowed_folders(&self, access: Option<FileAccess>) -> Vec<PathBuf> {
        let mut folders = Vec::new();
        if let Ok(store) = self.read_json("always-approvals.json", json!({"folders":[]}))
            && let Some(rows) = store["folders"].as_array()
        {
            for row in rows {
                if let Some(raw) = row.as_str() {
                    folders.push(PathBuf::from(raw));
                } else if let Some(dir) = row["directory"].as_str() {
                    folders.push(PathBuf::from(dir));
                }
            }
        }
        if let Ok(store) = self.read_json("folder-grants.json", json!({"folders":[]}))
            && let Some(rows) = store["folders"].as_array()
        {
            for row in rows {
                if row.get("request_id").is_some_and(|v| !v.is_null()) {
                    continue;
                }
                let scope_ok = matches!(
                    (access, row["scope"].as_str()),
                    (None, _)
                        | (_, Some("read_write"))
                        | (Some(FileAccess::Read), Some("read"))
                        | (Some(FileAccess::Write), Some("write"))
                );
                if scope_ok && let Some(dir) = row["directory"].as_str() {
                    folders.push(PathBuf::from(dir));
                }
            }
        }
        normalize_existing_roots(folders)
            .into_iter()
            .filter(|dir| {
                !dir.starts_with(&self.root)
                    && (!dir.starts_with(&self.installation)
                        || dir.starts_with(self.installation.join("files")))
            })
            .collect()
    }

    /// Free/default workspace folders (configured roots, e.g. `files/`).
    pub fn free_folders(&self) -> Vec<PathBuf> {
        self.refresh_env_file_roots()
            .unwrap_or_else(|_| self.roots.clone())
    }

    pub fn effective_file_roots(
        &self,
        access: FileAccess,
        extra_roots: &[PathBuf],
    ) -> Vec<PathBuf> {
        let mut roots = self.free_folders();
        for dir in self
            .always_allowed_folders(Some(access))
            .into_iter()
            .chain(normalize_existing_roots(extra_roots.to_vec()))
        {
            if !roots.contains(&dir) {
                roots.push(dir);
            }
        }
        roots
    }

    /// Resolve a path through its existing ancestors, including symlink targets.
    /// Callers must still use exclusive creation for outputs; resolution is not a file lock.
    pub fn resolve_file(&self, path: &Path, access: FileAccess) -> Result<PathBuf> {
        self.resolve_file_with_roots(path, access, &[])
    }

    /// Resolve a path against configured roots, always-allowed folders, and optional
    /// per-request approved folder roots.
    pub fn resolve_file_with_roots(
        &self,
        path: &Path,
        access: FileAccess,
        extra_roots: &[PathBuf],
    ) -> Result<PathBuf> {
        validate_path_syntax(path)?;
        let roots = self.effective_file_roots(access, extra_roots);
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            roots
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
        let matched_root = roots
            .iter()
            .find(|root| resolved.starts_with(root))
            .cloned()
            .or_else(|| matching_temporary_grant(&resolved, access))
            .context("The path is outside configured file roots")?;
        reject_tilde_tail(&resolved, &matched_root)?;
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

    /// Validate and sanitize a download destination path before creating any temp or final file.
    pub fn resolve_download_file(&self, path: &Path) -> Result<PathBuf> {
        let sanitized = sanitize_download_path(path)?;
        self.resolve_file(&sanitized, FileAccess::Write)
    }

    /// Resolve a transfer parent directory solely for an authorized destination file without
    /// broadening a single-file temporary grant to the parent directory or sibling files.
    pub(crate) fn resolve_transfer_parent(
        &self,
        parent: &Path,
        destination: &Path,
    ) -> Result<PathBuf> {
        self.resolve_transfer_parent_for(parent, destination, FileAccess::Write)
    }

    pub(crate) fn resolve_transfer_parent_for(
        &self,
        parent: &Path,
        target: &Path,
        access: FileAccess,
    ) -> Result<PathBuf> {
        validate_path_syntax(parent)?;
        let authorized = match access {
            FileAccess::Write => self.resolve_download_file(target)?,
            FileAccess::Read => self.resolve_file(target, FileAccess::Read)?,
        };
        let expected_parent = authorized
            .parent()
            .context("Transfer destination has no parent")?;
        let canonical_parent = canonical_existing(parent)?;
        ensure!(
            canonical_parent == expected_parent,
            "Transfer parent does not match authorized file parent"
        );
        if canonical_parent.starts_with(&self.root) {
            bail!("The server state directory is protected");
        }
        if canonical_parent.starts_with(&self.installation)
            && !canonical_parent.starts_with(self.installation.join("files"))
        {
            bail!("The installation is protected; use its files directory");
        }
        if canonical_parent.components().any(|part| {
            matches!(part, Component::Normal(name) if [".env", ".git", ".ai", ".codex", ".agents"]
                .iter()
                .any(|blocked| name.to_string_lossy().eq_ignore_ascii_case(blocked)))
        }) {
            bail!("The path names private configuration or repository data");
        }
        ensure!(
            canonical_parent.is_dir(),
            "Transfer parent is not an existing directory"
        );
        Ok(canonical_parent)
    }

    pub(crate) fn resolved_event_feed_path(&self) -> Result<PathBuf> {
        let Some(custom) = &self.feed_file else {
            return Ok(self.root.join("incoming_feed.jsonl"));
        };
        validate_path_syntax(custom)?;
        let candidate = if custom.is_absolute() {
            custom.clone()
        } else {
            self.root.join(custom)
        };
        let resolved = canonical_existing(&candidate)?;
        if resolved.starts_with(&self.root) {
            if let Ok(meta) = resolved.symlink_metadata()
                && (meta.file_type().is_symlink() || meta.is_dir())
            {
                bail!("Event feed path must be a regular file");
            }
            return Ok(resolved);
        }
        let checked = self.resolve_file(custom, FileAccess::Write)?;
        if checked.exists() && checked.is_dir() {
            bail!("Event feed path must be a regular file");
        }
        Ok(checked)
    }

    pub(crate) fn resolved_live_alias_path(&self) -> Result<Option<PathBuf>> {
        let Some(custom) = &self.live_alias_file else {
            return Ok(None);
        };
        validate_path_syntax(custom)?;
        let candidate = if custom.is_absolute() {
            custom.clone()
        } else {
            self.root.join(custom)
        };
        let resolved = canonical_existing(&candidate)?;
        if resolved.starts_with(&self.root) {
            if let Ok(meta) = resolved.symlink_metadata()
                && (meta.file_type().is_symlink() || meta.is_dir())
            {
                bail!("Alias store target must be a regular file");
            }
            return Ok(Some(resolved));
        }
        let checked = self.resolve_file(custom, FileAccess::Write)?;
        if checked.exists() && checked.is_dir() {
            bail!("Alias store target must be a regular file");
        }
        Ok(Some(checked))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scoped_path_grants_enforce_exact_path_access_and_protected_refusals() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let root_dir = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("target.txt");
        let sibling = outside.join("sibling.txt");
        std::fs::write(&target, "ok").unwrap();
        std::fs::write(&sibling, "no").unwrap();
        let state = State::at(state_dir.clone(), vec![root_dir.clone()]).unwrap();
        let canonical_target = canonical_existing(&target).unwrap();
        let canonical_outside = canonical_existing(&outside).unwrap();
        let canonical_state = canonical_existing(&state_dir).unwrap();

        let read_grant = vec![ScopedPathGrant {
            path: canonical_target.clone(),
            access: FileAccess::Read,
            allow_descendants: false,
        }];
        with_temporary_grants(read_grant, async {
            assert!(state.resolve_file(&target, FileAccess::Read).is_ok());
            assert!(state.resolve_file(&sibling, FileAccess::Read).is_err());
            assert!(state.resolve_file(&target, FileAccess::Write).is_err());
        })
        .await;
        assert!(state.resolve_file(&target, FileAccess::Read).is_err());

        let traversal_grant = vec![
            ScopedPathGrant {
                path: outside.join("..").join("outside").join("target.txt"),
                access: FileAccess::Read,
                allow_descendants: false,
            },
            ScopedPathGrant {
                path: PathBuf::from(format!(
                    "{}{}..{}outside{}target.txt",
                    canonical_outside.display(),
                    std::path::MAIN_SEPARATOR,
                    std::path::MAIN_SEPARATOR,
                    std::path::MAIN_SEPARATOR
                )),
                access: FileAccess::Read,
                allow_descendants: false,
            },
        ];
        with_temporary_grants(traversal_grant, async {
            assert!(state.resolve_file(&target, FileAccess::Read).is_err());
        })
        .await;

        let read_dir_grant = vec![ScopedPathGrant {
            path: canonical_outside.clone(),
            access: FileAccess::Read,
            allow_descendants: true,
        }];
        with_temporary_grants(read_dir_grant, async {
            assert!(state.resolve_file(&target, FileAccess::Read).is_err());
        })
        .await;

        let write_file = outside.join("single_out.txt");
        let canonical_write_file = canonical_existing(&write_file).unwrap();
        with_temporary_grants(
            vec![ScopedPathGrant {
                path: canonical_write_file,
                access: FileAccess::Write,
                allow_descendants: false,
            }],
            async {
                assert_eq!(
                    state
                        .resolve_transfer_parent(&outside, &write_file)
                        .unwrap(),
                    canonical_outside
                );
                assert!(state.resolve_transfer_parent(&outside, &sibling).is_err());
                assert!(
                    state
                        .resolve_transfer_parent(&root_dir, &write_file)
                        .is_err()
                );
                assert!(state.resolve_file(&outside, FileAccess::Write).is_err());
                assert!(state.resolve_file(&sibling, FileAccess::Write).is_err());
            },
        )
        .await;

        let write_dir_grant = vec![
            ScopedPathGrant {
                path: canonical_outside,
                access: FileAccess::Write,
                allow_descendants: true,
            },
            ScopedPathGrant {
                path: canonical_state,
                access: FileAccess::Write,
                allow_descendants: true,
            },
        ];
        with_temporary_grants(write_dir_grant, async {
            assert!(
                state
                    .resolve_download_file(&outside.join("nested").join("out.txt"))
                    .is_ok()
            );
            assert!(state.resolve_file(&target, FileAccess::Read).is_err());
            assert!(
                state
                    .resolve_file(&state_dir.join("escape.txt"), FileAccess::Write)
                    .is_err()
            );
            assert!(
                state
                    .resolve_file(&outside.join(".env"), FileAccess::Write)
                    .is_err()
            );
        })
        .await;
    }
}
