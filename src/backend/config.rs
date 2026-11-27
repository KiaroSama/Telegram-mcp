use crate::{session::validate_label, state::State};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ClientIdentity {
    pub device_model: Option<String>,
    pub system_version: Option<String>,
    pub app_version: Option<String>,
}

fn validate_identity_field(name: &str, raw: Option<&str>) -> Result<Option<String>> {
    let Some(value) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    ensure!(
        value.len() <= 256 && !value.chars().any(char::is_control),
        "{name} contains unsupported characters or exceeds 256 bytes"
    );
    Ok(Some(value.to_owned()))
}

impl ClientIdentity {
    pub fn from_snapshot(snapshot: &crate::account_manager::ConfigSnapshot) -> Result<Self> {
        let get = |key: &str| {
            snapshot
                .values
                .get(key)
                .cloned()
                .or_else(|| std::env::var(key).ok())
        };
        Ok(Self {
            device_model: validate_identity_field(
                "TELEGRAM_DEVICE_MODEL",
                get("TELEGRAM_DEVICE_MODEL").as_deref(),
            )?,
            system_version: validate_identity_field(
                "TELEGRAM_SYSTEM_VERSION",
                get("TELEGRAM_SYSTEM_VERSION").as_deref(),
            )?,
            app_version: validate_identity_field(
                "TELEGRAM_APP_VERSION",
                get("TELEGRAM_APP_VERSION").as_deref(),
            )?,
        })
    }

    pub fn apply_to(&self, params: &mut grammers_mtsender::ConnectionParams) {
        if let Some(model) = &self.device_model {
            params.device_model = model.clone();
        }
        if let Some(system) = &self.system_version {
            params.system_version = system.clone();
        }
        if let Some(app) = &self.app_version {
            params.app_version = app.clone();
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PooledSession {
    pub index: usize,
    pub slot_label: String,
    pub auth_identity: String,
    pub value: String,
}

/// Derive a private canonical auth-key fingerprint for deduplicating session strings
/// without exposing key material.
pub fn session_auth_key_identity(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(body) = trimmed.strip_prefix('1') {
        let decoded = base64::engine::general_purpose::URL_SAFE
            .decode(body)
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body))
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(body));
        if let Ok(bytes) = decoded
            && (bytes.len() == 263 || bytes.len() == 275)
        {
            let auth_key = &bytes[bytes.len() - 256..];
            return crate::lowercase_hex(&Sha256::digest(auth_key));
        }
    }
    crate::lowercase_hex(&Sha256::digest(trimmed.as_bytes()))
}

/// Parse `TELEGRAM_SESSION_STRINGS` (whitespace/comma/semicolon separated) into an
/// ordered deduplicated pool, dropping collisions with named sessions or earlier items.
pub fn pooled_sessions(snapshot: &crate::account_manager::ConfigSnapshot) -> Vec<PooledSession> {
    let mut seen = BTreeSet::new();
    for (key, value) in &snapshot.values {
        if !value.trim().is_empty()
            && (key == "TELEGRAM_SESSION_STRING" || key.starts_with("TELEGRAM_SESSION_STRING_"))
        {
            seen.insert(session_auth_key_identity(value));
        }
    }
    let Some(raw) = snapshot
        .values
        .get("TELEGRAM_SESSION_STRINGS")
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for token in raw
        .split(|c: char| c.is_ascii_whitespace() || c == ',' || c == ';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let auth_identity = session_auth_key_identity(token);
        if !seen.insert(auth_identity.clone()) {
            continue;
        }
        let slot_label = format!("pool_{}", &auth_identity[..12]);
        let index = out.len();
        out.push(PooledSession {
            index,
            slot_label,
            auth_identity,
            value: token.to_owned(),
        });
    }
    out
}

/// A source revision is always a SHA-256 digest, never arbitrary source text.
/// Keeping this representation separate prevents serialization from exposing
/// the legacy session value when the public report gains fields in the future.
struct ConfigurationRevision([u8; 32]);
impl Serialize for ConfigurationRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&crate::lowercase_hex(&self.0))
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
                    secret_chat_available: true,
                },
            );
        }
    }
    let pool = pooled_sessions(snapshot);
    if !entries.contains_key("default") && !pool.is_empty() {
        let text = &snapshot.values["TELEGRAM_SESSION_STRINGS"];
        entries.insert(
            "default".into(),
            PublicConfigurationEntry {
                label: "default".into(),
                selected_label: "default".into(),
                config_key: "TELEGRAM_SESSION_STRINGS".into(),
                revision: ConfigurationRevision(Sha256::digest(text.as_bytes()).into()),
                native_session: pool.iter().any(|candidate| {
                    root.join(format!("{}.session.json", candidate.slot_label))
                        .is_file()
                }),
                source: "configuration",
                backend: "grammers",
                secret_chat_available: true,
            },
        );
    }
    Ok(PublicConfigurationReport {
        entries: entries.into_values().collect(),
        backend: "grammers",
        secret_chat_available: true,
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
    if label == "default" && !pooled_sessions(snapshot).is_empty() {
        return Some((
            "TELEGRAM_SESSION_STRINGS".into(),
            snapshot.values["TELEGRAM_SESSION_STRINGS"].clone(),
        ));
    }
    None
}
pub fn generation(label: &str) -> Result<Option<String>> {
    Ok(source(label)?.map(|(key, value)| {
        format!(
            "{key}:{}",
            crate::lowercase_hex(&Sha256::digest(value.as_bytes()))
        )
    }))
}
pub fn account_labels() -> Result<Vec<String>> {
    let snapshot = crate::account_manager::current_config()?;
    let mut labels = BTreeSet::new();
    for (key, value) in &snapshot.values {
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
    if !pooled_sessions(&snapshot).is_empty() {
        labels.insert("default".into());
    }
    Ok(labels
        .into_iter()
        .filter(|label| !label.starts_with("__login_"))
        .collect())
}
pub fn accounts_snapshot() -> Result<Value> {
    Ok(serde_json::to_value(current_configuration_report()?)?)
}

/// Enrich configured accounts with live `get_me()` identity for already-connected clients
/// without opening auxiliary logins. Reports `unavailable` separately from `disconnected`.
pub async fn live_accounts_snapshot(
    connected: &[(String, Arc<dyn crate::backend::Telegram>)],
) -> Result<Value> {
    let mut report = accounts_snapshot()?;
    let Some(entries) = report["accounts"].as_array_mut() else {
        return Ok(report);
    };
    let map: BTreeMap<&str, &Arc<dyn crate::backend::Telegram>> = connected
        .iter()
        .map(|(label, api)| (label.as_str(), api))
        .collect();
    for entry in entries.iter_mut() {
        let label = entry["label"].as_str().unwrap_or("");
        if let Some(api) = map.get(label) {
            entry["connected"] = json!(true);
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                api.invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]})),
            )
            .await
            {
                Ok(Ok(Value::Array(users))) => {
                    if let Some(me) = users.first() {
                        let first = me["first_name"].as_str();
                        let last = me["last_name"].as_str();
                        let display = crate::sanitize::full_name(first, last);
                        entry["status"] = json!("connected");
                        entry["user_id"] = me["id"].clone();
                        entry["display_name"] = json!(display);
                        entry["username"] = me["username"].clone();
                    } else {
                        entry["status"] = json!("unavailable");
                    }
                }
                _ => {
                    entry["status"] = json!("unavailable");
                }
            }
        } else {
            entry["connected"] = json!(false);
            entry["status"] = json!("disconnected");
        }
    }
    Ok(report)
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
