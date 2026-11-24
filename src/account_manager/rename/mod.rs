use super::config::{
    ConfigWriteGuard, Publication, config_write_lock, file_values, remove_env_value_locked,
    rename_env_key_locked, state_owner, sync_directory, validate_env_key_rename,
};
use crate::state::State;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::path::{Component, Path};

mod recovery;

pub use recovery::recover_rename;
use recovery::recover_unlocked;

const JOURNAL: &str = "account-rename-transaction.json";

fn is_completed(state: &State, journal: &Value) -> Result<bool> {
    if journal.get("version").is_none() {
        if ["phase", "owner", "legacy_recovery"]
            .iter()
            .any(|key| journal.get(*key).is_some())
        {
            bail!("Account transaction completion version is missing");
        }
        return Ok(false); // Legacy journals retain their original recovery rules.
    }
    if journal["version"].as_u64() != Some(2) || journal["owner"] != state_owner(state) {
        bail!("Account transaction version or private state ownership is invalid");
    }
    if journal.get("legacy_recovery").is_some() {
        if journal["phase"] == "completed"
            && journal["legacy_recovery"] == true
            && journal.as_object().is_some_and(|fields| fields.len() == 4)
        {
            return Ok(true);
        }
        bail!("Legacy account completion envelope is invalid");
    }
    for field in ["from", "to"] {
        let label = journal[field]
            .as_str()
            .context("Invalid account transaction label")?;
        crate::session::validate_label(label)?;
        if label != label.to_ascii_lowercase() {
            bail!("Account transaction labels are not canonical");
        }
    }
    if journal["from"] == journal["to"] {
        bail!("Account transaction labels must differ");
    }
    match journal["phase"].as_str() {
        Some("prepared") => Ok(false),
        Some("completed") if journal.as_object().is_some_and(|fields| fields.len() == 5) => {
            Ok(true)
        }
        _ => bail!("Account transaction completion envelope is invalid"),
    }
}

fn completion_receipt(state: &State, journal: &Value) -> Result<Value> {
    let receipt = if journal.get("version").is_none() {
        // Older journals did not always record labels. They may acquire this
        // closed cleanup-only envelope only after their recovery is durable.
        json!({"version":2,"phase":"completed","owner":state_owner(state),"legacy_recovery":true})
    } else {
        json!({"version":2,"phase":"completed","owner":state_owner(state),"from":journal["from"],"to":journal["to"]})
    };
    is_completed(state, &receipt)?;
    Ok(receipt)
}

fn retire_completed(state: &State) -> Result<()> {
    // Also confirms a receipt whose atomic replacement previously reported an
    // uncertain directory sync. Never delete the only completion evidence first.
    sync_directory(state.root()).context("Completed account receipt durability is unconfirmed")?;
    std::fs::remove_file(state.root().join(JOURNAL))?;
    if sync_directory(state.root()).is_err() {
        // The durable completed receipt can only reappear as cleanup-only. Its
        // removal uncertainty must never undo or fail the acknowledged operation.
        tracing::warn!(
            "Completed account receipt is removed; its retirement durability is unconfirmed"
        );
    }
    Ok(())
}

/// Caller already holds state_lock. Only cleanup-only receipts may bypass the
/// ordinary-writer fence; pending transactions still require trusted recovery.
pub(crate) fn cleanup_completed_rename_locked(state: &State) -> Result<bool> {
    let metadata = state
        .root()
        .join(JOURNAL)
        .symlink_metadata()
        .context("Cannot verify the pending account transaction record")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("The pending account transaction record is not a regular file");
    }
    let journal = state.read_json_unlocked(JOURNAL, Value::Null)?;
    if !is_completed(state, &journal)? {
        return Ok(false);
    }
    retire_completed(state)?;
    Ok(true)
}

fn configuration_plan(journal: &Value) -> Option<&Value> {
    journal
        .get("configuration")
        .filter(|value| !value.is_null())
        .or_else(|| journal.get("removal").filter(|value| !value.is_null()))
}

