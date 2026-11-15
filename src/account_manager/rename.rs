use crate::state::State;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::path::{Component, Path};

const JOURNAL: &str = "account-rename-transaction.json";

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

pub fn rename_account(state: &State, from: &str, to: &str) -> Result<Vec<String>> {
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
        recover_unlocked(state)?;
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
            for prefix in ["TELEGRAM_SESSION_NAME", "TELEGRAM_SESSION_STRING"] {
                let old_key = if from == "default" {
                    prefix.to_owned()
                } else {
                    format!("{prefix}_{}", from.to_ascii_uppercase())
                };
                if key == old_key {
                    let new_key = if to == "default" {
                        prefix.to_owned()
                    } else {
                        format!("{prefix}_{}", to.to_ascii_uppercase())
                    };
                    // The retained PowerShell manager renames keys and preserves
                    // their values, including legacy session paths. Keep its hash.
                    after["authorization_source"] = json!(format!("{new_key}:{digest}"));
                }
            }
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
                if let Some(rows) = after["accounts"][&to].as_object_mut() {
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
    state.write_json_transaction_unlocked(JOURNAL, &json!({"files":files,"rewrites":rewrites}))?;
    let result = (|| -> Result<Vec<String>> {
        let mut changed = Vec::new();
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
        std::fs::remove_file(state.root().join(JOURNAL))?;
        Ok(changed)
    })();
    if result.is_err() {
        recover_unlocked(state).context("Account rename failed; rollback could not finish, retain the private transaction journal")?;
    }
    drop(leases);
    result
}

pub fn recover_rename(state: &State) -> Result<()> {
    if !state.root().join(JOURNAL).exists() {
        return Ok(());
    }
    let _lock = state.state_lock()?;
    recover_unlocked(state)
}

fn recover_unlocked(state: &State) -> Result<()> {
    let journal = state.read_json_unlocked(JOURNAL, Value::Null)?;
    if journal.is_null() {
        return Ok(());
    }
    // Ordinary writers are fenced while this marker exists. Also detect raw
    // out-of-band edits before restoring any whole-record snapshot.
    for pair in journal["files"]
        .as_array()
        .context("Invalid account transaction journal")?
    {
        let source = local_path(state, pair[0].as_str().context("Invalid account source")?)?;
        let target = local_path(
            state,
            pair[1].as_str().context("Invalid account destination")?,
        )?;
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
            continue;
        }
        let current = state.read_json_unlocked(name, Value::Null)?;
        if current != row["before"] && current != row["after"] {
            bail!(
                "Account state changed after the transaction snapshot; recovery retained all current records and its private journal"
            );
        }
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
    for row in journal["files"]
        .as_array()
        .context("Invalid account transaction journal")?
        .iter()
        .rev()
    {
        let source = local_path(state, row[0].as_str().context("Invalid account source")?)?;
        let target = local_path(
            state,
            row[1].as_str().context("Invalid account destination")?,
        )?;
        if target.exists() {
            if source.exists() {
                bail!(
                    "Account rollback found conflicting files; retain both and the private journal"
                );
            }
            std::fs::rename(target, source)?;
        }
    }
    std::fs::remove_file(state.root().join(JOURNAL))?;
    Ok(())
}
