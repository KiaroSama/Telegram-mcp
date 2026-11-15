mod locking;
use anyhow::{Context, Result, bail};
pub(super) use locking::{ConfigWriteGuard, lock as config_write_lock};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static PROCESS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
static SOURCE: OnceLock<PathBuf> = OnceLock::new();
static ACCEPTED: Mutex<bool> = Mutex::new(false);

pub fn select_source(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if let Some(existing) = SOURCE.get() {
        if existing != &absolute {
            bail!("The account configuration source is already fixed for this process");
        }
        return Ok(());
    }
    SOURCE.set(absolute).map_err(|_| {
        anyhow::anyhow!("The account configuration source was initialized concurrently")
    })?;
    Ok(())
}

pub struct ConfigSnapshot {
    pub values: BTreeMap<String, String>,
    pub digest: String,
    pub path: PathBuf,
    pub process_keys: BTreeSet<String>,
}

pub fn capture_process_config() {
    let _ = PROCESS.set(std::env::vars().collect());
    if let Ok(cwd) = std::env::current_dir() {
        let _ = SOURCE.set(cwd.join(".env"));
    }
}

pub fn config_digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

pub fn label_for(key: &str) -> Result<String> {
    for prefix in ["TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME"] {
        if key == prefix {
            return Ok("default".to_owned());
        }
        if let Some(label) = key.strip_prefix(&format!("{prefix}_")) {
            if label.is_empty()
                || !label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                bail!("Invalid account configuration key");
            }
            return Ok(label.to_ascii_lowercase());
        }
    }
    bail!("Not an account configuration key")
}

pub fn current_config() -> Result<ConfigSnapshot> {
    if PROCESS.get().is_none() {
        capture_process_config();
    }
    let path = SOURCE
        .get()
        .context("Configuration source has not been initialized")?;
    let mut accepted = ACCEPTED
        .lock()
        .map_err(|_| anyhow::anyhow!("Configuration source lock is unavailable"))?;
    let (snapshot, present) = read_config_revision(path)?;
    if *accepted && !present {
        bail!("The accepted .env file is missing; active configuration was preserved");
    }
    *accepted |= present;
    Ok(snapshot)
}

pub fn read_config(path: &Path) -> Result<ConfigSnapshot> {
    Ok(read_config_revision(path)?.0)
}

fn read_bytes(path: &Path) -> Result<(Vec<u8>, bool)> {
    let (raw, present) = match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .context("Cannot read account configuration; active accounts were preserved")?;
            (bytes, true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), false),
        Err(_) => bail!("Cannot read account configuration; active accounts were preserved"),
    };
    if raw.len() > 8 * 1024 * 1024 {
        bail!("Account configuration exceeds its byte budget");
    }
    Ok((raw, present))
}

pub(super) fn file_values(path: &Path) -> Result<BTreeMap<String, String>> {
    let (raw, _) = read_bytes(path)?;
    parse_config(
        std::str::from_utf8(&raw).context("Account configuration must be UTF-8")?,
        &BTreeMap::new(),
    )
}

fn read_config_revision(path: &Path) -> Result<(ConfigSnapshot, bool)> {
    let (raw, present) = read_bytes(path)?;
    let process = PROCESS
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::vars().collect());
    let text = std::str::from_utf8(&raw).context("Account configuration must be UTF-8")?;
    let values = parse_config(text, &process)?;
    let process_keys = process
        .keys()
        .filter(|key| key.starts_with("TELEGRAM_SESSION_"))
        .cloned()
        .collect();
    Ok((
        ConfigSnapshot {
            values,
            digest: format!("{:x}", Sha256::digest(&raw)),
            path: path.to_owned(),
            process_keys,
        },
        present,
    ))
}

/// Parse one immutable revision, using startup process values for interpolation.
/// Never consult the already-merged environment during a reload.
pub fn parse_config(
    text: &str,
    process: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut values = BTreeMap::new();
    for record in logical_records(text)? {
        parse_line(record, process, &mut values)?;
    }
    for (key, value) in process {
        if key.starts_with("TELEGRAM_") || matches!(key.as_str(), "XDG_STATE_HOME" | "LOCALAPPDATA")
        {
            values.insert(key.clone(), value.clone());
        }
    }
    Ok(values)
}

