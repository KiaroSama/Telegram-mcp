use super::config::{Publication, config_write_lock, state_owner, write_env_value_locked};
use super::recovery::{
    cleanup_completed_locked, completion_receipt, journal_name, recover_locked, value_digest,
};
use super::{config_digest, label_for, read_config};
use crate::state::State;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use rand::Rng;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct LoginTransaction {
    state: Arc<State>,
    account: String,
    stage: String,
    env_file: PathBuf,
    source_key: Option<String>,
    source_digest: Option<String>,
    expected_user_id: Option<i64>,
}

pub fn begin_login(
    state: Arc<State>,
    account: &str,
    env_file: &Path,
    replace_key: Option<&str>,
    expected_config_digest: Option<&str>,
    expected_user_id: Option<i64>,
) -> Result<LoginTransaction> {
    crate::session::validate_label(account)?;
    let account = account.to_ascii_lowercase();
    let key = if account == "default" {
        "TELEGRAM_SESSION_NAME".to_owned()
    } else {
        format!("TELEGRAM_SESSION_NAME_{}", account.to_ascii_uppercase())
    };
    label_for(&key)?;
    if account.starts_with("__login_") {
        bail!("This account prefix is reserved for private login staging");
    }
    super::recover_logins(&state)?;
    if state.root().join(journal_name(&account)).exists() {
        bail!(
            "The completed login recovery marker could not be cleared; preserve it before changing this account again"
        );
    }
    let env_file = if env_file.is_absolute() {
        env_file.to_owned()
    } else {
        std::env::current_dir()?.join(env_file)
    };
    // Reject unsupported Windows configuration storage before interactive
    // authorization creates a new staging session. The guard is not held while
    // the owner enters login credentials; commit rechecks the actual source.
    #[cfg(windows)]
    {
        let _admission = config_write_lock(&env_file)?;
    }
    let snapshot = read_config(&env_file)?;
    if replace_key.is_none()
        && snapshot.values.iter().any(|(key, value)| {
            !value.is_empty() && label_for(key).ok().as_deref() == Some(account.as_str())
        })
    {
        bail!("This account is already configured; use the explicit repair flow");
    }

    let source_digest = if let Some(key) = replace_key {
        if label_for(key)? != account {
            bail!("Replacement key does not belong to the selected account");
        }
        let value = snapshot
            .values
            .get(key)
            .context("The account configuration to replace is missing")?;
        let digest = config_digest(value);
        if expected_config_digest.is_some_and(|expected| expected != digest) {
            bail!("Account configuration changed before login; nothing was replaced");
        }
        Some(digest)
    } else {
        None
    };
    let old = state.read_json(&format!("{account}.session.json"), Value::Null)?;
    if replace_key.is_none() && !old.is_null() {
        bail!("This account already has a native session; use the explicit repair flow");
    }
    let stored_user = old["peer_infos"].as_array().and_then(|peers| {
        peers.iter().find_map(|peer| {
            let user = peer.get("User")?;
            if user["is_self"].as_bool() != Some(true) {
                return None;
            }
            user["id"]
                .as_i64()
                .or_else(|| user["id"].as_str().and_then(|id| id.parse().ok()))
        })
    });
    if let (Some(expected), Some(stored)) = (expected_user_id, stored_user)
        && expected != stored
    {
        bail!("Expected user identity conflicts with the existing native session");
    }
    let expected_user_id = expected_user_id.or(stored_user);
    if replace_key.is_some() && expected_user_id.is_none() {
        bail!(
            "Repair needs the existing Telegram user id; pass --expected-user-id to prevent replacing a different account"
        );
    }
    if expected_user_id.is_some_and(|id| id <= 0) {
        bail!("Expected user id must be positive");
    }
    let stage = format!("__login_{:016x}", rand::rng().random::<u64>());
    if state.root().join(format!("{stage}.session.json")).exists() {
        bail!("Private login staging name collision; retry login");
    }
    Ok(LoginTransaction {
        state,
        account,
        stage,
        env_file,
        source_key: replace_key.map(str::to_owned),
        source_digest,
        expected_user_id,
    })
}

impl LoginTransaction {
    pub fn stage_label(&self) -> &str {
        &self.stage
    }

