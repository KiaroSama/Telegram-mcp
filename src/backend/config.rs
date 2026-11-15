use crate::{session::validate_label, state::State};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

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
    let root = State::global()?;
    let accounts=account_labels()?.into_iter().map(|label|{
        let (key,value)=source(&label)?.context("Account source disappeared during snapshot")?;
        let digest=format!("{:x}",Sha256::digest(value.as_bytes()));
        Ok(json!({"label":label,"account":label,"config_key":key,"config_digest":digest,"native_session":root.root().join(format!("{label}.session.json")).is_file(),"source":"configuration","backend":"grammers","secret_chat_available":false}))
    }).collect::<Result<Vec<_>>>()?;
    Ok(json!({"accounts":accounts,"backend":"grammers","secret_chat_available":false}))
}
pub fn health(account: Option<&str>) -> Result<Value> {
    let _state = State::global()?;
    if let Some(label) = account {
        validate_label(label)?;
    }
    Ok(
        json!({"ready":true,"backend":"grammers","state_accessible":true,"network_authorization":"not_probed","account":account,"version":env!("CARGO_PKG_VERSION")}),
    )
}
