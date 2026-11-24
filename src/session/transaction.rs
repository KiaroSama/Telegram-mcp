use super::*;
use sha2::{Digest, Sha256};
impl PersistentSession {
    pub fn bind_source(&self, source: Option<String>) -> Result<()> {
        let current = self.data()?;
        let old = self.binding.lock().map_err(|_| SessionError)?.clone();
        if old.is_some() && old != source {
            bail!(
                "Native authorization belongs to a different configuration revision; use the explicit account repair flow"
            );
        }
        if old == source {
            return Ok(());
        }
        *self.binding.lock().map_err(|_| SessionError)? = source;
        if let Err(error) = self.persist(&current) {
            *self.binding.lock().map_err(|_| SessionError)? = old;
            return Err(error.into());
        }
        Ok(())
    }
    pub(super) fn clone_data(data: &SessionData) -> SessionData {
        SessionData {
            home_dc: data.home_dc,
            dc_options: data.dc_options.clone(),
            peer_infos: data.peer_infos.clone(),
            updates_state: data.updates_state.clone(),
        }
    }
    pub(super) fn transaction(
        &self,
        change: impl FnOnce(&mut SessionData),
    ) -> std::result::Result<(), SessionError> {
        let mut current = self.data()?;
        let mut candidate = Self::clone_data(&current);
        change(&mut candidate);
        self.persist(&candidate)?;
        *current = candidate;
        Ok(())
    }
    pub(super) fn commit_import(&self, candidate: SessionData) -> Result<()> {
        let mut current = self.data()?;
        if current.dc_options.values().any(|dc| dc.auth_key.is_some()) {
            bail!("Refusing to overwrite an existing authorization");
        }
        self.persist_import(&candidate)?;
        *current = candidate;
        Ok(())
    }
    pub(super) fn persist_import(&self, candidate: &SessionData) -> Result<()> {
        let previous = self.origin.lock().map_err(|_| SessionError)?.clone();
        *self.origin.lock().map_err(|_| SessionError)? = "legacy".into();
        if let Err(error) = self.persist(candidate) {
            *self.origin.lock().map_err(|_| SessionError)? = previous;
            return Err(error.into());
        }
        Ok(())
    }
    pub(super) fn acquire_auth_leases(&self, data: &SessionData) -> Result<()> {
        let mut leases = self
            .auth_leases
            .lock()
            .map_err(|_| anyhow::anyhow!("Authorization lease lock unavailable"))?;
        for key in data
            .dc_options
            .values()
            .filter_map(|dc| dc.auth_key.as_ref())
        {
            let fingerprint = format!("{:x}", Sha256::digest(key));
            if leases.contains_key(&fingerprint) {
                continue;
            }
            let path = self.state.root().join(format!("auth-{fingerprint}.lock"));
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
                bail!("Invalid authorization lease path");
            }
            let file = options
                .open(&path)
                .context("Cannot open private authorization lease")?;
            crate::state::storage::restrict(&path, false)?;
            FileExt::try_lock_exclusive(&file).context("This Telegram authorization is already active through another account label; use its shared service")?;
            leases.insert(fingerprint, file);
        }
        Ok(())
    }
}