    /// Authenticated network handles must be closed before committing staged keys.
    pub fn commit(self, actual_user_id: i64) -> Result<Value> {
        if actual_user_id <= 0
            || self
                .expected_user_id
                .is_some_and(|expected| expected != actual_user_id)
        {
            bail!(
                "The new login belongs to a different Telegram account; the existing session was preserved"
            );
        }
        let config = config_write_lock(&self.env_file)?;
        if let (Some(key), Some(expected)) = (&self.source_key, &self.source_digest) {
            let snapshot = read_config(&self.env_file)?;
            if snapshot
                .values
                .get(key)
                .map(|value| config_digest(value))
                .as_ref()
                != Some(expected)
            {
                bail!(
                    "Account configuration changed during login; the existing session was preserved"
                );
            }
        }
        let lease_path = self
            .state
            .root()
            .join(format!("{}.session.lock", self.account));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        if lease_path
            .symlink_metadata()
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            bail!("The target account lease is not a regular file");
        }
        let lease = options.open(&lease_path)?;
        crate::state::storage::restrict(&lease_path, false)?;
        FileExt::try_lock_exclusive(&lease).context(
            "The account is still in use; stop its owned service before committing a replacement",
        )?;
        let _stage_lease = lock_stage(&self.state, &self.stage)?;
        let _state_lock = self.state.state_lock()?;

        let source_name = format!("{}.session.json", self.stage);
        let target_name = format!("{}.session.json", self.account);
        let mut staged = self.state.read_json_unlocked(&source_name, Value::Null)?;
        let stage_digest = value_digest(&staged)?;
        if staged.is_null() {
            bail!("The staged native login was not saved");
        }
        let home = staged["home_dc"]
            .as_i64()
            .context("Staged native login has no home datacenter")?;
        let options: Vec<grammers_session::types::DcOption> =
            serde_json::from_value(staged["dc_options"].clone())
                .context("Staged native login datacenters are invalid")?;
        let peers: Vec<grammers_session::types::PeerInfo> =
            serde_json::from_value(staged["peer_infos"].clone())
                .context("Staged native login peer information is invalid")?;
        let _: grammers_session::types::UpdatesState =
            serde_json::from_value(staged["updates_state"].clone())
                .context("Staged native login update state is invalid")?;
        if !options
            .iter()
            .any(|option| i64::from(option.id) == home && option.auth_key.is_some())
        {
            bail!(
                "Staged native login has no authenticated home datacenter; the existing session was preserved"
            );
        }
        if !peers.iter().any(|peer|matches!(peer,grammers_session::types::PeerInfo::User{id,is_self:Some(true),..} if *id==actual_user_id)) {
            bail!("Staged native login does not contain the authenticated user identity; the existing session was preserved");
        }
        let authorization_source =
            if let (Some(key), Some(digest)) = (&self.source_key, &self.source_digest) {
                format!("{key}:{digest}")
            } else {
                let key = if self.account == "default" {
                    "TELEGRAM_SESSION_NAME".to_owned()
                } else {
                    format!(
                        "TELEGRAM_SESSION_NAME_{}",
                        self.account.to_ascii_uppercase()
                    )
                };
                format!("{key}:{}", config_digest(&self.account))
            };
        staged["authorization_source"] = json!(authorization_source);
        let old = self.state.read_json_unlocked(&target_name, Value::Null)?;
        if self.source_key.is_none() && !old.is_null() {
            bail!(
                "This account gained a native session during login; the existing session was preserved"
            );
        }
        let backup_name = format!("{}.native-backup.json", self.account);
        if self.source_key.is_none() {
            let key = if self.account == "default" {
                "TELEGRAM_SESSION_NAME".to_owned()
            } else {
                format!(
                    "TELEGRAM_SESSION_NAME_{}",
                    self.account.to_ascii_uppercase()
                )
            };
            let snapshot = read_config(&config.path)?;
            if snapshot
                .process_keys
                .iter()
                .any(|key| label_for(key).ok().as_deref() == Some(self.account.as_str()))
                || snapshot.values.iter().any(|(key, value)| {
                    !value.is_empty()
                        && label_for(key).ok().as_deref() == Some(self.account.as_str())
                })
            {
                bail!(
                    "This account gained configuration during login; the existing authorization was preserved"
                );
            }
            let name = journal_name(&self.account);
            let journal = json!({"version":2,"phase":"prepared","owner":state_owner(&self.state),"account":self.account,"stage":self.stage,"stage_digest":stage_digest,"env_file":config.path,"config_key":key,"config_digest":config_digest(&self.account),"installed":staged});
            let publication = (|| -> Result<Publication> {
                self.state.write_json_unlocked(&name, &journal)?;
                self.state.write_json_unlocked(&target_name, &staged)?;
                write_env_value_locked(&config, &key, &self.account, None)
            })();
            match publication {
                Ok(Publication::Uncertain) => {
                    bail!(
                        "Account authorization and configuration were published; recovery pending because directory durability is unconfirmed. Do not repeat login; retry recovery after the storage issue is resolved"
                    );
                }
                Ok(Publication::Durable) => {
                    let receipt = completion_receipt(&self.state, &name, &journal, true)?;
                    if self.state.write_json_unlocked(&name, &receipt).is_err() {
                        bail!(
                            "Account authorization and configuration were published; recovery pending because completion durability is unconfirmed. Do not repeat login; retry recovery after the storage issue is resolved"
                        );
                    }
                    if cleanup_completed_locked(&self.state, &name, &receipt).is_err() {
                        tracing::warn!(
                            "Account login is confirmed; its private completed-stage cleanup receipt is retained"
                        );
                    }
                }
                Err(error) => {
                    if self.state.root().join(&name).exists() {
                        let recovered=recover_locked(&self.state, &name, &journal, &config)
                            .context("Account login recovery pending; do not repeat login until recovery completes")?;
                        if recovered.committed {
                            if !recovered.durable {
                                bail!(
                                    "Account authorization and configuration were published; recovery pending because directory durability is unconfirmed. Do not repeat login; retry recovery after the storage issue is resolved"
                                );
                            }
                            tracing::warn!(
                                "Account login committed before a post-publication error"
                            );
                        } else {
                            return Err(error.context("Could not save account configuration; the incomplete native session was removed"));
                        }
                    } else {
                        return Err(error);
                    }
                }
            }
        } else {
            if !old.is_null() {
                self.state.write_json_unlocked(&backup_name, &old)?;
            }
            self.state.write_json_unlocked(&target_name, &staged)?;
        }
        // The new snapshot is durable. The staging copy is removed by Drop; the
        // single private backup keeps a failed operational rollback recoverable.
        Ok(
            json!({"saved":true,"account":self.account,"user_id":actual_user_id.to_string(),"backend":"grammers","secret_chat_available":false}),
        )
    }
}

