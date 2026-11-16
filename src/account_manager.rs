//! Account configuration provenance and owner-driven local transactions.
mod config;
mod health;
mod login;
mod recovery;
mod rename;
use base64::Engine;
pub use config::{
    ConfigSnapshot, bind_address, capture_process_config, config_digest, current_config, label_for,
    parse_config, read_config, select_source, write_env_value,
};
#[cfg(test)]
pub(crate) use config::{fail_configuration_sync_after_for_test, fail_configuration_sync_for_test};
pub(crate) use health::authenticated_health_request;
pub use health::{session_health, session_health_with_state};
pub use login::{LoginTransaction, begin_login};
pub use recovery::recover_logins;
pub use rename::{recover_rename, rename_account, rename_configured_account};

/// Public transaction metadata has no configuration values or session payloads.
/// Private fields keep future callers from attaching arbitrary sensitive data.
#[derive(serde::Serialize)]
pub struct RemovalReport {
    removed: bool,
    #[serde(rename = "account")]
    selected_label: String,
    archived: String,
    moved: Vec<String>,
}

pub fn removal_report(
    state: &crate::state::State,
    account: &str,
    env_file: &std::path::Path,
    key: &str,
) -> anyhow::Result<RemovalReport> {
    rename::remove_owned_configuration(state, account, env_file, key)
}

/// Retain the JSON interface for embedders while sharing the same transaction.
pub fn remove_account(
    state: &crate::state::State,
    account: &str,
    env_file: &std::path::Path,
    key: &str,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::to_value(removal_report(
        state, account, env_file, key,
    )?)?)
}

/// Validate the historic import format without persisting or opening a connection.
pub fn session_validate(text: &str) -> &'static str {
    let text = text.trim();
    if text.is_empty() {
        return "empty";
    }
    let Some(encoded) = text.strip_prefix('1') else {
        return "invalid";
    };
    let decoded = base64::engine::general_purpose::URL_SAFE.decode(encoded);
    let Ok(bytes) = decoded else {
        return "invalid";
    };
    let ip_len = match bytes.len() {
        263 => 4,
        275 => 16,
        _ => return "invalid",
    };
    if !(1..=5).contains(&bytes[0])
        || u16::from_be_bytes([bytes[ip_len + 1], bytes[ip_len + 2]]) == 0
    {
        return "invalid";
    }
    "valid"
}
