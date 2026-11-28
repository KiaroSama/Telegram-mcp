use super::*;
use kiaro_secret_chat::{binding_migration::migrate_v1_enc_store_to_v2, storage::FileStorage};

// The upstream JSON envelope hex-encodes ciphertext and has its own 128 MiB limit.
// MAX_BYTES is the smaller decrypted-state budget, not the outer file size.
const MAX_ENVELOPE_BYTES: usize = 128 * 1024 * 1024;

pub(super) fn upgrade(
    directory: &Path,
    label: &str,
    encrypted: &Path,
    key: [u8; 32],
    old: &LegacyAuthorizationBinding,
    binding: &AuthorizationBinding,
) -> Result<()> {
    let key = Zeroizing::new(key);
    // Keep the real store lease from snapshot capture through backup and replacement.
    let mut owner = FileStorage::open(encrypted)?;
    let outer = owner
        .load()?
        .context("Encrypted secret-chat store is missing")?;
    ensure!(
        outer.mode == StoreMode::Ingress,
        "Secret-chat binding migration requires an encrypted ingress store"
    );
    if outer.authorization == binding.digest() {
        // The caller authenticates this store before publishing any receipt change.
        // An older interrupted upgrade may legitimately have no ciphertext backup.
        return Ok(());
    }
    ensure!(
        outer.authorization == old.fingerprint(),
        "Encrypted secret-chat store does not match the historical authorization"
    );
    let original = Zeroizing::new(
        read_private(encrypted, MAX_ENVELOPE_BYTES)?
            .context("Encrypted secret-chat store disappeared during migration")?,
    );
    let stage = tempfile::tempdir_in(directory)?;
    crate::state::storage::restrict(stage.path(), true)?;
    let staged_store = stage.path().join("binding.enc");
    write_atomic(&staged_store, &original, false)?;
    // Authenticate the original snapshot and both binding identities on a private
    // copy. A bad key, corrupt ciphertext, or invalid proof cannot change custody.
    migrate_v1_enc_store_to_v2(&staged_store, *key, old, binding, MAX_BYTES)?;
    let replacement = FileStorage::open(&staged_store)?
        .load()?
        .context("Rebound secret-chat snapshot is missing")?;
    ensure!(
        replacement.mode == StoreMode::Ingress && replacement.authorization == binding.digest(),
        "Rebound secret-chat snapshot has an invalid authorization or mode"
    );

    let base = directory.join("legacy-backups");
    private_directory(&base)?;
    let backup = base.join(format!("{label}-binding-v1-{}.enc", hash(&original)));
    receipts::retain_backup_file(&backup, &original)?;
    // This copies the latest ciphertext, never the storage key or a hard link to
    // the live file. The older plaintext import backup may predate native changes.
    owner.commit(&replacement).context(
        "Cannot confirm secret-chat binding replacement; reopen the store before retrying",
    )?;
    crate::state::storage::sync_directory(directory).context(
        "Secret-chat binding was replaced but directory durability could not be confirmed",
    )?;
    Ok(())
}
