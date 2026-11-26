use super::{
    ConfigWriteGuard, Publication, config_digest, config_write_lock, confirm_configuration,
    label_for, logical_records, parse_config, process_values, read_bytes,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub fn write_env_value(
    path: &Path,
    key: &str,
    value: &str,
    expected_digest: Option<&str>,
) -> Result<()> {
    let guard = config_write_lock(path)?;
    write_env_value_locked(&guard, key, value, expected_digest).map(|_| ())
}

pub(in crate::account_manager) fn write_env_value_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    value: &str,
    expected_digest: Option<&str>,
) -> Result<Publication> {
    rewrite_env_locked(guard, key, Some(value), expected_digest)
}

pub(in crate::account_manager) fn remove_env_value_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    expected_digest: &str,
) -> Result<Publication> {
    rewrite_env_locked(guard, key, None, Some(expected_digest))
}

pub(in crate::account_manager) fn rename_env_key_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    target: &str,
    expected_digest: &str,
) -> Result<Publication> {
    let (bytes, result) = prepare_env_key_rename(guard, key, target, expected_digest)?;
    persist_configuration(&guard.path, &bytes, &result)
}

pub(in crate::account_manager) fn validate_env_key_rename(
    guard: &ConfigWriteGuard,
    key: &str,
    target: &str,
    expected_digest: &str,
) -> Result<()> {
    prepare_env_key_rename(guard, key, target, expected_digest).map(|_| ())
}

fn prepare_env_key_rename(
    guard: &ConfigWriteGuard,
    key: &str,
    target: &str,
    expected_digest: &str,
) -> Result<(Vec<u8>, String)> {
    label_for(key)?;
    label_for(target)?;
    let metadata = guard.path.symlink_metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Refusing to rename nonregular account configuration");
    }
    let (bytes, _) = read_bytes(&guard.path)?;
    let text = std::str::from_utf8(&bytes).context("Account configuration must be UTF-8")?;
    let parsed = parse_config(text, &BTreeMap::new())?;
    if parsed.get(key).map(|value| config_digest(value)).as_deref() != Some(expected_digest)
        || parsed.contains_key(target)
    {
        bail!("Account configuration changed; no rename was written");
    }
    let mut result = String::new();
    for record in logical_records(text)? {
        let record = rename_references(record, key, target)?;
        let body = record.trim_start_matches('\u{feff}').trim_start();
        let candidate = body.strip_prefix("export ").unwrap_or(body).trim_start();
        if candidate
            .split_once('=')
            .map(|(name, _)| name.trim())
            .unwrap_or(candidate.trim())
            == key
        {
            let offset = record.len() - candidate.len();
            result.push_str(&record[..offset]);
            result.push_str(target);
            result.push_str(&candidate[key.len()..]);
        } else {
            result.push_str(&record);
        }
    }
    let process = process_values();
    let before = parse_config(text, &process)?;
    let after = parse_config(&result, &process)?;
    if before.len() != after.len()
        || before.iter().any(|(name, value)| {
            after.get(if name == key { target } else { name.as_str() }) != Some(value)
        })
    {
        bail!(
            "Configuration rename would change resolved settings; no state or configuration was changed"
        );
    }
    Ok((bytes, result))
}

fn rename_references(record: &str, key: &str, target: &str) -> Result<String> {
    let Some(equal) = record.find('=') else {
        return Ok(record.to_owned());
    };
    if record
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('#')
    {
        return Ok(record.to_owned());
    }
    let mut result = record[..equal + 1].to_owned();
    let value = &record[equal + 1..];
    let mut index = 0;
    let mut started = false;
    let mut quote = None;
    let mut previous = None;
    while index < value.len() {
        let Some(c) = value[index..].chars().next() else {
            break;
        };
        let next_index = index + c.len_utf8();
        let first = !started && !c.is_whitespace();
        if first {
            started = true;
        }
        if c == '#' && quote.is_none() && (first || previous.is_some_and(char::is_whitespace)) {
            result.push_str(&value[index..]);
            break;
        }
        if first && matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if quote == Some(c) {
            quote = None;
        } else if c == '\\'
            && quote.is_some()
            && let Some(next) = value[next_index..].chars().next()
        {
            let escaped = next == '\\'
                || next == '\''
                || quote == Some('"')
                    && matches!(next, '"' | 'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v');
            if escaped {
                result.push(c);
                result.push(next);
                previous = Some(next);
                index = next_index + next.len_utf8();
                continue;
            }
        }
        if c == '$' && quote != Some('\'') && value[next_index..].starts_with('{') {
            let expression_start = next_index + 1;
            let end = value[expression_start..]
                .find('}')
                .map(|end| end + expression_start)
                .context("Incomplete configuration reference")?;
            let expression = &value[expression_start..end];
            let name = expression
                .split_once(":-")
                .map_or(expression, |(name, _)| name);
            result.push_str("${");
            if name == key {
                result.push_str(target);
                result.push_str(&expression[key.len()..]);
            } else {
                result.push_str(expression);
            }
            result.push('}');
            previous = Some('}');
            index = end + 1;
            continue;
        }
        result.push(c);
        previous = Some(c);
        index = next_index;
    }
    Ok(result)
}