fn logical_records(text: &str) -> Result<Vec<&str>> {
    let mut records = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut equal = false;
    let mut value_started = false;
    for (index, c) in text.char_indices() {
        if c == '\n' && quote.is_none() {
            records.push(&text[start..index + 1]);
            start = index + 1;
            comment = false;
            escaped = false;
            equal = false;
            value_started = false;
            continue;
        }
        if comment {
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(open) = quote {
            if c == open {
                quote = None;
            }
            continue;
        }
        if c == '#' {
            comment = true;
            continue;
        }
        if !equal {
            if c == '=' {
                equal = true;
            }
            continue;
        }
        if !value_started && !c.is_whitespace() {
            value_started = true;
            if matches!(c, '\'' | '"') {
                quote = Some(c);
            }
        }
    }
    if quote.is_some() {
        bail!("Account configuration contains an unterminated quote");
    }
    if start < text.len() {
        records.push(&text[start..]);
    }
    Ok(records)
}

fn parse_line(
    line: &str,
    process: &BTreeMap<String, String>,
    values: &mut BTreeMap<String, String>,
) -> Result<()> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(());
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let (key, raw) = line.split_once('=').unwrap_or((line, ""));
    let key = key.trim();
    if key.is_empty()
        || !key
            .bytes()
            .enumerate()
            .all(|(i, c)| c.is_ascii_alphabetic() || c == b'_' || i > 0 && c.is_ascii_digit())
    {
        bail!("Account configuration contains an invalid key");
    }
    if values.contains_key(key) {
        bail!("Account configuration contains duplicate keys");
    }
    let mut result = String::new();
    let mut keep = 0;
    let mut quote = None;
    let chars: Vec<_> = raw.trim().chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '#' && quote.is_none() && (i == 0 || chars[i - 1].is_whitespace()) {
            break;
        }
        if matches!(c, '\'' | '"') {
            if quote == Some(c) {
                quote = None;
                i += 1;
                continue;
            }
            if quote.is_none() && i == 0 {
                quote = Some(c);
                i += 1;
                continue;
            }
        }
        if c == '\\'
            && quote.is_some()
            && let Some(next) = chars.get(i + 1).copied()
        {
            let decoded = match (quote, next) {
                (_, '\\') => Some('\\'),
                (Some('\''), '\'') => Some('\''),
                (Some('"'), '\'') => Some('\''),
                (Some('"'), '"') => Some('"'),
                (Some('"'), 'a') => Some('\u{7}'),
                (Some('"'), 'b') => Some('\u{8}'),
                (Some('"'), 'f') => Some('\u{c}'),
                (Some('"'), 'n') => Some('\n'),
                (Some('"'), 'r') => Some('\r'),
                (Some('"'), 't') => Some('\t'),
                (Some('"'), 'v') => Some('\u{b}'),
                _ => None,
            };
            if let Some(decoded) = decoded {
                result.push(decoded);
                keep = result.len();
                i += 2;
                continue;
            }
        }
        if c == '$' && quote != Some('\'') {
            let start = i + 1;
            let mut end = start;
            if chars.get(start) == Some(&'{') {
                end = start + 1;
                while end < chars.len() && chars[end] != '}' {
                    end += 1;
                }
                if end >= chars.len() {
                    bail!("Account configuration contains an incomplete interpolation");
                }
                let expression: String = chars[start + 1..end].iter().collect();
                let (name, fallback) = expression.split_once(":-").unwrap_or((&expression, ""));
                result.push_str(
                    process
                        .get(name)
                        .or_else(|| values.get(name))
                        .map(String::as_str)
                        .filter(|v| !v.is_empty())
                        .unwrap_or(fallback),
                );
                keep = result.len();
                i = end + 1;
                continue;
            }
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            if end > start {
                let name: String = chars[start..end].iter().collect();
                result.push_str(
                    process
                        .get(&name)
                        .or_else(|| values.get(&name))
                        .map(String::as_str)
                        .unwrap_or(""),
                );
                keep = result.len();
                i = end;
                continue;
            }
        }
        result.push(c);
        if quote.is_some() || !c.is_whitespace() {
            keep = result.len();
        }
        i += 1;
    }
    if quote.is_some() {
        bail!("Account configuration contains an unterminated quote");
    }
    result.truncate(keep);
    values.insert(key.to_owned(), result);
    Ok(())
}

pub fn write_env_value(
    path: &Path,
    key: &str,
    value: &str,
    expected_digest: Option<&str>,
) -> Result<()> {
    let guard = config_write_lock(path)?;
    write_env_value_locked(&guard, key, value, expected_digest)
}

pub(super) fn write_env_value_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    value: &str,
    expected_digest: Option<&str>,
) -> Result<()> {
    let path = &guard.path;
    label_for(key)?;
    if value.contains(['\r', '\n', '\0']) {
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
    let quoted = if value.contains('$') {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    };
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
            result.push_str(&format!("{key}={quoted}"));
            result.push_str(ending);
            matched = true;
        } else {
            result.push_str(line);
        }
    }
    if !matched {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push_str(ending);
        }
        result.push_str(&format!("{key}={quoted}{ending}"));
    }
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
        private_replace(&backup, &bytes, false)?;
    }
    private_replace(path, result.as_bytes(), true)?;
    // Backups hold complete logins, so keep only the newest recoverable revision.
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Ok(());
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
    Ok(())
}

fn private_replace(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
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
    #[cfg(unix)]
    if std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .is_err()
    {
        tracing::warn!(
            "Configuration replacement committed; directory durability could not be confirmed"
        );
    }
    Ok(())
}
