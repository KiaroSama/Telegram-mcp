//! Recovery for a new account's session/configuration two-file publication.
use super::{
    config::{ConfigWriteGuard, config_write_lock, file_values},
    config_digest,
    login::lock_stage,
};
use crate::state::State;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;
const PREFIX: &str = "account-login-";
const SUFFIX: &str = "-transaction.json";
pub(super) fn journal_name(account: &str) -> String {
    format!("{PREFIX}{account}{SUFFIX}")
}
pub(super) fn value_digest(value: &Value) -> Result<String> {
    Ok(config_digest(&serde_json::to_string(value)?))
}

fn same_authorization(current: &Value, installed: &Value) -> bool {
    let home = &installed["home_dc"];
    let key = |value: &Value| {
        value["dc_options"]
            .as_array()
            .and_then(|options| options.iter().find(|option| option["id"] == *home))
            .and_then(|option| option.get("auth_key"))
            .filter(|key| !key.is_null())
            .cloned()
    };
    current["authorization_source"] == installed["authorization_source"]
        && current["home_dc"] == *home
        && key(current).is_some()
        && key(current) == key(installed)
}

/// Caller holds the selected configuration byte lock, both account leases and
/// state_lock. Recovery never restores a whole .env revision over newer writes.
pub(super) fn recover_locked(
    state: &State,
    name: &str,
    journal: &Value,
    config: &ConfigWriteGuard,
) -> Result<bool> {
    if state
        .root()
        .join("account-rename-transaction.json")
        .exists()
    {
        bail!("Recover the pending account rename before completing a login");
    }
    let account = journal["account"]
        .as_str()
        .context("Invalid login transaction account")?;
    crate::session::validate_label(account)?;
    if name != journal_name(account) {
        bail!("Login transaction filename does not match its account");
    }
    let env = Path::new(
        journal["env_file"]
            .as_str()
            .context("Invalid login configuration source")?,
    );
    if !env.is_absolute() || env != config.path {
        bail!("Login transaction configuration source changed");
    }
    let stage = journal["stage"]
        .as_str()
        .context("Invalid login staging label")?;
    crate::session::validate_label(stage)?;
    if !stage.starts_with("__login_") {
        bail!("Invalid private login staging label");
    }
    let installed = &journal["installed"];
    if !installed.is_object() {
        bail!("Invalid installed login snapshot");
    }
    let target = format!("{account}.session.json");
    let current = state.read_json_unlocked(&target, Value::Null)?;
    let key = journal["config_key"]
        .as_str()
        .context("Invalid login configuration key")?;
    if super::label_for(key)? != account || !key.starts_with("TELEGRAM_SESSION_NAME") {
        bail!("Login transaction configuration key conflicts with its account");
    }
    let expected = journal["config_digest"]
        .as_str()
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .context("Invalid login configuration digest")?;
    if expected != config_digest(account) {
        bail!("Login recovery digest does not match its new account binding");
    }
    if !journal["stage_digest"].as_str().is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        bail!("Invalid login staging digest");
    }
    if installed["authorization_source"] != format!("{key}:{expected}")
        || !same_authorization(installed, installed)
    {
        bail!("Login recovery snapshot has no matching authorization");
    }
    let stage_name = format!("{stage}.session.json");
    let staged = state.read_json_unlocked(&stage_name, Value::Null)?;
    if !staged.is_null()
        && Some(value_digest(&staged)?.as_str()) != journal["stage_digest"].as_str()
    {
        bail!("The login staging snapshot changed; recovery retained its private journal");
    }
    let values = file_values(&config.path)?;
    let committed = values.get(key).map(|value| config_digest(value)).as_deref()
        == journal["config_digest"].as_str();
    if committed {
        if current.is_null() {
            state.write_json_unlocked(&target, installed)?;
        } else if !same_authorization(&current, installed) {
            bail!(
                "The login target changed after publication; recovery preserved its current authorization and journal"
            );
        }
    } else if !current.is_null() {
        if current != *installed {
            bail!(
                "The incomplete login target changed; recovery preserved its current authorization and journal"
            );
        }
        std::fs::remove_file(state.root().join(&target))?;
    }
    if !staged.is_null() {
        std::fs::remove_file(state.root().join(stage_name))?;
    }
    // Both files now describe a completed commit or a completed rollback.
    // Failing to prune its marker must not report that committed files rolled back.
    if std::fs::remove_file(state.root().join(name)).is_err() {
        tracing::warn!("Account login completed; its private recovery marker could not be removed");
    }
    Ok(committed)
}

pub fn recover_logins(state: &State) -> Result<()> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(state.root())? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    for name in names {
        let initial = state.read_json(&name, Value::Null)?;
        let env = Path::new(
            initial["env_file"]
                .as_str()
                .context("Invalid pending login configuration source")?,
        );
        if !env.is_absolute() {
            bail!("Pending login configuration must have an absolute source path");
        }
        let config = config_write_lock(env)?;
        let account = initial["account"]
            .as_str()
            .context("Invalid pending login account")?;
        let stage = initial["stage"]
            .as_str()
            .context("Invalid pending login staging label")?;
        let _target = lock_stage(state, account)?;
        let _stage = lock_stage(state, stage)?;
        let _state = state.state_lock()?;
        let journal = state.read_json_unlocked(&name, Value::Null)?;
        if journal != initial {
            bail!("Login recovery marker changed while acquiring ownership");
        }
        recover_locked(state, &name, &journal, &config)?;
    }
    Ok(())
}
