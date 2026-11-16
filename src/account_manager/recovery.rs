//! Recovery for a new account's session/configuration two-file publication.
use super::{
    config::{
        ConfigWriteGuard, Publication, config_write_lock, confirm_configuration, file_values,
        state_owner, sync_directory,
    },
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

fn read_stage(state: &State, name: &str) -> Result<Option<Value>> {
    match state.root().join(name).symlink_metadata() {
        Ok(_) => Ok(Some(state.read_json_unlocked(name, Value::Null)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn is_completed(state: &State, name: &str, journal: &Value) -> Result<bool> {
    if journal.get("version").is_none() {
        if journal.get("phase").is_some() || journal.get("owner").is_some() {
            bail!("Login completion envelope is missing its version");
        }
        return Ok(false);
    }
    if journal["version"].as_u64() != Some(2) || journal["owner"] != state_owner(state) {
        bail!("Login transaction version or private state ownership is invalid");
    }
    match journal["phase"].as_str() {
        Some("prepared") => Ok(false),
        Some("completed") => {
            if !journal.as_object().is_some_and(|fields| fields.len() == 7)
                || !journal["committed"].is_boolean()
            {
                bail!("Login completion envelope is invalid");
            }
            let account = journal["account"]
                .as_str()
                .context("Invalid completed login account")?;
            crate::session::validate_label(account)?;
            if account != account.to_ascii_lowercase()
                || account.starts_with("__login_")
                || name != journal_name(account)
            {
                bail!("Completed login receipt does not match its account");
            }
            let stage = journal["stage"]
                .as_str()
                .context("Invalid completed login stage")?;
            crate::session::validate_label(stage)?;
            if !stage.starts_with("__login_") || stage == account {
                bail!("Completed login receipt does not identify a private stage");
            }
            if !journal["stage_digest"].as_str().is_some_and(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                bail!("Completed login staging digest is invalid");
            }
            Ok(true)
        }
        _ => bail!("Login transaction phase is invalid"),
    }
}

pub(super) fn completion_receipt(
    state: &State,
    name: &str,
    journal: &Value,
    committed: bool,
) -> Result<Value> {
    let receipt = serde_json::json!({"version":2,"phase":"completed","owner":state_owner(state),"account":journal["account"],"stage":journal["stage"],"stage_digest":journal["stage_digest"],"committed":committed});
    is_completed(state, name, &receipt)?;
    Ok(receipt)
}

/// Caller holds state_lock and the reserved stage lease. A receipt is historical
/// cleanup evidence, so this path never reads configuration or the native target.
pub(super) fn cleanup_completed_locked(state: &State, name: &str, receipt: &Value) -> Result<()> {
    if !is_completed(state, name, receipt)? {
        bail!("A completed login receipt is required");
    }
    let stage = receipt["stage"]
        .as_str()
        .context("Invalid completed login stage")?;
    let stage_name = format!("{stage}.session.json");
    if let Some(staged) = read_stage(state, &stage_name)? {
        if Some(value_digest(&staged)?.as_str()) != receipt["stage_digest"].as_str() {
            bail!(
                "Completed login staging data changed; retain it and its private cleanup receipt"
            );
        }
        std::fs::remove_file(state.root().join(stage_name))?;
    }
    // Confirm both receipt publication and any stage deletion before retiring
    // recovery evidence. Its durable completed phase is harmless if resurrected.
    sync_directory(state.root())
        .context("Completed login cleanup is pending directory confirmation")?;
    std::fs::remove_file(state.root().join(name))?;
    if sync_directory(state.root()).is_err() {
        tracing::warn!("Completed login receipt is removed; retirement durability is unconfirmed");
    }
    Ok(())
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

pub(super) struct RecoveryOutcome {
    pub committed: bool,
    pub durable: bool,
}

/// Caller holds the selected configuration byte lock, both account leases and
/// state_lock. Recovery never restores a whole .env revision over newer writes.
pub(super) fn recover_locked(
    state: &State,
    name: &str,
    journal: &Value,
    config: &ConfigWriteGuard,
) -> Result<RecoveryOutcome> {
    if is_completed(state, name, journal)? {
        sync_directory(state.root())
            .context("Login completion was published; recovery pending, do not repeat login")?;
        if cleanup_completed_locked(state, name, journal).is_err() {
            tracing::warn!(
                "Login completion is confirmed; its private stage cleanup receipt is retained"
            );
        }
        return Ok(RecoveryOutcome {
            committed: journal["committed"].as_bool().unwrap_or(false),
            durable: true,
        });
    }
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
    let staged = read_stage(state, &stage_name)?;
    if let Some(staged) = &staged
        && Some(value_digest(staged)?.as_str()) != journal["stage_digest"].as_str()
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
    if staged.is_some() {
        std::fs::remove_file(state.root().join(stage_name))?;
    }
    sync_directory(state.root())
        .context("Login recovery cleanup durability is unconfirmed; retain its private intent")?;
    // Publication may already have happened. Retain intent until its selected
    // configuration directory is durable, without undoing the visible commit.
    if confirm_configuration(&config.path) == Publication::Uncertain {
        return Ok(RecoveryOutcome {
            committed,
            durable: false,
        });
    }
    // Resolve legacy prepared intent into a durable, target-independent cleanup
    // receipt before unlinking. A resurrected receipt never replays rollback.
    let receipt = completion_receipt(state, name, journal, committed)?;
    state.write_json_unlocked(name, &receipt)?;
    if cleanup_completed_locked(state, name, &receipt).is_err() {
        tracing::warn!(
            "Login recovery is confirmed; its private completed-stage cleanup receipt is retained"
        );
    }
    Ok(RecoveryOutcome {
        committed,
        durable: true,
    })
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
        if is_completed(state, &name, &initial)? {
            let stage = initial["stage"]
                .as_str()
                .context("Invalid completed login stage")?;
            let _stage = lock_stage(state, stage)?;
            let _state = state.state_lock()?;
            if state.read_json_unlocked(&name, Value::Null)? != initial {
                bail!("Completed login receipt changed while acquiring ownership");
            }
            cleanup_completed_locked(state, &name, &initial)?;
            continue;
        }
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
        if !recover_locked(state, &name, &journal, &config)?.durable {
            bail!(
                "Login recovery is applied; configuration durability is unconfirmed, so its private intent is retained"
            );
        }
    }
    Ok(())
}