fn rewrite_env_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    value: Option<&str>,
    expected_digest: Option<&str>,
) -> Result<Publication> {
    let path = &guard.path;
    label_for(key)?;
    if value.is_some_and(|value| value.contains(['\r', '\n', '\0'])) {
        bail!("An account configuration value must fit on one line");
    }
    if path
        .symlink_metadata()
        .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
    {
        bail!("Refusing to replace nonregular account configuration");
    }
    let parent = path
        .parent()
        .context("Configuration file has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let (bytes, _) = read_bytes(path)?;
    let text = std::str::from_utf8(&bytes).context("Account configuration must be UTF-8")?;
    let process = BTreeMap::new();
    let parsed = parse_config(text, &process)?;
    if let Some(expected) = expected_digest
        && parsed.get(key).map(|v| config_digest(v)).as_deref() != Some(expected)
    {
        bail!("Account configuration changed; no replacement was written");
    }
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let quoted = value.map(|value| {
        if value.contains('$') {
            format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
        } else {
            format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
        }
    });
    let mut matched = false;
    let mut result = String::new();
    for line in logical_records(text)? {
        let body = line.trim_end_matches(['\r', '\n']);
        let candidate = body
            .trim_start_matches('\u{feff}')
            .trim()
            .strip_prefix("export ")
            .unwrap_or(body.trim_start_matches('\u{feff}').trim());
        if candidate
            .split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(candidate)
            .trim()
            == key
        {
            if body.starts_with('\u{feff}') {
                result.push('\u{feff}');
            }
            if let Some(quoted) = &quoted {
                result.push_str(&format!("{key}={quoted}"));
                result.push_str(ending);
            }
            matched = true;
        } else {
            result.push_str(line);
        }
    }
    if !matched && let Some(quoted) = &quoted {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push_str(ending);
        }
        result.push_str(&format!("{key}={quoted}{ending}"));
    }
    persist_configuration(path, &bytes, &result)
}

fn persist_configuration(path: &Path, bytes: &[u8], result: &str) -> Result<Publication> {
    let parent = path
        .parent()
        .context("Configuration file has no parent directory")?;
    let backup_prefix = format!(
        "{}.backup-",
        path.file_name()
            .context("Configuration source has no filename")?
            .to_string_lossy()
    );
    if path.exists() {
        let backup = path.with_file_name(format!(
            "{backup_prefix}{}",
            chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S_%f_UTC")
        ));
        let _ = private_replace(&backup, bytes, false)?;
    }
    let publication = private_replace(path, result.as_bytes(), true)?;
    // Backups hold complete logins, so keep only the newest recoverable revision.
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Ok(publication);
    };
    let mut backups = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&backup_prefix))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| entry.file_name());
    let excess = backups.len().saturating_sub(1);
    for entry in backups.into_iter().take(excess) {
        let _ = std::fs::remove_file(entry.path());
    }
    Ok(publication)
}

fn private_replace(path: &Path, bytes: &[u8], replace: bool) -> Result<Publication> {
    let parent = path.parent().context("Private file has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    crate::state::storage::restrict(temporary.path(), false)?;
    temporary.write_all(bytes)?;
    temporary.as_file_mut().sync_all()?;
    if replace {
        temporary
            .persist(path)
            .map_err(|_| anyhow::anyhow!("Cannot atomically replace account configuration"))?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|_| anyhow::anyhow!("Cannot preserve account configuration backup"))?;
    }
    Ok(confirm_configuration(path))
}
