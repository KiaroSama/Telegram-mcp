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

/// The replacement is visible in both cases; only Durable permits discarding
/// recovery intent that spans the configuration and another filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(super) enum Publication {
    Durable,
    Uncertain,
}

#[cfg(test)]
thread_local! {
    static FAILED_SYNC: std::cell::RefCell<Option<SyncFault>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
struct SyncFault {
    path: PathBuf,
    skip: usize,
}
#[cfg(test)]
struct SyncFailureGuard {
    previous: Option<SyncFault>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
#[cfg(test)]
impl Drop for SyncFailureGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = FAILED_SYNC.try_with(|slot| slot.replace(previous));
    }
}
/// A scoped, thread-local fault at one configuration publication or directory sync.
#[cfg(test)]
pub(crate) fn fail_configuration_sync_for_test(path: &Path) -> impl Drop {
    fail_configuration_sync_after_for_test(path, 0)
}

#[cfg(test)]
pub(crate) fn fail_configuration_sync_after_for_test(path: &Path, skip: usize) -> impl Drop {
    let path = std::path::absolute(path).expect("Synthetic configuration path must be absolute");
    let previous = FAILED_SYNC.with(|slot| slot.replace(Some(SyncFault { path, skip })));
    SyncFailureGuard {
        previous,
        _thread: std::marker::PhantomData,
    }
}

pub(super) fn confirm_configuration(path: &Path) -> Publication {
    let sync = || -> std::io::Result<()> {
        check_sync_fault(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("Configuration source has no parent"))?;
        sync_directory(parent)
    };
    if sync().is_ok() {
        Publication::Durable
    } else {
        tracing::warn!(
            "Configuration replacement is visible; directory durability remains unconfirmed"
        );
        Publication::Uncertain
    }
}

fn check_sync_fault(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if FAILED_SYNC.with(|slot| {
        let mut fault = slot.borrow_mut();
        let Some(fault) = fault.as_mut().filter(|fault| fault.path == path) else {
            return false;
        };
        if fault.skip == 0 {
            true
        } else {
            fault.skip -= 1;
            false
        }
    }) {
        return Err(std::io::Error::other(
            "Synthetic account directory sync failure",
        ));
    }
    let _ = path;
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> std::io::Result<()> {
    check_sync_fault(path)?;
    crate::state::storage::sync_directory(path)
}

fn process_values() -> BTreeMap<String, String> {
    PROCESS
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::vars().collect())
}

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

pub(super) fn state_owner(state: &crate::state::State) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        state.root().as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes = {
        use std::os::windows::ffi::OsStrExt;
        state
            .root()
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    format!("{:x}", Sha256::digest(bytes))
}

/// Resolve the shared service address with the same file and process precedence.
pub fn bind_address() -> Result<std::net::SocketAddr> {
    let snapshot = current_config()?;
    let value = |key: &str| {
        PROCESS
            .get()
            .and_then(|values| values.get(key))
            .or_else(|| snapshot.values.get(key))
            .cloned()
    };
    let host = value("MCP_HOST").unwrap_or_else(|| "127.0.0.1".into());
    let port = value("MCP_PORT").unwrap_or_else(|| "8765".into());
    Ok(std::net::SocketAddr::new(
        host.parse().context("MCP_HOST must be an IP address")?,
        port.parse().context("MCP_PORT must be a valid port")?,
    ))
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
    let process = process_values();
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
            if chars.get(start) == Some(&'{') {
                let mut end = start + 1;
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
    write_env_value_locked(&guard, key, value, expected_digest).map(|_| ())
}

pub(super) fn write_env_value_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    value: &str,
    expected_digest: Option<&str>,
) -> Result<Publication> {
    rewrite_env_locked(guard, key, Some(value), expected_digest)
}

pub(super) fn remove_env_value_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    expected_digest: &str,
) -> Result<Publication> {
    rewrite_env_locked(guard, key, None, Some(expected_digest))
}

pub(super) fn rename_env_key_locked(
    guard: &ConfigWriteGuard,
    key: &str,
    target: &str,
    expected_digest: &str,
) -> Result<Publication> {
    let (bytes, result) = prepare_env_key_rename(guard, key, target, expected_digest)?;
    persist_configuration(&guard.path, &bytes, &result)
}

pub(super) fn validate_env_key_rename(
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
        let c = value[index..].chars().next().expect("Character boundary");
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
