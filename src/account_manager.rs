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
pub use health::{session_health, session_health_with_state};
pub use login::{LoginTransaction, begin_login};
pub use recovery::recover_logins;
pub use rename::{recover_rename, remove_account, rename_account};

/// Validate the historic import format without persisting or opening a connection.
pub fn session_validate(text: &str) -> &'static str {
    let text = text.trim();
    if text.is_empty() {
        return "empty";
    }
    let Some(encoded) = text.strip_prefix('1') else {
        return "invalid";
    };
    let decoded = base64::engine::general_purpose::URL_SAFE
        .decode(encoded)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded));
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
