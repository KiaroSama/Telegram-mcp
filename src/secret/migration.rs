use super::{MAX_BYTES, metadata, plain_file, private_directory, read_private, write_atomic};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use kiaro_secret_chat::{
    binding::AuthorizationBinding,
    chat::{Chat, ChatState, DhRecord, Envelope, Exchange, Outgoing, ServiceDelivery},
    crypto::SharedKey,
    dh::SecretExponent,
    error::CloseReason,
    ingress::EncryptedIngressStorage,
    storage::{SecretRecord, State, Storage, StoreMode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
};
use zeroize::Zeroizing;

const SOURCES: [&str; 4] = ["", "-history.json", "-media.json", ".owner.json"];
const SIDECARS: [&str; 4] = ["", ".history.v1.json", ".media.v1.json", ""];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    authorization: String,
    backup: String,
    sha256: BTreeMap<String, String>,
}

pub(crate) fn custody_dir(root: &Path) -> Result<PathBuf> {
    ensure!(
        !root.components().any(|c| matches!(c, Component::ParentDir)),
        "Secret-chat state root contains traversal"
    );
    let root = std::path::absolute(root)?;
    for ancestor in root.ancestors() {
        ensure!(
            metadata(ancestor)?.is_some_and(|m| m.is_dir()),
            "Secret-chat state root is not a regular directory"
        );
    }
    let path = root.canonicalize()?.join("secret-chats");
    if let Some(m) = metadata(&path)? {
        ensure!(
            m.is_dir(),
            "Cannot verify deferred secret-chat custody; its directory is not regular"
        );
    }
    Ok(path)
}

pub(crate) fn legacy_owner(directory: &Path, label: &str) -> Result<Option<i64>> {
    let path = directory.join(format!("{label}.owner.json"));
    let Some(bytes) = read_private(&path, 16 * 1024)? else {
        return Ok(None);
    };
    let record = parse(&bytes, "Legacy secret-chat owner record is invalid")?;
    Ok(Some(
        record["user_id"]
            .as_i64()
            .filter(|id| *id > 0)
            .context("Legacy secret-chat owner identity is invalid")?,
    ))
}

pub(crate) fn has_legacy(directory: &Path, label: &str) -> Result<bool> {
    let mut found = false;
    for suffix in SOURCES {
        found |= plain_file(&directory.join(format!("{label}{suffix}")))?;
    }
    Ok(found)
}

/// Import legacy data for a caller-verified authorization, retaining the original files.
pub fn migrate_legacy(
    root: &Path,
    label: &str,
    binding: &AuthorizationBinding,
) -> Result<Option<EncryptedIngressStorage>> {
    open_store(root, label, binding, false)
}

