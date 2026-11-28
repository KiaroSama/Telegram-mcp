use super::*;
mod authorization;
/// A difference request implicitly acknowledges encrypted updates in grammers.
/// Never attach a legacy authorization to that update consumer while this
/// account's old encryption custody still needs verified migration.
pub fn check_deferred_custody(state: &State, label: &str) -> Result<()> {
    validate_label(label)?;
    let native = state.read_json(&format!("{label}.session.json"), serde_json::Value::Null)?;
    if native["authorization_origin"].as_str() == Some("native") {
        return Ok(());
    }
    let directory = state.root().join("secret-chats");
    match directory.symlink_metadata() {
        Ok(metadata) => {
            // Never follow a redirected parent to decide that custody is
            // absent: a dangling link would turn child metadata into ENOENT.
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!(
                    "Cannot verify deferred secret-chat custody; its directory is not a regular owned directory"
                );
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    bail!("Cannot verify deferred secret-chat custody through a reparse point");
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => bail!(
            "Cannot verify deferred secret-chat custody; the legacy authorization was not connected"
        ),
    }
    for path in [
        directory.join(label),
        directory.join(format!("{label}.owner.json")),
    ] {
        match path.symlink_metadata() {
            Ok(_) => bail!(
                "This account has existing secret-chat custody. Verify its authorization and migrate the legacy store before connecting, or log in with a fresh native authorization under a separate label."
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => bail!(
                "Cannot verify deferred secret-chat custody; the legacy authorization was not connected"
            ),
        }
    }
    Ok(())
}

/// Verify custody against the selected authorization before a sender can start.
pub fn check_deferred_custody_with_config(
    state: &State,
    label: &str,
    config: &crate::account_manager::ConfigSnapshot,
) -> Result<()> {
    check_custody_identity(state, label, config)?;
    let directory = crate::secret::custody_dir(state.root())?;
    if !crate::secret::has_legacy(&directory, label)? {
        return Ok(());
    }
    let native = state.read_json(&format!("{label}.session.json"), serde_json::Value::Null)?;
    let mut snapshot: Snapshot = serde_json::from_value(native).map_err(|_| {
        anyhow::anyhow!(
            "Legacy secret-chat custody needs an imported local authorization before migration"
        )
    })?;
    let identity = snapshot.peer_infos.iter().find_map(|peer| match peer {
        PeerInfo::User {
            id,
            is_self: Some(true),
            bot,
            ..
        } => Some((*id, *bot)),
        _ => None,
    });
    let key = snapshot
        .dc_options
        .iter_mut()
        .find(|option| option.id == snapshot.home_dc)
        .and_then(|option| option.auth_key.take())
        .map(zeroize::Zeroizing::new)
        .context("No home authorization key is available for secret-chat custody")?;
    let owner = crate::secret::legacy_owner(&directory, label)?;
    let Some((user, bot)) = identity else {
        bail!(
            "No verified Telegram user identity is available for legacy secret-chat custody; verify the session owner before publishing migration"
        );
    };
    if bot == Some(true) {
        bail!("Bot authorization cannot adopt legacy secret-chat custody");
    }
    if owner.is_some_and(|expected| expected != user) {
        bail!("Verified session user does not match legacy secret-chat custody owner");
    }
    if snapshot.authorization_origin == "native" && owner.is_none() {
        bail!("Fresh native authorization cannot adopt unverified legacy secret-chat custody");
    }
    let binding = crate::secret::session_binding(user, snapshot.home_dc, &key)?;
    let legacy_binding = crate::secret::legacy_session_binding(user, snapshot.home_dc, &key)?;
    drop(crate::secret::migrate_legacy_with_v1(
        state.root(),
        label,
        &binding,
        Some(&legacy_binding),
    )?);
    Ok(())
}

impl PersistentSession {
    pub(crate) fn check_custody_identity(
        state: &State,
        label: &str,
        config: &crate::account_manager::ConfigSnapshot,
    ) -> Result<()> {
        check_custody_identity(state, label, config)
    }
}

fn check_custody_identity(
    state: &State,
    label: &str,
    config: &crate::account_manager::ConfigSnapshot,
) -> Result<()> {
    validate_label(label)?;
    let native = state.read_json(&format!("{label}.session.json"), serde_json::Value::Null)?;
    let fresh_native = native["authorization_origin"].as_str() == Some("native");
    let directory = crate::secret::custody_dir(state.root())?;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => bail!("Cannot verify deferred authorization custody; no connection was opened"),
    };
    let mut owners = std::collections::BTreeMap::new();
    for (count, entry) in entries.enumerate() {
        if count >= 10_000 {
            bail!("Deferred custody exceeds its entry budget; no connection was opened");
        }
        let entry = entry
            .map_err(|_| anyhow::anyhow!("Cannot enumerate deferred authorization custody"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            bail!("Cannot identify deferred custody owner; no connection was opened");
        };
        if name == "legacy-backups" {
            continue;
        }
        let (owner, native_store) = match name.strip_suffix(".v1.enc") {
            Some(owner) => (owner, true),
            None => (name.strip_suffix(".owner.json").unwrap_or(name), false),
        };
        if owner != label && validate_label(owner).is_ok() {
            owners
                .entry(owner.to_owned())
                .and_modify(|saved| *saved |= native_store)
                .or_insert(native_store);
        }
    }
    // A new native login is independent of unverified legacy custody, but a
    // copied native session must not split an existing encrypted store's identity.
    owners.retain(|_, native_store| !fresh_native || *native_store);
    if owners.is_empty() {
        return Ok(());
    }
    let keys = authorization::load(state, label, config, false)
        .map_err(|_| anyhow::anyhow!("Cannot verify candidate authorization against deferred custody; no connection was opened"))?;
    if keys.is_empty() {
        // An existing native snapshot without an authorization cannot consume
        // anybody's encrypted queue; Native does not replace existing snapshots.
        return Ok(());
    }
    for (owner, native_store) in owners {
        let owned = authorization::load(state, &owner, config, !native_store)
            .map_err(|_| anyhow::anyhow!("Cannot identify an existing deferred-custody authorization; no connection was opened"))?;
        if owned.is_empty() {
            bail!(
                "Cannot identify an existing deferred-custody authorization; no connection was opened"
            );
        }
        if keys.intersects(&owned) {
            bail!(
                "This authorization has existing secret-chat custody under another account label. Open that label or use the account rename flow to preserve its keys and protocol state"
            );
        }
    }
    Ok(())
}