impl Drop for LoginTransaction {
    fn drop(&mut self) {
        // Pending or completed intent owns exact stage cleanup. In particular,
        // never let this fallback delete a changed stage after receipt recovery
        // deliberately preserved it. Uninspectable intent is also preserved.
        match self
            .state
            .root()
            .join(journal_name(&self.account))
            .symlink_metadata()
        {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return,
        }
        // An unfinished sender can still own the authorization key. Preserve its
        // files when that owner has not released the lease, including error exits.
        let Ok(lease) = lock_stage(&self.state, &self.stage) else {
            return;
        };
        let path = self
            .state
            .root()
            .join(format!("{}.session.json", self.stage));
        if path
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
        {
            let _ = std::fs::remove_file(path);
        }
        drop(lease);
        // Keep the lease inode stable: unlinking it after release can race a
        // second owner opening it and split synchronization across two files.
    }
}

pub(super) fn lock_stage(state: &State, stage: &str) -> Result<std::fs::File> {
    crate::session::validate_label(stage)?;
    let path = state.root().join(format!("{stage}.session.lock"));
    if path
        .symlink_metadata()
        .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
    {
        bail!("Staged session lease is not a regular file");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lease = options.open(&path)?;
    crate::state::storage::restrict(&path, false)?;
    FileExt::try_lock_exclusive(&lease).context("The staged login is still active; close its sender before committing or cleaning its files")?;
    Ok(lease)
}