pub(crate) fn open_store(
    root: &Path,
    label: &str,
    binding: &AuthorizationBinding,
    create: bool,
) -> Result<Option<EncryptedIngressStorage>> {
    crate::session::validate_label(label)?;
    binding.validate()?;
    let directory = custody_dir(root)?;
    if metadata(&directory)?.is_none() {
        if !create {
            return Ok(None);
        }
        private_directory(&directory)?;
    }
    let lock = directory.join(format!("{label}.migrate.lock"));
    let encrypted = directory.join(format!("{label}.v1.enc"));
    let key = directory.join(format!("{label}.store.key"));
    let receipt = directory.join(format!("{label}.migration.json"));
    let store_lock = directory.join(format!("{label}.v1.enc.lock"));
    for file in [&lock, &encrypted, &key, &receipt, &store_lock] {
        plain_file(file)?;
    }
    if !create
        && !plain_file(&encrypted)?
        && !plain_file(&receipt)?
        && !has_legacy(&directory, label)?
    {
        return Ok(None);
    }
    let _guard = migration_lock(&lock)?;
    let legacy = has_legacy(&directory, label)?;
    for suffix in [".history.v1.json", ".media.v1.json"] {
        plain_file(&directory.join(format!("{label}{suffix}")))?;
    }
    if let Some(owner) = legacy_owner(&directory, label)? {
        ensure!(
            owner == binding.user_id(),
            "Legacy secret-chat custody belongs to another Telegram user"
        );
    }
    let saved_receipt = read_private(&receipt, 16 * 1024)?
        .map(|bytes| {
            serde_json::from_slice::<Receipt>(&bytes)
                .map_err(|_| anyhow::anyhow!("Invalid secret-chat migration receipt"))
        })
        .transpose()?;
    if let Some(saved) = &saved_receipt {
        ensure!(
            saved.version == 1 && saved.authorization == binding.digest(),
            "Secret-chat migration receipt does not match the authorization"
        );
    }
    if plain_file(&encrypted)? {
        ensure!(
            !legacy || saved_receipt.is_some(),
            "Existing encrypted secret-chat store has no migration receipt for the legacy data"
        );
        if let Some(saved) = &saved_receipt {
            validate_backup(&directory.join("legacy-backups"), saved)?;
        }
        let bytes = key_bytes(&key, false)?;
        let store = EncryptedIngressStorage::open(encrypted, *bytes, binding.clone(), MAX_BYTES)?;
        return Ok(Some(store));
    }
    if !legacy {
        ensure!(
            saved_receipt.is_none(),
            "Migrated secret-chat store is missing"
        );
        if !create {
            return Ok(None);
        }
        let bytes = key_bytes(&key, true)?;
        return Ok(Some(EncryptedIngressStorage::open(
            encrypted,
            *bytes,
            binding.clone(),
            MAX_BYTES,
        )?));
    }

    let mut files = BTreeMap::new();
    for suffix in SOURCES {
        if let Some(bytes) = read_private(&directory.join(format!("{label}{suffix}")), MAX_BYTES)? {
            files.insert(suffix, Zeroizing::new(bytes));
        }
    }
    let source = files
        .get("")
        .context("Legacy secret-chat sidecars exist without their key store")?;
    let mut state = convert(
        &parse(source, "Legacy secret-chat store contains invalid JSON")?,
        binding,
    )?;
    // The strong journal requires a schema marker when converting a nonempty state.
    state.private_records.insert(
        "ingress/journal-v1".into(),
        SecretRecord::new(serde_json::to_vec(
            &json!({"version":1,"checkpoint":null,"next_id":1,"batches":[],"paused":false}),
        )?),
    );
    for (index, suffix) in SOURCES.into_iter().enumerate().take(3).skip(1) {
        if let Some(bytes) = files.get(suffix) {
            ensure!(
                parse(
                    bytes,
                    "Legacy secret-chat history or media index contains invalid JSON"
                )?
                .is_object(),
                "Legacy secret-chat history or media index is not an object"
            );
            let output = directory.join(format!("{label}{}", SIDECARS[index]));
            if let Some(existing) = read_private(&output, MAX_BYTES)? {
                ensure!(
                    saved_receipt.is_some() && existing == **bytes,
                    "Secret-chat migration would overwrite an existing sidecar"
                );
            }
        }
    }
    state
        .validate()
        .context("Migrated secret-chat state is invalid")?;
    let checksums: BTreeMap<_, _> = files
        .iter()
        .map(|(suffix, bytes)| (format!("{label}{suffix}"), hash(bytes)))
        .collect();
    let base = directory.join("legacy-backups");
    private_directory(&base)?;
    let saved_receipt = match saved_receipt {
        Some(saved) => {
            ensure!(
                saved.sha256 == checksums,
                "Legacy secret-chat files changed since backup"
            );
            validate_backup(&base, &saved)?;
            saved
        }
        None => {
            let backup = format!("{label}-{}", chrono::Utc::now().format("%Y%m%dT%H%M%SZ"));
            let destination = base.join(&backup);
            fs::create_dir(&destination)
                .context("Cannot create a new secret-chat migration backup")?;
            crate::state::storage::restrict(&destination, true)?;
            for (suffix, bytes) in &files {
                let path = destination.join(format!("{label}{suffix}"));
                write_atomic(&path, bytes, false)?;
                let mut permissions = fs::metadata(&path)?.permissions();
                permissions.set_readonly(true);
                fs::set_permissions(path, permissions)?;
            }
            let saved = Receipt {
                version: 1,
                authorization: binding.digest(),
                backup,
                sha256: checksums,
            };
            validate_backup(&base, &saved)?;
            write_atomic(&receipt, &serde_json::to_vec(&saved)?, false)?;
            saved
        }
    };
    let bytes = key_bytes(&key, true)?;
    let stage = tempfile::tempdir_in(&directory)?;
    let staged_store = stage.path().join("state.enc");
    let mut store =
        EncryptedIngressStorage::open(&staged_store, *bytes, binding.clone(), MAX_BYTES)?;
    store
        .commit(&state)
        .context("Cannot commit migrated secret-chat state")?;
    drop(store);
    crate::state::storage::restrict(&staged_store, false)?;
    for (index, suffix) in SOURCES.into_iter().enumerate().take(3).skip(1) {
        if let Some(bytes) = files.get(suffix) {
            let output = directory.join(format!("{label}{}", SIDECARS[index]));
            if !plain_file(&output)? {
                write_atomic(&output, bytes, false)?;
            }
        }
    }
    // Publish the complete store last; the receipt permits an interrupted migration to resume.
    fs::hard_link(staged_store, &encrypted).context("Cannot publish migrated secret-chat state")?;
    #[cfg(unix)]
    File::open(&directory)?.sync_all()?;
    ensure!(
        saved_receipt.authorization == binding.digest(),
        "Secret-chat migration authorization changed"
    );
    Ok(Some(EncryptedIngressStorage::open(
        encrypted,
        *bytes,
        binding.clone(),
        MAX_BYTES,
    )?))
}