fn move_key(value: &mut Value, old: &str, new: &str) -> Result<()> {
    let Some(map) = value.as_object_mut() else {
        return Ok(());
    };
    if map.contains_key(old) {
        if map.contains_key(new) {
            bail!("The destination account already has stored records");
        }
        if let Some(value) = map.remove(old) {
            map.insert(new.to_owned(), value);
        }
    }
    Ok(())
}

fn local_path(state: &State, name: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("Invalid account transaction path");
    }
    let full = state.root().join(path);
    if full
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        bail!("An account transaction path is a symlink");
    }
    if let Some(parent) = full.parent()
        && parent.exists()
        && !parent.canonicalize()?.starts_with(state.root())
    {
        bail!("Account transaction path leaves private state");
    }
    Ok(full)
}

fn move_parents(
    state: &State,
    files: &[(String, String)],
) -> Result<std::collections::BTreeSet<std::path::PathBuf>> {
    let mut parents = std::collections::BTreeSet::from([state.root().to_owned()]);
    for (source, target) in files {
        for name in [source, target] {
            let path = local_path(state, name)?;
            parents.insert(
                path.parent()
                    .context("Account state path has no parent")?
                    .to_owned(),
            );
        }
    }
    Ok(parents)
}

fn sync_move_parents(state: &State, files: &[(String, String)]) -> Result<()> {
    for parent in move_parents(state, files)? {
        sync_directory(&parent).context("Could not confirm account state move durability")?;
    }
    Ok(())
}

pub fn rename_account(state: &State, from: &str, to: &str) -> Result<Vec<String>> {
    recover_rename(state)?;
    rename_with_config(state, from, to, None)
}

/// Move an account's state and exact file-owned configuration in one journal.
pub fn rename_configured_account(
    state: &State,
    from: &str,
    to: &str,
    env_file: &Path,
    key: &str,
) -> Result<Vec<String>> {
    let from = from.to_ascii_lowercase();
    let to = to.to_ascii_lowercase();
    crate::session::validate_label(&from)?;
    crate::session::validate_label(&to)?;
    if super::label_for(key)? != from {
        bail!("Rename key does not belong to the selected account");
    }
    let prefix = if key.starts_with("TELEGRAM_SESSION_STRING") {
        "TELEGRAM_SESSION_STRING"
    } else {
        "TELEGRAM_SESSION_NAME"
    };
    let new_key = if to == "default" {
        prefix.to_owned()
    } else {
        format!("{prefix}_{}", to.to_ascii_uppercase())
    };
    super::label_for(&new_key)?;
    if from == to {
        return Ok(vec![]);
    }
    recover_rename(state)?;
    super::recover_logins(state)?;
    let config = config_write_lock(env_file)?;
    let metadata = config
        .path
        .symlink_metadata()
        .context("The account configuration is missing")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Account rename requires a regular configuration file");
    }
    let snapshot = super::read_config(&config.path)?;
    if snapshot
        .process_keys
        .iter()
        .any(|key| super::label_for(key).is_ok_and(|label| label == from || label == to))
    {
        bail!("This account is owned by the process environment; rename it there first");
    }
    if snapshot
        .values
        .get(key)
        .is_none_or(|value| value.is_empty())
    {
        bail!("The selected account configuration is missing");
    }
    let values = file_values(&config.path)?;
    let value = values
        .get(key)
        .context("The selected account is not file-owned")?;
    if values.contains_key(&new_key)
        || values.iter().any(|(other, value)| {
            other != key
                && !value.is_empty()
                && super::label_for(other).is_ok_and(|label| label == from || label == to)
        })
    {
        bail!("Another configuration key defines a source or destination account");
    }
    let digest = super::config_digest(value);
    // Reference rewriting must be semantically safe before moving any state.
    validate_env_key_rename(&config, key, &new_key, &digest)?;
    let plan = json!({"env_file":config.path,"key":key,"digest":digest,"new_key":new_key});
    rename_with_config(state, &from, &to, Some((&config, &plan)))
}

