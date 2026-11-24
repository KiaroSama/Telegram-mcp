use anyhow::{Context, Result, bail};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

use super::super::config::{config_write_lock, file_values, write_env_value_locked};
use super::super::{config_digest, label_for, parse_config, read_config};
use crate::state::storage::{private_directory, restrict, sync_directory};

const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

pub(super) struct Account {
    pub label: String,
    pub key: String,
    pub digest: String,
}

fn read_regular(path: &Path) -> Result<Option<String>> {
    match path.symlink_metadata() {
        Ok(info) if info.is_file() && !info.file_type().is_symlink() => {}
        Ok(_) => bail!("Expected a regular private file"),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        bail!("Private file exceeds its byte budget");
    }
    Ok(Some(String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!("Private file must be UTF-8")
    })?))
}

pub(super) fn accounts(path: &Path) -> Result<Vec<Account>> {
    let Some(text) = read_regular(path)? else {
        return Ok(Vec::new());
    };
    // The native parser determines whether a key is active; line order is for display only.
    let values = parse_config(&text, &BTreeMap::new())?;
    let mut accounts: Vec<Account> = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim_start();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let Ok(label) = label_for(key) else {
            continue;
        };
        let Some(value) = values.get(key).filter(|value| !value.is_empty()) else {
            continue;
        };
        let account = Account {
            label,
            key: key.to_owned(),
            digest: config_digest(value),
        };
        if let Some(previous) = accounts
            .iter_mut()
            .find(|previous| previous.label == account.label)
        {
            if key != "TELEGRAM_SESSION_STRING" && key != "TELEGRAM_SESSION_NAME" {
                *previous = account;
            }
        } else {
            accounts.push(account);
        }
    }
    Ok(accounts)
}

pub(super) fn key_for(prefix: &str, label: &str) -> String {
    if label == "default" {
        prefix.to_owned()
    } else {
        format!("{prefix}_{}", label.to_ascii_uppercase())
    }
}

pub(super) fn import_session(
    path: &Path,
    label: &str,
    existing: Option<&Account>,
    session: &str,
) -> Result<()> {
    let guard = config_write_lock(path)?;
    let snapshot = read_config(&guard.path)?;
    if snapshot
        .process_keys
        .iter()
        .any(|key| label_for(key).is_ok_and(|owner| owner == label))
    {
        bail!("This account is owned by the process environment; change it there first");
    }
    let configured = file_values(&guard.path)?;
    let mut matching = configured.iter().filter(|(key, value)| {
        !value.is_empty() && label_for(key).is_ok_and(|owner| owner == label)
    });
    let key = match (existing, matching.next()) {
        (None, None) => key_for("TELEGRAM_SESSION_STRING", label),
        (Some(existing), Some((key, _))) if &existing.key == key && matching.next().is_none() => {
            if !key.starts_with("TELEGRAM_SESSION_STRING") {
                bail!("Use native login to replace a native authorization");
            }
            key.clone()
        }
        _ => bail!("The account configuration changed or has multiple keys; nothing was written"),
    };
    let publication = write_env_value_locked(
        &guard,
        &key,
        session,
        existing.map(|account| account.digest.as_str()),
    )?;
    if publication == super::super::config::Publication::Uncertain {
        bail!("The import is visible, but its durability could not be confirmed");
    }
    Ok(())
}