fn migration_lock(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .context("Cannot open secret-chat migration lock")?;
    plain_file(path)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .context("Secret-chat migration is locked by another process")?;
    crate::state::storage::restrict(path, false)?;
    Ok(file)
}

fn key_bytes(path: &Path, create: bool) -> Result<Zeroizing<[u8; 32]>> {
    if let Some(bytes) = read_private(path, 32)? {
        let bytes = Zeroizing::new(bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                "Secret-chat store key permissions are not owner-only"
            );
        }
        return Ok(Zeroizing::new(bytes.as_slice().try_into().map_err(
            |_| anyhow::anyhow!("Invalid secret-chat store key length"),
        )?));
    }
    ensure!(create, "Secret-chat store key is missing");
    let key = Zeroizing::new(rand::random::<[u8; 32]>());
    write_atomic(path, &*key, false)?;
    Ok(key)
}

fn validate_backup(base: &Path, receipt: &Receipt) -> Result<()> {
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
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    crate::lowercase_hex(&Sha256::digest(bytes))
}
fn parse(bytes: &[u8], message: &'static str) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!(message))
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .context("Invalid legacy secret-chat record shape")
}
fn id(text: &str) -> Result<i32> {
    let id = text
        .parse::<i32>()
        .map_err(|_| anyhow::anyhow!("Invalid legacy secret-chat identifier"))?;
    ensure!(
        id != 0 && id.to_string() == text,
        "Invalid legacy secret-chat identifier"
    );
    Ok(id)
}
fn field<T: serde::de::DeserializeOwned>(record: &Value, name: &str) -> Result<Option<T>> {
    if let Some(value) = record.get(name).filter(|value| !value.is_null()) {
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| anyhow::anyhow!("Invalid legacy secret-chat field: {name}"))
    } else {
        Ok(None)
    }
}
fn required<T: serde::de::DeserializeOwned>(record: &Value, name: &str) -> Result<T> {
    field(record, name)?.with_context(|| format!("Missing legacy secret-chat field: {name}"))
}
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let (pairs, remainder) = text.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}
fn bytes(value: &Value) -> Result<Option<Vec<u8>>> {
    if value.is_null() {
        return Ok(None);
    }
    if let Some(text) = value.get("__bytes__").and_then(Value::as_str) {
        return Ok(Some(
            decode_hex(text).context("Invalid legacy byte marker")?,
        ));
    }
    let text = value.as_str().context("Invalid legacy byte value")?;
    Ok(Some(
        decode_hex(text)
            .or_else(|| STANDARD.decode(text).ok())
            .context("Invalid legacy byte encoding")?,
    ))
}
fn big(value: &Value) -> Result<Option<Vec<u8>>> {
    match value {
        Value::Number(number) => Ok(Some(
            number
                .to_string()
                .parse::<num_bigint::BigUint>()
                .map_err(|_| anyhow::anyhow!("Invalid legacy DH integer"))?
                .to_bytes_be(),
        )),
        _ => bytes(value),
    }
}
fn exponent(value: &Value) -> Result<Option<SecretExponent>> {
    let Some(bytes) = big(value)? else {
        return Ok(None);
    };
    let bytes = Zeroizing::new(bytes);
    ensure!(
        bytes.len() <= 256 && bytes.iter().any(|byte| *byte != 0),
        "Invalid legacy DH exponent"
    );
    let mut padded = Zeroizing::new([0; 256]);
    padded[256 - bytes.len()..].copy_from_slice(&bytes);
    Ok(Some(SecretExponent::from_bytes(*padded)))
}
fn shared(value: &Value) -> Result<Option<SharedKey>> {
    bytes(value)?
        .map(|key| SharedKey::from_slice(&Zeroizing::new(key)).map_err(Into::into))
        .transpose()
}

