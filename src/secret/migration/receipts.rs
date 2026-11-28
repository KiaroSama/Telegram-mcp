use super::*;

pub(super) fn validate_backup<'a>(base: &Path, receipt: &'a Receipt) -> Result<&'a str> {
    // Account renames leave the original label in the receipt and backup.
    let (label, timestamp) = receipt
        .backup
        .rsplit_once('-')
        .context("Invalid migration backup name")?;
    crate::session::validate_label(label).context("Invalid migration backup label")?;
    let format = "%Y%m%dT%H%M%SZ";
    let time = chrono::NaiveDateTime::parse_from_str(timestamp, format)
        .context("Invalid migration backup timestamp")?;
    ensure!(
        time.format(format).to_string() == timestamp,
        "Invalid migration backup timestamp"
    );
    ensure!(
        metadata(base)?.is_some_and(|m| m.is_dir()),
        "Secret-chat backup directory is unavailable"
    );
    ensure!(
        receipt.sha256.contains_key(label),
        "Secret-chat backup has no legacy store checksum"
    );
    let directory = base.join(&receipt.backup);
    ensure!(
        metadata(&directory)?.is_some_and(|m| m.is_dir()),
        "Secret-chat backup is not a regular directory"
    );
    for (name, expected) in &receipt.sha256 {
        ensure!(
            SOURCES
                .iter()
                .any(|suffix| *name == format!("{label}{suffix}")),
            "Invalid migration backup filename"
        );
        let path = directory.join(name);
        let bytes = Zeroizing::new(
            read_private(&path, MAX_BYTES)?.context("Secret-chat backup is incomplete")?,
        );
        ensure!(
            hash(&bytes) == *expected,
            "Secret-chat backup SHA-256 verification failed"
        );
        ensure!(
            fs::metadata(path)?.permissions().readonly(),
            "Secret-chat backup is writable"
        );
    }
    Ok(label)
}

