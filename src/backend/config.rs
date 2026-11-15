use crate::{session::validate_label, state::State};
use anyhow::{Context, Result, bail};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// A source revision is always a SHA-256 digest, never arbitrary source text.
/// Keeping this representation separate prevents serialization from exposing
/// the legacy session value when the public report gains fields in the future.
struct ConfigurationRevision([u8; 32]);
impl Serialize for ConfigurationRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push(DIGITS[(byte >> 4) as usize] as char);
            hex.push(DIGITS[(byte & 15) as usize] as char);
        }
        serializer.serialize_str(&hex)
    }
}
#[derive(Serialize)]
struct PublicConfigurationEntry {
    label: String,
    #[serde(rename = "account")]
    selected_label: String,
    config_key: String,
    #[serde(rename = "config_digest")]
    revision: ConfigurationRevision,
    native_session: bool,
    source: &'static str,
    backend: &'static str,
    secret_chat_available: bool,
}
#[derive(Serialize)]
struct PublicConfigurationReport {
    #[serde(rename = "accounts")]
    entries: Vec<PublicConfigurationEntry>,
    backend: &'static str,
    secret_chat_available: bool,
}

/// An allowlisted projection from one immutable configuration revision. Raw
/// credentials and the configuration map cannot be carried in the output DTO.
fn configuration_report(
    snapshot: &crate::account_manager::ConfigSnapshot,
    root: &Path,
) -> Result<PublicConfigurationReport> {
    let mut entries = BTreeMap::new();
    // Match Native's source preference without rereading configuration between
    // discovering a label and calculating its source revision.
    for prefix in ["TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME"] {
        for (key, text) in &snapshot.values {
            if text.is_empty() || !(key == prefix || key.starts_with(&format!("{prefix}_"))) {
                continue;
            }
            let label = crate::account_manager::label_for(key)?;
            validate_label(&label)?;
            if label.starts_with("__login_") || entries.contains_key(&label) {
                continue;
            }
            let revision = ConfigurationRevision(Sha256::digest(text.as_bytes()).into());
            entries.insert(
                label.clone(),
                PublicConfigurationEntry {
                    native_session: root.join(format!("{label}.session.json")).is_file(),
                    selected_label: label.clone(),
                    label,
                    config_key: key.clone(),
                    revision,
                    source: "configuration",
                    backend: "grammers",
                    secret_chat_available: false,
                },
            );
        }
    }
    Ok(PublicConfigurationReport {
        entries: entries.into_values().collect(),
        backend: "grammers",
        secret_chat_available: false,
    })
}
fn current_configuration_report() -> Result<PublicConfigurationReport> {
    let root = State::global()?;
    crate::account_manager::recover_rename(&root)?;
    crate::account_manager::recover_logins(&root)?;
    let snapshot = crate::account_manager::current_config()?;
    configuration_report(&snapshot, root.root())
}
/// Explicit CLI output containing only public configuration metadata. This
/// serializes a closed DTO rather than a generic credential-bearing map.
pub fn public_configuration_json() -> Result<String> {
    Ok(serde_json::to_string(&current_configuration_report()?)?)
}

pub(super) fn api_id(snapshot: &crate::account_manager::ConfigSnapshot) -> Result<i32> {
    let id = snapshot
        .values
        .get("TELEGRAM_API_ID")
        .context("TELEGRAM_API_ID is required; obtain it from my.telegram.org/apps")?
        .parse::<i32>()
        .context("TELEGRAM_API_ID must be a positive integer")?;
    if id <= 0 {
        bail!("TELEGRAM_API_ID must be positive");
    }
    Ok(id)
}
pub fn source(label: &str) -> Result<Option<(String, String)>> {
    let snapshot = crate::account_manager::current_config()?;
    Ok(source_from(&snapshot, label))
}
pub(super) fn source_from(
    snapshot: &crate::account_manager::ConfigSnapshot,
    label: &str,
) -> Option<(String, String)> {
    for prefix in ["TELEGRAM_SESSION_STRING", "TELEGRAM_SESSION_NAME"] {
        for (key, value) in &snapshot.values {
            if key.starts_with(prefix)
                && !value.is_empty()
                && crate::account_manager::label_for(key).is_ok_and(|found| found == label)
            {
                return Some((key.clone(), value.clone()));
            }
        }
    }
    None
}
pub fn generation(label: &str) -> Result<Option<String>> {
    Ok(source(label)?.map(|(key, value)| format!("{key}:{:x}", Sha256::digest(value.as_bytes()))))
}
pub fn account_labels() -> Result<Vec<String>> {
    let mut labels = BTreeSet::new();
    for (key, value) in crate::account_manager::current_config()?.values {
        if value.is_empty() {
            continue;
        }
        if key == "TELEGRAM_SESSION_STRING" || key == "TELEGRAM_SESSION_NAME" {
            labels.insert("default".into());
        }
        for prefix in ["TELEGRAM_SESSION_STRING_", "TELEGRAM_SESSION_NAME_"] {
            if let Some(label) = key.strip_prefix(prefix) {
                let label = label.to_ascii_lowercase();
                validate_label(&label)?;
                labels.insert(label);
            }
        }
    }
    Ok(labels
        .into_iter()
        .filter(|label| !label.starts_with("__login_"))
        .collect())
}
pub fn accounts_snapshot() -> Result<Value> {
    Ok(serde_json::to_value(current_configuration_report()?)?)
}

#[cfg(test)]
#[path = "../../tests/unit/backend_config.rs"]
mod tests;
pub fn health(account: Option<&str>) -> Result<Value> {
    let _state = State::global()?;
    if let Some(label) = account {
        validate_label(label)?;
    }
    Ok(
        json!({"ready":true,"backend":"grammers","state_accessible":true,"network_authorization":"not_probed","account":account,"version":env!("CARGO_PKG_VERSION")}),
    )
}