pub(super) fn create_config(path: &Path) -> Result<()> {
    let guard = config_write_lock(path)?;
    if read_regular(&guard.path)?.is_none() {
        write_private(&guard.path, b"", false)?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path
        .parent()
        .context("Private file has no parent directory")?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    restrict(temporary.path(), false)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    let result = if replace {
        temporary.persist(path)
    } else {
        temporary.persist_noclobber(path)
    };
    result.map_err(|_| anyhow::anyhow!("Could not publish the private file atomically"))?;
    sync_directory(parent)
        .context("Private file was published but directory synchronization failed")?;
    Ok(())
}

pub(super) fn rename_registry(
    path: &Path,
    old_key: &str,
    new_key: &str,
    old_label: &str,
    new_label: &str,
) -> Result<bool> {
    let Some(text) = read_regular(path)? else {
        return Ok(false);
    };
    let Some(renamed) = rename_registry_section(&text, old_key, new_key, old_label, new_label)?
    else {
        return Ok(false);
    };
    if read_regular(path)?.as_deref() != Some(text.as_str()) {
        bail!("The registry changed during rename");
    }
    write_private(path, renamed.as_bytes(), true)?;
    Ok(true)
}

fn rename_registry_section(
    text: &str,
    old_key: &str,
    new_key: &str,
    old_label: &str,
    new_label: &str,
) -> Result<Option<String>> {
    let bom = usize::from(text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let mut first = None;
    let mut end = text.len();
    let mut offset = bom;
    for line in text[bom..].split_inclusive('\n') {
        let heading = line
            .strip_prefix("##")
            .filter(|rest| rest.starts_with([' ', '\t']));
        if let Some(key) = heading {
            let key = key.trim();
            if key == new_key || (key == old_key && first.is_some()) {
                bail!("The registry already contains a matching account heading");
            }
            if key == old_key {
                let body = line.trim_end_matches(['\r', '\n']);
                first = Some((offset, offset + body.len()));
            } else if first.is_some() && end == text.len() {
                end = offset;
            }
        }
        offset += line.len();
    }
    Ok(first.map(|(start, after_heading)| {
        let section = text[after_heading..end].replace(
            &format!("account '{old_label}'"),
            &format!("account '{new_label}'"),
        );
        format!("{}## {new_key}{section}{}", &text[..start], &text[end..])
    }))
}

fn matching_files(parent: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

pub(super) fn latest_backup(path: &Path) -> Option<OsString> {
    let prefix = format!("{}.backup-", path.file_name()?.to_string_lossy());
    let path = matching_files(path.parent()?, &prefix, "").pop()?;
    Some(path.file_name()?.to_owned())
}

pub(super) struct RunLog(RefCell<Option<File>>);

impl RunLog {
    pub fn open(root: &Path) -> Self {
        match Self::create(&root.join("logs")) {
            Ok(file) => Self(RefCell::new(Some(file))),
            Err(_) => {
                eprintln!("Account manager file logging is unavailable.");
                Self(RefCell::new(None))
            }
        }
    }

    fn create(directory: &Path) -> Result<File> {
        private_directory(directory)?;
        let stamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S_UTC");
        for suffix in 0..100 {
            let path = if suffix == 0 {
                directory.join(format!("Manage-Accounts_{stamp}.log"))
            } else {
                directory.join(format!("Manage-Accounts_{stamp}_{suffix:02}.log"))
            };
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    if let Err(error) = restrict(&path, false) {
                        drop(file);
                        let _ = fs::remove_file(&path);
                        return Err(error);
                    }
                    let mut logs = matching_files(directory, "Manage-Accounts_", ".log");
                    logs.sort_by_key(|item| item == &path);
                    for old in &logs[..logs.len().saturating_sub(10)] {
                        let _ = fs::remove_file(old);
                    }
                    eprintln!("Log: {}", path.display());
                    return Ok(file);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        bail!("Too many account manager logs share one timestamp")
    }

    pub fn record(&self, level: &str, message: &str) {
        let mut slot = self.0.borrow_mut();
        let Some(file) = slot.as_mut() else { return };
        if file
            .metadata()
            .is_ok_and(|metadata| metadata.len() < 1024 * 1024)
        {
            let stamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
            if writeln!(file, "[{stamp}] [{level}] [accounts] {message}").is_err() {
                eprintln!("Account manager file logging stopped after a write error.");
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_preserves_bom_newlines_values_and_unrelated_sections() {
        let text = "\u{feff}## OLD  \r\naccount 'old': café\r\n## UNRELATED\r\naccount 'old'\r\n";
        let updated = rename_registry_section(text, "OLD", "NEW", "old", "new")
            .unwrap()
            .unwrap();
        assert_eq!(
            updated,
            "\u{feff}## NEW\r\naccount 'new': café\r\n## UNRELATED\r\naccount 'old'\r\n"
        );
        assert!(
            rename_registry_section(text, "MISSING", "NEW", "old", "new")
                .unwrap()
                .is_none()
        );
        assert!(rename_registry_section(text, "OLD", "UNRELATED", "old", "new").is_err());
    }
}