use crate::secret::{
    history::message_id,
    receipts::{eligible, retain_history},
};
use kiaro_secret_chat::media::MediaReference;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consumer {
    version: u32,
    chats: BTreeMap<String, BTreeMap<String, Delivery>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delivery {
    seq_no: i32,
    committed_at: f64,
    message: Map<String, Value>,
    // Missing and null are distinct in the legacy schema.
    media_reference: Value,
}

type Sidecars = BTreeMap<&'static str, Zeroizing<Vec<u8>>>;

pub(super) fn sidecars(files: &Sidecars, settings: &BTreeMap<String, Value>) -> Result<Sidecars> {
    let mut outputs = Sidecars::new();
    for (source, target) in [
        ("-history.json", ".history.v1.json"),
        ("-media.json", ".media.v1.json"),
    ] {
        if let Some(bytes) = files.get(source) {
            let _: Map<String, Value> =
                serde_json::from_slice(bytes).context("Invalid legacy secret-chat sidecar")?;
            outputs.insert(target, bytes.clone());
        }
    }
    let Some(raw) = files.get("-consumer.json") else {
        return Ok(outputs);
    };
    import_consumer(raw, &mut outputs, None, settings)?;
    Ok(outputs)
}

fn import_consumer(
    raw: &[u8],
    outputs: &mut Sidecars,
    archive: Option<Value>,
    settings: &BTreeMap<String, Value>,
) -> Result<()> {
    let consumer: Consumer = serde_json::from_slice(raw)
        .map_err(|_| anyhow::anyhow!("Invalid legacy consumer receipts"))?;
    ensure!(consumer.version == 1, "Unknown legacy consumer version");
    let load = |name| -> Result<Map<String, Value>> {
        Ok(outputs
            .get(name)
            .map(|b| serde_json::from_slice(b))
            .transpose()?
            .unwrap_or_default())
    };
    let mut history = load(".history.v1.json")?;
    let mut media = load(".media.v1.json")?;
    let fresh = archive.is_none();
    let mut archive = archive.unwrap_or_else(|| json!({}));
    let obj = archive
        .as_object_mut()
        .context("Invalid native archive settings")?;
    let forgotten = obj.get("forgotten").cloned().unwrap_or_default();
    let receipts = obj
        .entry("delivery_receipts")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Invalid native delivery receipts")?;
    for (key, deliveries) in consumer.chats {
        let chat: i32 = key.parse().context("Invalid legacy consumer chat")?;
        ensure!(
            chat != 0 && chat.to_string() == key,
            "Invalid legacy consumer chat"
        );
        if forgotten.get(&key) == Some(&Value::Bool(true)) {
            continue;
        }
        let rows = history
            .entry(&key)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Invalid legacy history rows")?;
        let refs = media
            .entry(&key)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("Invalid legacy media index")?;
        let mut deliveries: Vec<_> = deliveries.into_iter().collect();
        deliveries.sort_by(|(_, a), (_, b)| {
            a.committed_at
                .total_cmp(&b.committed_at)
                .then(a.seq_no.cmp(&b.seq_no))
        });
        let mut blocked: std::collections::HashSet<i64> = rows
            .iter()
            .filter(|r| !eligible(r))
            .filter_map(message_id)
            .collect();
        blocked.extend(
            refs.iter()
                .filter(|(_, v)| {
                    v.get("ttl").is_some_and(|ttl| ttl.as_u64() != Some(0)) || !eligible(v)
                })
                .filter_map(|(id, _)| id.parse::<i64>().ok()),
        );
        for (key, delivery) in deliveries {
            ensure!(
                delivery.seq_no >= 0
                    && delivery.seq_no.to_string() == key
                    && delivery.committed_at.is_finite()
                    && delivery.committed_at > 0.0,
                "Invalid legacy consumer receipt identity"
            );
            let id = delivery
                .message
                .get("message_id")
                .and_then(Value::as_i64)
                .context("Invalid legacy consumer message identity")?;
            let mut row = Value::Object(delivery.message);
            if !eligible(&row) {
                blocked.insert(id);
                refs.remove(&id.to_string());
                continue;
            }
            if let Some(prior) = receipts.insert(format!("{chat}:{}", delivery.seq_no), json!(id)) {
                ensure!(
                    prior.as_i64() == Some(id),
                    "Conflicting native delivery receipt"
                );
            }
            if blocked.contains(&id) {
                continue;
            }
            let fields = row
                .as_object_mut()
                .context("Invalid legacy consumer message")?;
            fields.retain(|k, _| {
                matches!(
                    k.as_str(),
                    "message_id"
                        | "is_outgoing"
                        | "type"
                        | "can_be_saved"
                        | "text"
                        | "caption"
                        | "file_id"
                        | "self_destructs_after_seconds"
                        | "reply_to_message_id"
                )
            });
            for name in ["text", "caption"] {
                if let Some(text) = row.get(name) {
                    let text = text.as_str().context("Invalid legacy consumer text")?;
                    if !text.is_empty() {
                        row[name] = json!(
                            text.chars()
                                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                                .take(4096)
                                .collect::<String>()
                        );
                    }
                }
            }
            let date = chrono::DateTime::from_timestamp(
                delivery.committed_at as i64,
                (delivery.committed_at.fract() * 1e9) as u32,
            )
            .context("Invalid legacy consumer timestamp")?
            .to_rfc3339();
            row["date"] = json!(date);
            row["consumer_committed_at"] = json!(delivery.committed_at);
            row["date_basis"] = json!("consumer_commit");
            if !delivery.media_reference.is_null() {
                let reference = MediaReference::from_json(delivery.media_reference)
                    .map_err(|_| anyhow::anyhow!("Invalid legacy consumer media"))?;
                ensure!(
                    reference.chat_id == chat,
                    "Legacy consumer media belongs to another chat"
                );
                let verified = reference.to_json()?;
                if let Some(prior) = refs.get(&id.to_string()) {
                    ensure!(
                        prior.get("ttl").is_none_or(|ttl| ttl.as_u64() == Some(0))
                            && prior["reference"] == verified,
                        "Conflicting legacy consumer media"
                    );
                } else {
                    refs.insert(
                        id.to_string(),
                        json!({"reference":verified,"ttl":0,"received_at":date}),
                    );
                }
            }
            rows.push(row);
        }
        refs.retain(|id, _| id.parse::<i64>().is_ok_and(|id| !blocked.contains(&id)));
        rows.retain(|r| message_id(r).is_none_or(|id| !blocked.contains(&id)));
        retain_history(rows);
    }
    if fresh
        && let Some(config) = settings
            .get("auto_save_secret_chats")
            .or_else(|| settings.get("auto_save"))
        && config["on"] == true
        && config["folder"].is_string()
    {
        archive["folder"] = config["folder"].clone();
    }
    for (suffix, data) in [
        (".history.v1.json", Value::Object(history)),
        (".media.v1.json", Value::Object(media)),
        (".archive.v1.json", archive),
    ] {
        let bytes = Zeroizing::new(serde_json::to_vec(&data)?);
        ensure!(
            bytes.len() <= MAX_BYTES,
            "Imported secret-chat sidecar exceeds its byte budget"
        );
        outputs.insert(suffix, bytes);
    }
    Ok(())
}

pub(super) fn backup_file(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic(path, bytes, false)?;
    finish_backup(path)
}

pub(super) fn retain_backup_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(existing) = read_private(path, bytes.len())? {
        ensure!(
            existing == bytes,
            "Existing secret-chat backup differs from the snapshot"
        );
        finish_backup(path)
    } else {
        backup_file(path, bytes)
    }
}