fn convert(value: &Value, binding: &AuthorizationBinding) -> Result<State> {
    object(value)?;
    let mut state = State::empty(binding.digest())?;
    state.mode = StoreMode::Ingress;
    let incoming = object(&value["in"])?;
    let outgoing = object(&value["out"])?;
    let chats = object(&value["chats"])?;
    for label in incoming.keys().chain(outgoing.keys()) {
        ensure!(
            chats.contains_key(label),
            "Legacy secret-chat queue has no matching chat"
        );
    }
    for (key, row) in chats {
        let chat_id = id(key)?;
        let mut chat = convert_chat(row)?;
        ensure!(
            chat.id == chat_id,
            "Legacy secret-chat identifier does not match its record"
        );
        let own = if chat.is_outbound {
            chat.admin_id
        } else {
            chat.participant_id
        };
        ensure!(
            own.is_none_or(|id| id == binding.user_id()),
            "Legacy secret-chat record belongs to another user"
        );
        if let Some(queue) = incoming.get(key) {
            for item in queue.as_array().context("Invalid legacy incoming queue")? {
                let (seq, envelope) = incoming_row(item)?;
                ensure!(
                    chat.gaps.insert(seq, envelope).is_none(),
                    "Duplicate legacy incoming sequence"
                );
            }
        }
        if let Some(queue) = outgoing.get(key) {
            for item in queue.as_array().context("Invalid legacy outgoing queue")? {
                let row = Outgoing {
                    seq_no: required(item, "seq_no")?,
                    random_id: required(item, "random_id")?,
                    frame: bytes(&item["frame"])?.context("Missing legacy outgoing frame")?,
                    body: bytes(&item["body"])?.context("Missing legacy outgoing body")?,
                    failures: field(item, "failures")?.unwrap_or_default(),
                    service: required::<String>(item, "method")? == "service",
                    file: bytes(&item["file"])?,
                    pending: required(item, "pending")?,
                    obligation_generation: 0,
                };
                ensure!(
                    chat.outbox.insert(row.seq_no, row).is_none(),
                    "Duplicate legacy outgoing sequence"
                );
            }
        }
        if let Some(pending) = row.get("pending_deliveries").filter(|v| !v.is_null()) {
            for item in pending
                .as_array()
                .context("Invalid legacy delivery queue")?
            {
                let (seq, envelope) = incoming_row(item)?;
                let wrapper = kiaro_secret_chat::framing::unwrap(&envelope.body)
                    .context("Invalid legacy pending delivery")?;
                let service = matches!(
                    wrapper.message,
                    kiaro_secret_chat::enums::DecryptedMessage::Service(_)
                        | kiaro_secret_chat::enums::DecryptedMessage::ServiceLegacyaa48327d(_)
                );
                let old = if service {
                    chat.service_deliveries
                        .insert(
                            seq,
                            ServiceDelivery {
                                envelope,
                                applied: field(item, "applied")?,
                            },
                        )
                        .is_some()
                } else {
                    chat.deliveries.insert(seq, envelope).is_some()
                };
                ensure!(!old, "Duplicate legacy pending delivery");
            }
        }
        state.chats.insert(chat_id, chat);
    }
    state.settings = object(&value["settings"])?.clone().into_iter().collect();
    // The read-only backup retains original timestamps, reasons, and legacy-only fields.
    Ok(state)
}

fn incoming_row(row: &Value) -> Result<(i32, Envelope)> {
    Ok((
        required(row, "seq_no")?,
        Envelope {
            body: bytes(&row["body"])?.context("Missing legacy incoming body")?,
            file: bytes(&row["file"])?,
            date: field(row, "date")?.unwrap_or_default(),
        },
    ))
}