/// Archive one account and remove its exact file-owned configuration together.
pub(super) fn remove_owned_configuration(
    state: &State,
    account: &str,
    env_file: &Path,
    key: &str,
) -> Result<super::RemovalReport> {
    let account = account.to_ascii_lowercase();
    crate::session::validate_label(&account)?;
    if super::label_for(key)? != account {
        bail!("Removal key does not belong to the selected account");
    }
    recover_rename(state)?;
    super::recover_logins(state)?;
    let config = config_write_lock(env_file)?;
    let metadata = config
        .path
        .symlink_metadata()
        .context("The account configuration is missing")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("Account removal requires a regular configuration file");
    }
    let snapshot = super::read_config(&config.path)?;
    if snapshot
        .process_keys
        .iter()
        .any(|key| super::label_for(key).is_ok_and(|label| label == account))
    {
        bail!("This account is owned by the process environment; remove it there first");
    }
    let values = file_values(&config.path)?;
    let value = values
        .get(key)
        .filter(|value| !value.is_empty())
        .context("The selected account configuration is missing")?;
    if values.iter().any(|(other, value)| {
        other != key
            && !value.is_empty()
            && super::label_for(other).is_ok_and(|label| label == account)
    }) {
        bail!("Several configuration keys define this account; resolve them before removal");
    }
    use rand::Rng;
    let archive = format!(
        "__removed_{}_{:012x}",
        account.chars().take(41).collect::<String>(),
        rand::rng().random::<u64>() & 0xffffffffffff
    );
    let plan = json!({"env_file":config.path,"key":key,"digest":super::config_digest(value)});
    let moved = rename_with_config(state, &account, &archive, Some((&config, &plan)))?;
    Ok(super::RemovalReport {
        removed: true,
        selected_label: account,
        archived: archive,
        moved,
    })
}

