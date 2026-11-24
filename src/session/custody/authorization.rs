use super::*;
use crate::account_manager::{ConfigSnapshot, label_for};
use base64::Engine;
use std::path::PathBuf;
use subtle::{Choice, ConstantTimeEq};
use zeroize::{Zeroize, Zeroizing};

const MAX_AUTHORIZATION_KEYS: usize = 32;

/// Only temporary comparison copies live here. This does not erase original
/// configuration strings, persisted sessions or the session library's storage.
pub(super) struct AuthorizationKeys {
    values: Vec<Zeroizing<[u8; 256]>>,
}
impl AuthorizationKeys {
    fn new() -> Self {
        Self {
            values: Vec::with_capacity(MAX_AUTHORIZATION_KEYS),
        }
    }
    fn push(&mut self, value: Zeroizing<[u8; 256]>) -> Result<()> {
        anyhow::ensure!(
            self.values.len() < MAX_AUTHORIZATION_KEYS,
            "Authorization comparison exceeds its key budget"
        );
        self.values.push(value);
        Ok(())
    }
    pub(super) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub(super) fn intersects(&self, other: &Self) -> bool {
        let mut equal = Choice::from(0);
        // Every pair has the same 256-byte length. Accumulate all comparisons,
        // including pairs after a match; neither contents nor matching position
        // select an early return. Only the bounded key counts affect loop size.
        for left in &self.values {
            for right in &other.values {
                equal |= left[..].ct_eq(&right[..]);
            }
        }
        bool::from(equal)
    }
}

pub(super) fn load(
    state: &State,
    label: &str,
    config: &ConfigSnapshot,
    custody_owner: bool,
) -> Result<AuthorizationKeys> {
    let native = state.read_json(&format!("{label}.session.json"), serde_json::Value::Null)?;
    if !native.is_null() {
        let mut snapshot: Snapshot = serde_json::from_value(native)?;
        // A fresh native authorization is explicitly exempt for its own label,
        // but cannot identify old encryption custody left from an older login.
        if custody_owner && snapshot.authorization_origin == "native" {
            bail!("Fresh native authorization does not identify deferred legacy custody");
        }
        let mut keys = AuthorizationKeys::new();
        for option in &mut snapshot.dc_options {
            if let Some(key) = &mut option.auth_key {
                let comparison = Zeroizing::new(*key);
                key.zeroize();
                keys.push(comparison)?;
            }
        }
        if !keys.is_empty() || !custody_owner {
            return Ok(keys);
        }
    }
    let mut source = None;
    for prefix in ["TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME"] {
        source = config.values.iter().find(|(key, value)| {
            !value.is_empty()
                && key.starts_with(prefix)
                && label_for(key).is_ok_and(|found| found == label)
        });
        if source.is_some() {
            break;
        }
    }
    let Some((key, value)) = source else {
        return Ok(AuthorizationKeys::new());
    };
    if key.starts_with("TELEGRAM_SESSION_STRING") {
        anyhow::ensure!(value.starts_with('1'), "Unsupported StringSession version");
        let bytes = Zeroizing::new(base64::engine::general_purpose::URL_SAFE.decode(&value[1..])?);
        anyhow::ensure!(
            matches!(bytes.len(), 263 | 275) && (1..=5).contains(&bytes[0]),
            "Invalid StringSession shape"
        );
        let mut key = Zeroizing::new([0u8; 256]);
        key.copy_from_slice(&bytes[bytes.len() - 256..]);
        let mut keys = AuthorizationKeys::new();
        keys.push(key)?;
        return Ok(keys);
    }
    let path = source_path(state, config, value)?;
    let mut source = Zeroizing::new(super::super::import::authorization_keys(&path)?);
    let mut keys = AuthorizationKeys::new();
    for raw in source.iter_mut() {
        let comparison = Zeroizing::new(*raw);
        raw.zeroize();
        keys.push(comparison)?;
    }
    Ok(keys)
}

fn source_path(state: &State, config: &ConfigSnapshot, value: &str) -> Result<PathBuf> {
    let base = config
        .path
        .parent()
        .context("Configuration has no parent")?;
    let raw = PathBuf::from(value);
    let mut candidates = vec![base.join(&raw), state.root().join(&raw)];
    candidates.extend(candidates.clone().into_iter().map(|path| {
        let mut name = path.into_os_string();
        name.push(".session");
        PathBuf::from(name)
    }));
    let mut found = candidates
        .into_iter()
        .filter(|path| path.is_file())
        .map(|path| path.canonicalize())
        .collect::<std::io::Result<Vec<_>>>()?;
    found.sort();
    found.dedup();
    anyhow::ensure!(
        found.len() == 1,
        "Legacy authorization source is missing or ambiguous"
    );
    found
        .pop()
        .context("Legacy authorization source is missing")
}
