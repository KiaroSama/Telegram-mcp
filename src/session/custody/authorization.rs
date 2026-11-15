use super::*;
use crate::account_manager::{ConfigSnapshot, label_for};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::PathBuf};

pub(super) fn fingerprints(
    state: &State,
    label: &str,
    config: &ConfigSnapshot,
    custody_owner: bool,
) -> Result<BTreeSet<[u8; 32]>> {
    let native = state.read_json(&format!("{label}.session.json"), serde_json::Value::Null)?;
    if !native.is_null() {
        let snapshot: Snapshot = serde_json::from_value(native)?;
        // A fresh native authorization is explicitly exempt for its own label,
        // but cannot identify old encryption custody left from an older login.
        if custody_owner && snapshot.authorization_origin == "native" {
            bail!("Fresh native authorization does not identify deferred legacy custody");
        }
        let keys = snapshot
            .dc_options
            .iter()
            .filter_map(|dc| dc.auth_key)
            .map(digest)
            .collect::<BTreeSet<_>>();
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
        return Ok(BTreeSet::new());
    };
    if key.starts_with("TELEGRAM_SESSION_STRING") {
        anyhow::ensure!(value.starts_with('1'), "Unsupported StringSession version");
        let bytes = base64::engine::general_purpose::URL_SAFE.decode(&value[1..])?;
        anyhow::ensure!(
            matches!(bytes.len(), 263 | 275) && (1..=5).contains(&bytes[0]),
            "Invalid StringSession shape"
        );
        let key: [u8; 256] = bytes[bytes.len() - 256..].try_into()?;
        return Ok(BTreeSet::from([digest(key)]));
    }
    let path = source_path(state, config, value)?;
    Ok(super::super::import::authorization_keys(&path)?
        .into_iter()
        .map(digest)
        .collect())
}

fn digest(key: [u8; 256]) -> [u8; 32] {
    Sha256::digest(key).into()
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