fn finish_backup(path: &Path) -> Result<()> {
    crate::state::storage::restrict(path, false)?;
    let mut readonly = fs::metadata(path)?.permissions();
    readonly.set_readonly(true);
    #[cfg(windows)]
    let handle = {
        // FlushFileBuffers requires a write-capable handle. A resumed backup may
        // already be read-only, so restore that attribute even if opening fails.
        let mut writable = readonly.clone();
        writable.set_readonly(false);
        fs::set_permissions(path, writable)?;
        let opened = OpenOptions::new().read(true).write(true).open(path);
        fs::set_permissions(path, readonly)?;
        opened?
    };
    #[cfg(unix)]
    fs::set_permissions(path, readonly)?;
    #[cfg(windows)]
    handle.sync_all()?;
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    crate::state::storage::sync_directory(
        path.parent()
            .context("Secret-chat backup has no parent directory")?,
    )?;
    Ok(())
}

struct PreparedUpgrade {
    name: String,
    backup: PathBuf,
    raw: Zeroizing<Vec<u8>>,
    before: Sidecars,
    after: Sidecars,
}

// Discard the preflight snapshot: a current owner can still update native sidecars
// before we acquire custody, including the release/reopen gap during rebinding.
pub(super) fn preflight_upgrade(directory: &Path, label: &str, saved: &Receipt) -> Result<()> {
    prepare_upgrade(directory, label, saved).map(|_| ())
}

// Older native releases published the encrypted store without importing consumer receipts.
// Preparation must stay read-only: the caller may not have authenticated/rebound custody yet.
fn prepare_upgrade(
    directory: &Path,
    label: &str,
    saved: &Receipt,
) -> Result<Option<PreparedUpgrade>> {
    let base = directory.join("legacy-backups");
    let original = validate_backup(&base, saved)?;
    let name = format!("{original}-consumer.json");
    if saved.sha256.contains_key(&name) {
        return Ok(None);
    }
    let Some(raw) = read_private(&directory.join(format!("{label}-consumer.json")), MAX_BYTES)?
    else {
        return Ok(None);
    };
    let raw = Zeroizing::new(raw);
    let mut before = Sidecars::new();
    for suffix in [".history.v1.json", ".media.v1.json", ".archive.v1.json"] {
        if let Some(bytes) = read_private(&directory.join(format!("{label}{suffix}")), MAX_BYTES)? {
            before.insert(suffix, Zeroizing::new(bytes));
        }
    }
    let archive = before
        .get(".archive.v1.json")
        .map(|b| serde_json::from_slice(b))
        .transpose()?;
    let mut after = before.clone();
    import_consumer(&raw, &mut after, archive, &BTreeMap::new())?;
    let backup = base.join(&saved.backup).join(&name);
    if let Some(bytes) = read_private(&backup, MAX_BYTES)? {
        ensure!(
            *raw == bytes,
            "Legacy consumer file changed during migration"
        );
    }
    Ok(Some(PreparedUpgrade {
        name,
        backup,
        raw,
        before,
        after,
    }))
}