fn convert_chat(row: &Value) -> Result<Chat> {
    let state = match required::<String>(row, "state")?.as_str() {
        "requested" => ChatState::Requested,
        "pending" => ChatState::Requested,
        "waiting" => ChatState::Waiting,
        "ready" => ChatState::Ready,
        "rekeying" => ChatState::Rekeying,
        "suspended" => ChatState::Suspended,
        "closed" => ChatState::Closed,
        _ => bail!("Unknown legacy secret-chat state"),
    };
    let mut chat = Chat::new(
        required(row, "id")?,
        required(row, "access_hash")?,
        required(row, "peer_user_id")?,
        required(row, "is_outbound")?,
        state,
    );
    if chat.state == ChatState::Requested && chat.is_outbound {
        chat.state = ChatState::Waiting;
    }
    chat.admin_id = field(row, "admin_id")?;
    chat.participant_id = field(row, "participant_id")?;
    chat.created_at = timestamp(row, "created_at")?;
    chat.rekeyed_at = timestamp(row, "rekeyed_at")?;
    chat.messages_since_rekey = field(row, "messages_since_rekey")?.unwrap_or_default();
    chat.closed_reason =
        field::<String>(row, "closed_reason")?.map(|reason| match reason.as_str() {
            "the peer discarded the chat" | "peer_discarded" => CloseReason::PeerDiscarded,
            "closed by this application" | "application" => CloseReason::Application,
            "already closed" | "already_closed" => CloseReason::AlreadyClosed,
            _ => CloseReason::Protocol,
        });
    chat.key = shared(&row["key"])?;
    if let Some(fingerprint) = field(row, "key_fingerprint")? {
        ensure!(
            chat.key.as_ref().map(SharedKey::fingerprint) == Some(fingerprint),
            "Legacy secret-chat key fingerprint does not match"
        );
    }
    chat.previous_key = shared(&row["previous_key"])?;
    chat.initial_key_hash = bytes(&row["initial_key_hash"])?;
    let handshake = &row["handshake"];
    let dh_g = field(row, "dh_g")?.or(field(handshake, "g")?);
    let dh_prime = big(&row["dh_prime"])?.or(big(&handshake["p"])?);
    chat.dh = match (dh_g, dh_prime) {
        (Some(g), Some(prime)) => Some(DhRecord { g, prime }),
        (None, None) => None,
        _ => bail!("Incomplete legacy secret-chat DH parameters"),
    };
    chat.handshake_secret = exponent(&handshake["secret"])?;
    chat.peer_public = big(&handshake["g_a"])?;
    let exchange_id = field(row, "exchange_id")?;
    let role: Option<String> = field(row, "rekey_role")?;
    let exchange_secret = exponent(&row["exchange_secret"])?;
    let pending_key = shared(&row["pending_key"])?;
    chat.exchange = match (role.as_deref(), exchange_id, exchange_secret, pending_key) {
        (None | Some("committed"), _, None, None) => None,
        (None | Some("requested"), Some(id), Some(secret), None) => {
            Some(Exchange::Requested { id, secret })
        }
        (None | Some("accepted"), Some(id), _, Some(key)) => Some(Exchange::Accepted { id, key }),
        _ => bail!("Invalid legacy secret-chat rekey state"),
    };
    chat.in_seq_no = required(row, "in_seq_no")?;
    chat.out_seq_no = required(row, "out_seq_no")?;
    chat.peer_in_seq_no = field(row, "peer_in_seq_no")?.unwrap_or_default();
    chat.capability_layer = field(row, "layer")?.unwrap_or(46);
    chat.wrapper_layer = field(row, "wrapper_layer")?.unwrap_or_default();
    chat.ttl = field(row, "ttl")?.unwrap_or_default();
    chat.gap_end = field(row, "gap_end")?;
    chat.resend_due = field(row, "resend_due")?;
    chat.new_key_confirmed = field(row, "new_key_confirmed")?.unwrap_or_default();
    Ok(chat)
}

fn timestamp(row: &Value, field_name: &str) -> Result<u64> {
    let time = field::<f64>(row, field_name)?.unwrap_or_default();
    ensure!(
        (0.0..(u64::MAX as f64)).contains(&time),
        "Invalid legacy secret-chat timestamp"
    );
    Ok(time as u64)
}
