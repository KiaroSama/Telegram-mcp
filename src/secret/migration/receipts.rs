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
    #[cfg(windows)]
    let handle = OpenOptions::new().read(true).write(true).open(path)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)?;
    #[cfg(windows)]
    handle.sync_all()?;
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

// Older native releases published the encrypted store without importing consumer receipts.
pub(super) fn upgrade(
    directory: &Path,
    label: &str,
    receipt: &Path,
    saved: &mut Receipt,
) -> Result<()> {
    let base = directory.join("legacy-backups");
    let original = validate_backup(&base, saved)?;
    let name = format!("{original}-consumer.json");
    if saved.sha256.contains_key(&name) {
        return Ok(());
    }
    let Some(raw) = read_private(&directory.join(format!("{label}-consumer.json")), MAX_BYTES)?
    else {
        return Ok(());
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
        let mut perm = fs::metadata(&backup)?.permissions();
        if !perm.readonly() {
            perm.set_readonly(true);
            fs::set_permissions(&backup, perm)?;
        }
    } else {
        backup_file(&backup, &raw)?;
    }
    saved.sha256.insert(name, hash(&raw));
    validate_backup(&base, saved)?;
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
        write_atomic(receipt, &serde_json::to_vec(saved)?, true)
    })();
    if let Err(error) = commit {
        // A directory-sync error may follow a successful atomic receipt rename.
        if read_private(receipt, 16 * 1024)?.is_some_and(|bytes| {
            serde_json::from_slice::<Receipt>(&bytes).is_ok_and(|r| r.sha256 == saved.sha256)
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
