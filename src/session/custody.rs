use super::*;
mod authorization;
/// A difference request implicitly acknowledges encrypted updates in grammers.
/// Never attach a legacy authorization to that update consumer while this
/// account's old encryption custody is waiting for its separate Rust backend.
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
                "This account has existing secret-chat custody. Keep its legacy authorization untouched until the separate Rust secret backend is ready, or log in with a fresh native authorization under a separate label."
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
    if snapshot.authorization_origin == "native"
        && (owner.is_none() || identity.map(|i| i.0) != owner)
    {
        bail!("Fresh native authorization cannot adopt unverified legacy secret-chat custody");
    }
    if identity.is_some_and(|(_, bot)| bot == Some(true)) {
        bail!("Bot authorization cannot adopt legacy secret-chat custody");
    }
    let user = identity.map(|i| i.0).or(owner).context(
        "No verified Telegram user identity is available for legacy secret-chat custody",
    )?;
    let binding = crate::secret::session_binding(user, snapshot.home_dc, &key)?;
    drop(crate::secret::migrate_legacy(
        state.root(),
        label,
        &binding,
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
    if native["authorization_origin"].as_str() == Some("native") {
        return Ok(());
    }
    let directory = crate::secret::custody_dir(state.root())?;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => bail!("Cannot verify deferred authorization custody; no connection was opened"),
    };
    let mut owners = std::collections::BTreeSet::new();
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
        let owner = name.strip_suffix(".owner.json").unwrap_or(name);
        if owner != label && validate_label(owner).is_ok() {
            owners.insert(owner.to_owned());
        }
    }
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
    for owner in owners {
        let owned = authorization::load(state, &owner, config, true)
            .map_err(|_| anyhow::anyhow!("Cannot identify an existing deferred-custody authorization; no connection was opened"))?;
        if owned.is_empty() {
            bail!(
                "Cannot identify an existing deferred-custody authorization; no connection was opened"
            );
        }
        if keys.intersects(&owned) {
            bail!(
                "This authorization has existing secret-chat custody under an account label. Keep its legacy authorization untouched until the separate Rust secret backend is ready, or use a fresh native authorization"
            );
        }
    }
    Ok(())
}
