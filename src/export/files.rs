//! Safe archive-relative paths, collision-free naming, vCard output, and destination planning.

use crate::state::{FileAccess, State};
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Validates an archive-relative path: forward slashes only, no traversal, drives, or unsafe chars.
pub fn clean_relative_path(raw: &str) -> Option<String> {
    if raw.is_empty()
        || raw.trim() != raw
        || raw.starts_with('/')
        || raw.ends_with('/')
        || raw.contains(['\\', ':', '\0'])
        || raw
            .chars()
            .any(|c| c.is_control() || crate::text_fidelity::is_unsafe_char(c))
    {
        return None;
    }
    if raw
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(raw.to_owned())
}

/// Splits after the last `/` at the first `.` so compound extensions (`file.tar.xz`) stay intact.
fn split_first_extension(relative: &str) -> (&str, &str) {
    let after_slash = relative.rfind('/').map_or(0, |idx| idx + 1);
    match relative[after_slash..].find('.') {
        Some(dot) => {
            let split_at = after_slash + dot;
            (&relative[..split_at], &relative[split_at..])
        }
        None => (relative, ""),
    }
}

/// Desktop `Output::File::PrepareRelativePath`: picks `name (N).ext` when occupied.
pub fn prepare_relative_path(export_dir: &Path, suggested: &str) -> Result<String> {
    let clean = clean_relative_path(suggested)
        .ok_or_else(|| anyhow::anyhow!("Export relative path is invalid"))?;
    if !export_dir.join(&clean).exists() {
        return Ok(clean);
    }
    let (base, ext) = split_first_extension(&clean);
    for attempt in 1..=10_000 {
        let candidate = format!("{base} ({attempt}){ext}");
        if !export_dir.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    bail!("No unused relative export filename is available")
}

/// Writes a brand-new export asset inside `export_dir` using `create_new(true)` under file-root policy.
pub fn write_new_export_file(export_dir: &Path, suggested: &str, bytes: &[u8]) -> Result<String> {
    let state = State::from_environment()?;
    let root = state.resolve_file(export_dir, FileAccess::Write)?;
    for _ in 0..64 {
        let rel = prepare_relative_path(&root, suggested)?;
        let destination = state.resolve_file(&root.join(&rel), FileAccess::Write)?;
        let parent = destination
            .parent()
            .context("Export file path has no parent directory")?;
        std::fs::create_dir_all(parent).context("Export asset directory could not be created")?;
        match File::options()
            .create_new(true)
            .write(true)
            .open(&destination)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.flush()?;
                file.sync_all()?;
                return Ok(rel);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err).context("Export asset could not be written"),
        }
    }
    bail!("Export asset collided repeatedly; existing files were not overwritten")
}

/// Writes a sanitized UTF-8 contact `.vcard` inside `contacts/` when within `max_bytes`.
pub fn write_contact_vcard(
    export_dir: &Path,
    index: usize,
    vcard: &str,
    max_bytes: u64,
) -> Result<Option<(String, u64)>> {
    let cleaned: String = vcard
        .chars()
        .filter(|&c| {
            matches!(c, '\r' | '\n' | '\t')
                || (!c.is_control() && !crate::text_fidelity::is_unsafe_char(c))
        })
        .collect();
    if cleaned.trim().is_empty() {
        return Ok(None);
    }
    let bytes = cleaned.as_bytes();
    if (bytes.len() as u64) > max_bytes {
        return Ok(None);
    }
    let suggested = format!("contacts/contact_{}.vcard", index.max(1));
    let rel = write_new_export_file(export_dir, &suggested, bytes)?;
    Ok(Some((rel, bytes.len() as u64)))
}

pub(crate) fn directory_is_nonempty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if !path.is_dir() {
        bail!("Export destination must be a directory");
    }
    let mut entries =
        std::fs::read_dir(path).context("Export destination could not be inspected")?;
    Ok(entries.next().transpose()?.is_some())
}

pub(crate) fn chat_export_stamp() -> String {
    format!("ChatExport_{}", chrono::Local::now().format("%Y-%m-%d"))
}

pub(crate) fn next_unused_child(state: &State, parent: &Path) -> Result<PathBuf> {
    let stem = chat_export_stamp();
    let base = parent.join(&stem);
    let mut candidate = base.clone();
    for suffix in 1..=10_000 {
        if !candidate.exists() {
            return state.resolve_file(&candidate, FileAccess::Write);
        }
        candidate = parent.join(format!("{stem}_{suffix}"));
    }
    if !candidate.exists() {
        return state.resolve_file(&candidate, FileAccess::Write);
    }
    bail!("No unused export folder is available")
}

/// Plans the target directory without mutating disk:
/// - Explicit empty or non-existent folder (distinct from default downloads) is used directly.
/// - Occupied folder or default downloads gets a fresh `ChatExport_YYYY-MM-DD` child.
pub fn plan_destination(state: &State, raw_destination: Option<&str>) -> Result<PathBuf> {
    let explicit = raw_destination.map(str::trim).filter(|s| !s.is_empty());
    let default_downloads = state.default_downloads().ok();
    let destination = match explicit {
        Some(raw) => state.resolve_file(Path::new(raw), FileAccess::Write)?,
        None => state.default_downloads()?,
    };
    let is_default = explicit.is_none()
        || default_downloads
            .as_ref()
            .is_some_and(|def| def == &destination);
    if !is_default && !directory_is_nonempty(&destination)? {
        return Ok(destination);
    }
    next_unused_child(state, &destination)
}