pub(super) fn apply_upgrade(
    directory: &Path,
    label: &str,
    receipt: &Path,
    original: &Receipt,
    authorization: &str,
) -> Result<()> {
    // The caller now holds the authenticated store lease. Rebuild the plan from
    // current files so a stale preflight cannot overwrite another owner's rows.
    let prepared = prepare_upgrade(directory, label, original)?;
    let mut saved = original.clone();
    saved.authorization = authorization.to_owned();
    let Some(PreparedUpgrade {
        name,
        backup,
        raw,
        before,
        after,
    }) = prepared
    else {
        if saved != *original {
            write_atomic(receipt, &serde_json::to_vec(&saved)?, true)?;
        }
        return Ok(());
    };
    retain_backup_file(&backup, &raw)?;
    saved.sha256.insert(name, hash(&raw));
    validate_backup(&directory.join("legacy-backups"), &saved)?;
    let mut changed = Vec::new();
    let commit = (|| -> Result<()> {
        for (suffix, bytes) in &after {
            if before.get(suffix).is_some_and(|prior| prior == bytes) {
                continue;
            }
            let path = directory.join(format!("{label}{suffix}"));
            changed.push((*suffix, path.clone()));
            write_atomic(&path, bytes, before.contains_key(suffix))?;
        }
        write_atomic(receipt, &serde_json::to_vec(&saved)?, true)
    })();
    if let Err(error) = commit {
        // A directory-sync error may follow a successful atomic receipt rename.
        if read_private(receipt, 16 * 1024)?.is_some_and(|bytes| {
            serde_json::from_slice::<Receipt>(&bytes).is_ok_and(|r| r == saved)
        }) {
            return Err(error);
        }
        for (suffix, path) in changed.into_iter().rev() {
            if let Some(bytes) = before.get(suffix) {
                write_atomic(&path, bytes, true)?;
            } else if plain_file(&path)? {
                fs::remove_file(path)?;
            }
        }
        crate::state::storage::sync_directory(directory)?;
        return Err(error);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::SecretHistory;
    use kiaro_secret_chat::storage::FileStorage;

    #[test]
    fn consumer_upgrade_reloads_native_history_after_preflight() {
        let temporary = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let directory = temporary.path().join("secret-chats");
        private_directory(&directory).unwrap();
        let label = "freshness";
        let binding =
            AuthorizationBinding::v2(17, 2, [0x42; 32], "consumer-freshness-test").unwrap();
        write_atomic(
            &directory.join(label),
            br#"{"chats":{},"in":{},"out":{},"settings":{}}"#,
            false,
        )
        .unwrap();
        drop(crate::secret::migrate_legacy(temporary.path(), label, &binding).unwrap());

        let receipt = directory.join(format!("{label}.migration.json"));
        let saved: Receipt = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
        let pending = json!({
            "version": 1,
            "chats": {"12": {"0": {
                "seq_no": 0,
                "committed_at": 1546800000,
                "message": {"message_id": 42, "text": "pending legacy delivery"},
                "media_reference": null
            }}}
        });
        let consumer = directory.join(format!("{label}-consumer.json"));
        write_atomic(&consumer, &serde_json::to_vec(&pending).unwrap(), false).unwrap();
        let pending_bytes = fs::read(&consumer).unwrap();
        let encrypted = directory.join(format!("{label}.v1.enc"));
        let key = key_bytes(&directory.join(format!("{label}.store.key")), false).unwrap();

        // A current owner may finish one native delivery after the read-only
        // preflight but before the upgrading caller obtains the custody lease.
        let current_owner =
            EncryptedIngressStorage::open(&encrypted, *key, binding.clone(), MAX_BYTES).unwrap();
        let history = SecretHistory::open(&directory, label, &BTreeMap::new()).unwrap();
        let earlier = json!({
            "message_id": 90, "text": "earlier native row",
            "date": "2026-11-28T12:00:00Z"
        });
        history.append(12, earlier.clone(), None).unwrap();
        preflight_upgrade(&directory, label, &saved).unwrap();
        let arrived = json!({
            "message_id": 99, "text": "native delivery after preflight",
            "date": "2026-11-28T13:00:00Z"
        });
        history.append(12, arrived.clone(), None).unwrap();
        drop(history);
        drop(current_owner);

        let applying_owner =
            EncryptedIngressStorage::open(&encrypted, *key, binding.clone(), MAX_BYTES).unwrap();
        assert!(
            FileStorage::open(&encrypted).is_err(),
            "The apply phase must own the store lease"
        );
        apply_upgrade(&directory, label, &receipt, &saved, &binding.digest()).unwrap();

        let outputs = [
            ".history.v1.json",
            ".media.v1.json",
            ".archive.v1.json",
            ".migration.json",
        ]
        .map(|suffix| directory.join(format!("{label}{suffix}")));
        let first = outputs.each_ref().map(|path| fs::read(path).unwrap());
        let actual: Value = serde_json::from_slice(&first[0]).unwrap();
        let rows = actual["12"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.contains(&earlier));
        assert!(
            rows.contains(&arrived),
            "A native row committed after preflight must survive"
        );
        assert_eq!(rows.iter().filter(|row| row["message_id"] == 42).count(), 1);
        let archive: Value = serde_json::from_slice(&first[2]).unwrap();
        assert_eq!(archive["delivery_receipts"]["12:0"], 42);

        let committed: Receipt = serde_json::from_slice(&first[3]).unwrap();
        apply_upgrade(&directory, label, &receipt, &committed, &binding.digest()).unwrap();
        assert_eq!(
            outputs.each_ref().map(|path| fs::read(path).unwrap()),
            first
        );
        assert_eq!(fs::read(&consumer).unwrap(), pending_bytes);
        drop(applying_owner);

        // Only this test's read-only fixture backups are made removable.
        #[cfg(windows)]
        for entry in fs::read_dir(directory.join("legacy-backups").join(&saved.backup)).unwrap() {
            let path = entry.unwrap().path();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions).unwrap();
        }
        temporary.close().unwrap();
    }
}
