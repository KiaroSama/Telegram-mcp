use super::{
    JOURNAL, completion_receipt, configuration_plan, is_completed, local_path, retire_completed,
    sync_move_parents,
};
use crate::account_manager::{
    config::{
        ConfigWriteGuard, Publication, config_write_lock, confirm_configuration, file_values,
    },
    config_digest, label_for, login,
};
use crate::state::State;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

pub fn recover_rename(state: &State) -> Result<()> {
    if !state.root().join(JOURNAL).exists() {
        return Ok(());
    }
    let initial = state.read_json(JOURNAL, Value::Null)?;
    if is_completed(state, &initial)? {
        let _lock = state.state_lock()?;
        if state.read_json_unlocked(JOURNAL, Value::Null)? != initial {
            bail!("Completed account receipt changed while acquiring ownership");
        }
        return retire_completed(state);
    }
    let config = if let Some(plan) = configuration_plan(&initial) {
        let path = plan["env_file"]
            .as_str()
            .context("Invalid account configuration transaction source")?;
        let path = Path::new(path);
        if !path.is_absolute() {
            bail!("Account configuration source must be absolute");
        }
        Some(config_write_lock(path)?)
    } else {
        None
    };
    let mut leases = Vec::new();
    for label in [initial["from"].as_str(), initial["to"].as_str()]
        .into_iter()
        .flatten()
    {
        leases.push(login::lock_stage(state, label)?);
    }
    let _lock = state.state_lock()?;
    if state.read_json_unlocked(JOURNAL, Value::Null)? != initial {
        bail!("Account transaction changed while acquiring recovery ownership");
    }
    recover_unlocked(state, config.as_ref())
}

pub(super) fn recover_unlocked(state: &State, config: Option<&ConfigWriteGuard>) -> Result<()> {
    let journal = state.read_json_unlocked(JOURNAL, Value::Null)?;
    if journal.is_null() {
        return Ok(());
    }
    if is_completed(state, &journal)? {
        return retire_completed(state);
    }
    let committed = if let Some(plan) = configuration_plan(&journal) {
        let config = config.context("Account recovery needs its configuration lock")?;
        if plan["env_file"].as_str() != config.path.to_str() {
            bail!("Account configuration source changed");
        }
        let metadata = config
            .path
            .symlink_metadata()
            .context("Account configuration is missing; retain its journal")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("Account configuration is not a regular file");
        }
        let key = plan["key"]
            .as_str()
            .context("Invalid account removal key")?;
        if Some(label_for(key)?.as_str()) != journal["from"].as_str() {
            bail!("Account removal labels conflict with their configuration key");
        }
        let digest = plan["digest"]
            .as_str()
            .context("Invalid account removal digest")?;
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Invalid account removal digest");
        }
        let values = file_values(&config.path)?;
        if let Some(target) = plan["new_key"].as_str() {
            if Some(label_for(target)?.as_str()) != journal["to"].as_str()
                || key.starts_with("TELEGRAM_SESSION_STRING")
                    != target.starts_with("TELEGRAM_SESSION_STRING")
            {
                bail!("Account rename labels conflict with their configuration keys");
            }
            match (values.get(key), values.get(target)) {
                (Some(value), None) if config_digest(value) == digest => false,
                (None, Some(value)) if config_digest(value) == digest => true,
                _ => bail!(
                    "Account rename configuration changed; preserve its current values and transaction journal"
                ),
            }
        } else {
            if !journal["to"]
                .as_str()
                .is_some_and(|label| label.starts_with("__removed_"))
            {
                bail!("Account removal destination is not a private archive");
            }
            match values.get(key) {
                Some(value) if config_digest(value) == digest => false,
                Some(_) => bail!(
                    "The removed account configuration changed; preserve its state and transaction journal"
                ),
                None => true,
            }
        }
    } else {
        false
    };
    // Ordinary writers are fenced while this marker exists. Also detect raw
    // out-of-band edits before restoring any whole-record snapshot.
    let files = journal["files"]
        .as_array()
        .context("Invalid account transaction journal")?
        .iter()
        .map(|pair| {
            Ok((
                pair[0]
                    .as_str()
                    .context("Invalid account source")?
                    .to_owned(),
                pair[1]
                    .as_str()
                    .context("Invalid account destination")?
                    .to_owned(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    for (source, target) in &files {
        let source = local_path(state, source)?;
        let target = local_path(state, target)?;
        if source.exists() && target.exists() {
            bail!("Account rollback found conflicting files; retain both and the private journal");
        }
        if !source.exists() && !target.exists() {
            bail!("Account rollback source is missing; retain the private journal");
        }
    }
    for row in journal["rewrites"]
        .as_array()
        .context("Invalid account transaction journal")?
    {
        let name = row["name"]
            .as_str()
            .context("Invalid account transaction record")?;
        if let Some(source) = row["moved_source"].as_str()
            && local_path(state, source)?.exists()
        {
            if committed && state.read_json_unlocked(source, Value::Null)? != row["before"] {
                bail!(
                    "Account authorization changed before archival; retain its current session and journal"
                );
            }
            continue;
        }
        let current = state.read_json_unlocked(name, Value::Null)?;
        if current != row["before"] && current != row["after"] {
            bail!(
                "Account state changed after the transaction snapshot; recovery retained all current records and its private journal"
            );
        }
    }
    if committed {
        // The exact configuration-key change is the commit point. Finish any
        // remaining state moves without restoring an obsolete .env revision.
        for (source, target) in &files {
            let source = local_path(state, source)?;
            let target = local_path(state, target)?;
            if source.exists() {
                std::fs::rename(source, target)?;
            }
        }
        for row in journal["rewrites"]
            .as_array()
            .context("Invalid account transaction journal")?
        {
            let name = row["name"]
                .as_str()
                .context("Invalid account transaction record")?;
            state.write_json_transaction_unlocked(name, &row["after"])?;
        }
        sync_move_parents(state, &files)?;
        confirm_recovery_configuration(config)?;
        return complete_recovery(state, &journal);
    }
    for row in journal["rewrites"]
        .as_array()
        .context("Invalid account transaction journal")?
        .iter()
        .rev()
    {
        let name = row["name"]
            .as_str()
            .context("Invalid account transaction record")?;
        if let Some(source) = row["moved_source"].as_str() {
            // If the file move had not happened before a crash, restoring the
            // target here would manufacture a conflicting second session file.
            if local_path(state, source)?.exists() {
                continue;
            }
        }
        state.write_json_transaction_unlocked(name, &row["before"])?;
    }
    for (source, target) in files.iter().rev() {
        let source = local_path(state, source)?;
        let target = local_path(state, target)?;
        if target.exists() {
            if source.exists() {
                bail!(
                    "Account rollback found conflicting files; retain both and the private journal"
                );
            }
            std::fs::rename(target, source)?;
        }
    }
    sync_move_parents(state, &files)?;
    confirm_recovery_configuration(config)?;
    complete_recovery(state, &journal)
}

fn confirm_recovery_configuration(config: Option<&ConfigWriteGuard>) -> Result<()> {
    if config.is_some_and(|config| confirm_configuration(&config.path) == Publication::Uncertain) {
        bail!(
            "Account recovery is applied; configuration durability is unconfirmed, so its private intent is retained"
        );
    }
    Ok(())
}

fn complete_recovery(state: &State, journal: &Value) -> Result<()> {
    state.write_json_transaction_unlocked(JOURNAL, &completion_receipt(state, journal)?)?;
    retire_completed(state)
}