fn rename_with_config(
    state: &State,
    from: &str,
    to: &str,
    configuration: Option<(&ConfigWriteGuard, &Value)>,
) -> Result<Vec<String>> {
    let from = from.to_ascii_lowercase();
    let to = to.to_ascii_lowercase();
    crate::session::validate_label(&from)?;
    crate::session::validate_label(&to)?;
    if from == to {
        return Ok(vec![]);
    }
    let mut leases = Vec::new();
    for label in [&from, &to] {
        let path = state.root().join(format!("{label}.session.lock"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        if path
            .symlink_metadata()
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            bail!("Invalid account lease file");
        }
        let file = options.open(&path)?;
        crate::state::storage::restrict(&path, false)?;
        FileExt::try_lock_exclusive(&file)
            .context("An account is in use; stop its service before renaming")?;
        leases.push(file);
    }
    let _lock = state.state_lock()?;
    if state.root().join(JOURNAL).exists() {
        bail!("Recover the pending account transaction before renaming");
    }
    if [&from, &to].into_iter().any(|label| {
        state
            .root()
            .join(super::recovery::journal_name(label))
            .exists()
    }) {
        bail!("Recover the pending account login before renaming its account");
    }
    let mut files = Vec::<(String, String)>::new();
    let native = (format!("{from}.session.json"), format!("{to}.session.json"));
    if state.root().join(&native.0).exists() {
        files.push(native);
    }
    // Preserve deferred secret-chat state and its labels without importing it.
    for pattern in [
        "{}",
        "{}.owner.json",
        "{}-history.json",
        "{}-media.json",
        "{}-consumer.json",
    ] {
        let source = format!("secret-chats/{}", pattern.replace("{}", &from));
        let target = format!("secret-chats/{}", pattern.replace("{}", &to));
        if local_path(state, &source)?.exists() {
            files.push((source, target));
        }
    }
    for (source, target) in &files {
        local_path(state, source)?;
        if local_path(state, target)?.exists() {
            bail!("The destination account already has stored files; nothing was renamed");
        }
    }
    let mut rewrites = Vec::new();
    let native_source = format!("{from}.session.json");
    if state.root().join(&native_source).is_file() {
        let before = state.read_json_unlocked(&native_source, Value::Null)?;
        let mut after = before.clone();
        if let Some(binding) = before["authorization_source"].as_str() {
            let (key, digest) = binding
                .split_once(':')
                .context("Native authorization source is malformed; nothing was renamed")?;
            if super::label_for(key)? != from {
                bail!(
                    "Native authorization source belongs to another account; nothing was renamed"
                );
            }
            let prefix = if key.starts_with("TELEGRAM_SESSION_STRING") {
                "TELEGRAM_SESSION_STRING"
            } else {
                "TELEGRAM_SESSION_NAME"
            };
            let new_key = if to == "default" {
                prefix.to_owned()
            } else {
                format!("{prefix}_{}", to.to_ascii_uppercase())
            };
            // Labels normalize to lowercase, but an existing source key may use
            // a mixed-case suffix. Preserve its source type and exact value hash.
            after["authorization_source"] = json!(format!("{new_key}:{digest}"));
        }
        if before != after {
            rewrites.push(json!({"name":format!("{to}.session.json"),"moved_source":native_source,"before":before,"after":after}));
        }
    }
    for name in [
        "ghost.json",
        "aliases-v2.json",
        "aliases.json",
        "proxies-v1.json",
        "proxy-pool.json",
        "sent-recipients.json",
        "always-approvals.json",
        "approval-messages.json",
    ] {
        if !state.root().join(name).exists() {
            continue;
        }
        let before = state.read_json_unlocked(name, Value::Null)?;
        let mut after = before.clone();
        match name {
            "ghost.json" => {
                move_key(&mut after["accounts"], &from, &to)?;
                move_key(&mut after["chats"], &from, &to)?;
            }
            "aliases-v2.json" => {
                move_key(&mut after["accounts"], &from, &to)?;
                if let Some(rows) = after
                    .get_mut("accounts")
                    .and_then(|accounts| accounts.get_mut(&to))
                    .and_then(Value::as_object_mut)
                {
                    for value in rows.values_mut() {
                        value["account"] = json!(to);
                    }
                }
            }
            "aliases.json" => {
                let map = after
                    .as_object_mut()
                    .context("Legacy alias store is invalid")?;
                let old_prefix = format!("{from}\n");
                let new_prefix = format!("{to}\n");
                let moves = map
                    .keys()
                    .filter(|key| key.to_lowercase().starts_with(&old_prefix))
                    .cloned()
                    .collect::<Vec<_>>();
                if !moves.is_empty()
                    && map
                        .keys()
                        .any(|key| key.to_lowercase().starts_with(&new_prefix))
                {
                    bail!("Destination account already has legacy aliases");
                }
                for key in moves {
                    if let Some(mut value) = map.remove(&key) {
                        if let Some(account) = value.get_mut("account") {
                            if account
                                .as_str()
                                .is_some_and(|label| label.eq_ignore_ascii_case(&from))
                            {
                                *account = json!(to);
                            } else if !account.is_null() {
                                bail!(
                                    "Legacy alias account conflicts with its scope; nothing was renamed"
                                );
                            }
                        }
                        map.insert(format!("{new_prefix}{}", &key[old_prefix.len()..]), value);
                    }
                }
            }
            "proxies-v1.json" => {
                if let Some(map) = after.as_object_mut() {
                    if map.values().any(|row| {
                        row["pool"].as_str() == Some(&to)
                            || row["pools"].as_array().is_some_and(|pools| {
                                pools.iter().any(|pool| pool.as_str() == Some(&to))
                            })
                    }) {
                        bail!("Destination account already has a proxy pool");
                    }
                    for row in map.values_mut() {
                        if row["pool"].as_str() == Some(&from) {
                            row["pool"] = json!(to);
                        }
                        if let Some(pools) = row["pools"].as_array_mut() {
                            for pool in pools {
                                if pool.as_str() == Some(&from) {
                                    *pool = json!(to);
                                }
                            }
                        }
                    }
                }
            }
            "sent-recipients.json" => move_key(&mut after, &from, &to)?,
            "always-approvals.json" => {
                if let Some(rows) = after["grants"].as_array_mut() {
                    if rows.iter().any(|row| row[0].as_str() == Some(&to)) {
                        bail!("Destination account already has approval grants");
                    }
                    for row in rows {
                        if row[0].as_str() == Some(&from) {
                            row[0] = json!(to);
                        }
                    }
                }
            }
            _ => {
                move_key(&mut after["accounts"], &from, &to)?;
                move_key(&mut after["routes"], &from, &to)?;
            }
        }
        if before != after {
            rewrites.push(json!({"name":name,"before":before,"after":after}));
        }
    }
    // These approvals bind exact arguments and must not survive an account rename.
    for name in ["approval-pending.json", "approval-grants.json"] {
        if state.root().join(name).exists() {
            let before = state.read_json_unlocked(name, json!({}))?;
            rewrites.push(json!({"name":name,"before":before,"after":{}}));
        }
    }
    // Admission probes the real Windows capability before creating intent or
    // changing any account file, including preserved stores in nested parents.
    #[cfg(windows)]
    for parent in move_parents(state, &files)? {
        crate::state::storage::sync_directory(&parent)
            .context("Account state requires accessible NTFS directories on Windows")?;
    }
    let journal = json!({"version":2,"phase":"prepared","owner":state_owner(state),"from":from,"to":to,"files":files,"rewrites":rewrites,"configuration":configuration.map(|(_,plan)|plan)});
    state.write_json_transaction_unlocked(JOURNAL, &journal)?;
    let mut changed = Vec::new();
    let mut recovery_pending = false;
    let result = (|| -> Result<()> {
        for (source, target) in &files {
            std::fs::rename(local_path(state, source)?, local_path(state, target)?)?;
            changed.push(source.clone());
        }
        for row in &rewrites {
            let name = row["name"]
                .as_str()
                .context("Invalid account transaction record")?;
            state.write_json_transaction_unlocked(name, &row["after"])?;
            changed.push(name.to_owned());
        }
        // Native files and deferred secret-chat sidecars can have different
        // parents. Confirm every move before publishing the configuration key.
        sync_move_parents(state, &files)?;
        let publication = if let Some((config, plan)) = configuration {
            let key = plan["key"]
                .as_str()
                .context("Invalid transaction configuration key")?;
            let digest = plan["digest"]
                .as_str()
                .context("Invalid transaction configuration digest")?;
            if let Some(target) = plan["new_key"].as_str() {
                rename_env_key_locked(config, key, target, digest)?
            } else {
                remove_env_value_locked(config, key, digest)?
            }
        } else {
            Publication::Durable
        };
        if publication == Publication::Uncertain {
            recovery_pending = true;
            bail!(
                "Account changes and configuration were published; recovery pending because directory durability is unconfirmed. Do not repeat the operation; retry recovery after the storage issue is resolved"
            );
        }
        if let Err(error) =
            state.write_json_transaction_unlocked(JOURNAL, &completion_receipt(state, &journal)?)
        {
            recovery_pending = true;
            return Err(error.context(
                "Account changes were published; recovery pending because completion durability is unconfirmed. Do not repeat the operation; retry recovery after the storage issue is resolved",
            ));
        }
        if retire_completed(state).is_err() {
            tracing::warn!(
                "Account transaction completed; its private completion receipt is retained for cleanup"
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Once publication has occurred, an unconfirmed configuration or
        // completion receipt must retain ownership for explicit recovery. A
        // normal precommit rollback could erase the visible committed state.
        if recovery_pending {
            return Err(error);
        }
        let completed = is_completed(state, &state.read_json_unlocked(JOURNAL, Value::Null)?)?;
        recover_unlocked(state, configuration.map(|(config, _)| config))
            .context("Account rename needs recovery; retain its private transaction journal")?;
        if !completed {
            return Err(error);
        }
    }
    drop(leases);
    Ok(changed)
}
