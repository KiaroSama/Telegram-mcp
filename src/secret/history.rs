use super::{MAX_BYTES, metadata, read_private, write_atomic};
use crate::state::{FileAccess, State};
use anyhow::{Context, Result, ensure};
use kiaro_secret_chat::{
    enums::DecryptedMessageAction,
    manager::{Event, MessageReceived},
    media::MediaReference,
};
use serde_json::{Map, Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct SecretHistory {
    history: PathBuf,
    media: PathBuf,
    settings: PathBuf,
    lock: Mutex<()>,
}

impl SecretHistory {
    pub(crate) fn open(
        directory: &Path,
        label: &str,
        legacy_settings: &std::collections::BTreeMap<String, Value>,
    ) -> Result<Self> {
        let history = Self {
            history: directory.join(format!("{label}.history.v1.json")),
            media: directory.join(format!("{label}.media.v1.json")),
            settings: directory.join(format!("{label}.archive.v1.json")),
            lock: Mutex::new(()),
        };
        for file in [&history.history, &history.media] {
            load(file)?;
        }
        if !super::plain_file(&history.settings)?
            && let Some(value) = legacy_settings
                .get("auto_save_secret_chats")
                .or_else(|| legacy_settings.get("auto_save"))
            && let Some(folder) = value["folder"].as_str().filter(|_| value["on"] == true)
        {
            save(
                &history.settings,
                &Map::from_iter([("folder".into(), json!(folder))]),
            )?;
        }
        load(&history.settings)?;
        Ok(history)
    }

    fn guard(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Secret-chat history lock is unavailable"))
    }

    pub fn append(
        &self,
        chat_id: i32,
        row: Value,
        reference: Option<&MediaReference>,
    ) -> Result<()> {
        ensure!(
            chat_id != 0 && row.is_object(),
            "Invalid secret-chat history entry"
        );
        let _guard = self.guard()?;
        let key = chat_id.to_string();
        if forgotten(&load(&self.settings)?, &key) {
            return Ok(());
        }
        self.append_locked(key, row, reference)
    }

    fn append_locked(
        &self,
        key: String,
        row: Value,
        reference: Option<&MediaReference>,
    ) -> Result<()> {
        let id = message_id(&row);
        let ttl = row["self_destructs_after_seconds"].as_u64().unwrap_or(0);
        if let Some(reference) = reference {
            let id = id.context("Secret-chat media entry needs a message ID")?;
            let mut media = load(&self.media)?;
            let records = media.entry(&key).or_insert_with(|| json!({}));
            let records = records
                .as_object_mut()
                .context("Invalid secret-chat media registry")?;
            let id = id.to_string();
            let reference = reference.to_json()?;
            if !records.get(&id).is_some_and(|saved| {
                saved["reference"] == reference && saved["ttl"].as_u64() == Some(ttl)
            }) {
                records.insert(
                    id,
                    json!({"reference": reference, "ttl": ttl, "received_at": row["date"]}),
                );
                save(&self.media, &media)?;
            }
        }
        if ttl > 0 {
            return Ok(());
        }
        let mut history = load(&self.history)?;
        let rows = history.entry(key).or_insert_with(|| json!([]));
        let rows = rows
            .as_array_mut()
            .context("Invalid secret-chat history rows")?;
        if id.is_some() && rows.iter().any(|prior| message_id(prior) == id) {
            return Ok(());
        }
        if id.is_none()
            && rows
                .last()
                .is_some_and(|prior| prior["action"] == row["action"] && !row["action"].is_null())
        {
            return Ok(());
        }
        rows.push(row);
        save(&self.history, &history)
    }

    pub(super) fn commit_delivery(
        &self,
        chat_id: i32,
        seq: i32,
        row: Value,
        reference: Option<&MediaReference>,
    ) -> Result<()> {
        let id = message_id(&row).context("Secret-chat delivery has no message ID")?;
        ensure!(
            row["self_destructs_after_seconds"].as_u64().unwrap_or(0) == 0,
            "Timed secret-chat deliveries cannot be persisted into ordinary consumer snapshots; delivery left pending"
        );
        let _guard = self.guard()?;
        let mut settings = load(&self.settings)?;
        let chat = chat_id.to_string();
        ensure!(
            !forgotten(&settings, &chat),
            "Secret chat was forgotten locally"
        );
        let receipts = settings
            .entry("delivery_receipts")
            .or_insert_with(|| json!({}));
        let key = format!("{chat_id}:{seq}");
        if let Some(saved) = receipts.get(&key) {
            ensure!(
                saved.as_i64() == Some(id),
                "Secret-chat delivery identity changed"
            );
        }
        self.append_locked(chat, row, reference)?;
        receipts
            .as_object_mut()
            .context("Invalid secret-chat delivery receipts")?
            .insert(key, json!(id));
        save(&self.settings, &settings)
    }

    pub(super) fn delivery_committed(&self, chat_id: i32, seq: i32) -> Result<bool> {
        let _guard = self.guard()?;
        Ok(load(&self.settings)?
            .get("delivery_receipts")
            .and_then(|receipts| receipts.get(format!("{chat_id}:{seq}")))
            .and_then(Value::as_i64)
            .is_some())
    }

    pub fn read(&self, chat_id: i32, limit: usize) -> Result<Vec<Value>> {
        let _guard = self.guard()?;
        let history = load(&self.history)?;
        Ok(history
            .get(&chat_id.to_string())
            .and_then(Value::as_array)
            .map(|rows| rows.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default())
    }

    pub fn search(&self, chat_id: Option<i32>, query: &str, limit: usize) -> Result<Vec<Value>> {
        ensure!(
            !query.trim().is_empty(),
            "Secret-chat search query is empty"
        );
        let _guard = self.guard()?;
        let query = query.to_lowercase();
        let history = load(&self.history)?;
        let mut hits = Vec::new();
        for (id, rows) in history {
            let id: i32 = id
                .parse()
                .context("Invalid secret-chat history identifier")?;
            if chat_id.is_some_and(|chat| chat != id) {
                continue;
            }
            for row in rows
                .as_array()
                .context("Invalid secret-chat history rows")?
                .iter()
                .rev()
            {
                if ["text", "caption"].iter().any(|name| {
                    row[name]
                        .as_str()
                        .is_some_and(|text| text.to_lowercase().contains(&query))
                }) {
                    let mut row = row.clone();
                    row["chat_id"] = json!(id);
                    hits.push(row);
                }
            }
        }
        hits.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn media(&self, chat_id: i32, message_id: i64) -> Result<MediaReference> {
        self.media_with_restriction(chat_id, message_id, true)
    }

    pub fn media_with_restriction(
        &self,
        chat_id: i32,
        message_id: i64,
        honour: bool,
    ) -> Result<MediaReference> {
        let _guard = self.guard()?;
        let records = load(&self.media)?;
        let key = chat_id.to_string();
        let record = records
            .get(&key)
            .and_then(|rows| rows.get(message_id.to_string()))
            .context("No secret-chat media is saved for this message")?;
        ensure!(
            !honour || record["ttl"].as_u64().unwrap_or(0) == 0,
            "This message arrived under a self-destruct timer"
        );
        let reference = MediaReference::from_json(record["reference"].clone())
            .context("This saved media reference is not compatible with the native backend")?;
        ensure!(
            reference.chat_id == chat_id,
            "Saved secret media belongs to a different chat"
        );
        Ok(reference)
    }

    pub fn delete_saved(&self, chat_id: i32, ids: Option<&[i64]>) -> Result<usize> {
        let _guard = self.guard()?;
        let mut media = load(&self.media)?;
        let mut history = load(&self.history)?;
        let key = chat_id.to_string();
        let count = match ids {
            Some(ids) => {
                if let Some(records) = media.get_mut(&key) {
                    let records = records
                        .as_object_mut()
                        .context("Invalid secret-chat media registry")?;
                    for id in ids {
                        records.remove(&id.to_string());
                    }
                }
                if let Some(rows) = history.get_mut(&key) {
                    let rows = rows
                        .as_array_mut()
                        .context("Invalid secret-chat history rows")?;
                    let before = rows.len();
                    rows.retain(|row| message_id(row).is_none_or(|id| !ids.contains(&id)));
                    before - rows.len()
                } else {
                    0
                }
            }
            None => {
                media.remove(&key);
                history
                    .remove(&key)
                    .and_then(|rows| rows.as_array().map(Vec::len))
                    .unwrap_or(0)
            }
        };
        save(&self.media, &media)?;
        save(&self.history, &history)?;
        Ok(count)
    }

    pub fn clear(&self, chat_id: i32) -> Result<usize> {
        self.delete_saved(chat_id, None)
    }

    pub fn erase_local_chat(&self, chat_id: i32) -> Result<usize> {
        let count = self.clear(chat_id)?;
        self.forget_chat(chat_id)?;
        Ok(count)
    }

    pub(super) fn forget_chat(&self, chat_id: i32) -> Result<()> {
        let _guard = self.guard()?;
        let key = chat_id.to_string();
        let mut media = load(&self.media)?;
        if media.remove(&key).is_some() {
            save(&self.media, &media)?;
        }
        let mut settings = load(&self.settings)?;
        settings.entry("forgotten").or_insert_with(|| json!({}))[key] = json!(true);
        save(&self.settings, &settings)
    }

    pub fn is_forgotten(&self, chat_id: i32) -> Result<bool> {
        let _guard = self.guard()?;
        Ok(forgotten(&load(&self.settings)?, &chat_id.to_string()))
    }

    pub fn auto_save_folder(&self) -> Result<Option<PathBuf>> {
        let _guard = self.guard()?;
        Ok(load(&self.settings)?
            .get("folder")
            .and_then(Value::as_str)
            .map(PathBuf::from))
    }

    pub fn set_auto_save_folder(
        &self,
        state: &State,
        folder: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        let folder = folder.map(|path| mkdir(state, path)).transpose()?;
        let _guard = self.guard()?;
        let mut settings = load(&self.settings)?;
        if let Some(previous) = settings.remove("folder") {
            let folders = settings
                .entry("archive_folders")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("Invalid secret-chat archive folders")?;
            if !folders.contains(&previous) {
                folders.push(previous);
            }
        }
        if let Some(folder) = &folder {
            settings.insert("folder".into(), serde_json::to_value(folder)?);
        }
        save(&self.settings, &settings)?;
        Ok(folder)
    }

    pub(super) fn archive_rows(
        &self,
        state: &State,
        chat_id: i32,
    ) -> Result<Vec<(Value, PathBuf)>> {
        let _guard = self.guard()?;
        let settings = load(&self.settings)?;
        let mut rows = std::collections::BTreeMap::new();
        let folders = settings
            .get("archive_folders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(settings.get("folder"));
        for folder in folders {
            let folder = Path::new(
                folder
                    .as_str()
                    .context("Invalid secret-chat archive folder")?,
            );
            let directory = folder.join(chat_id.to_string());
            let resolved = state.resolve_file(&directory, FileAccess::Write)?;
            if metadata(&directory)?.is_none() {
                continue;
            }
            let directory = resolved;
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                let Some(id) = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.parse::<i64>().ok())
                else {
                    continue;
                };
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                super::plain_file(&path)?;
                let path = state.resolve_file(&path, FileAccess::Read)?;
                ensure!(
                    path.starts_with(&directory),
                    "Secret-chat archive row is outside its directory"
                );
                let bytes = read_private(&path, MAX_BYTES)?
                    .context("Secret-chat archive row is missing")?;
                let row: Value =
                    serde_json::from_slice(&bytes).context("Invalid secret-chat archive row")?;
                ensure!(
                    message_id(&row) == Some(id),
                    "Secret-chat archive row ID does not match its filename"
                );
                if row["self_destructs_after_seconds"].as_u64().unwrap_or(0) == 0 {
                    rows.insert(id, (row, directory.clone()));
                }
            }
        }
        let mut rows: Vec<_> = rows.into_values().collect();
        rows.sort_by(|(a, _), (b, _)| a["date"].as_str().cmp(&b["date"].as_str()));
        Ok(rows)
    }

    pub(super) fn delete_saved_files(
        &self,
        state: &State,
        chat_id: i32,
        selected: Option<&[i64]>,
    ) -> Result<(usize, Vec<String>)> {
        let _guard = self.guard()?;
        let settings = load(&self.settings)?;
        let wanted: Option<std::collections::HashSet<i64>> =
            selected.map(|ids| ids.iter().copied().collect());
        let mut removed = 0usize;
        let mut failed = Vec::new();
        let folders = settings
            .get("archive_folders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(settings.get("folder"));
        for folder in folders {
            let folder = Path::new(
                folder
                    .as_str()
                    .context("Invalid secret-chat archive folder")?,
            );
            let directory = folder.join(chat_id.to_string());
            let resolved = state.resolve_file(&directory, FileAccess::Write)?;
            if metadata(&directory)?.is_none() {
                continue;
            }
            let prefix = format!("secret-{chat_id}-");
            for entry in fs::read_dir(&resolved)? {
                let path = entry?.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let id = if let Some(stem) = name.strip_suffix(".json") {
                    stem.parse::<i64>().ok()
                } else if let Some(rest) = name.strip_prefix(&prefix) {
                    rest.strip_suffix(".bin")
                        .and_then(|s| s.parse::<i64>().ok())
                } else {
                    None
                };
                let Some(id) = id else {
                    continue;
                };
                if wanted.as_ref().is_some_and(|set| !set.contains(&id)) {
                    continue;
                }
                if super::plain_file(&path).unwrap_or(false)
                    && state.resolve_file(&path, FileAccess::Write).is_ok()
                {
                    match fs::remove_file(&path) {
                        Ok(()) => removed += 1,
                        Err(_) => failed.push(path.display().to_string()),
                    }
                } else {
                    failed.push(path.display().to_string());
                }
            }
        }
        Ok((removed, failed))
    }

    pub(crate) fn archive_row(
        &self,
        state: &State,
        chat_id: i32,
        row: &Value,
    ) -> Result<Option<PathBuf>> {
        if row["self_destructs_after_seconds"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            return Ok(None);
        }
        let _guard = self.guard()?;
        let settings = load(&self.settings)?;
        if forgotten(&settings, &chat_id.to_string()) {
            return Ok(None);
        }
        let Some(folder) = settings.get("folder").and_then(Value::as_str) else {
            return Ok(None);
        };
        let folder = mkdir(state, &Path::new(folder).join(chat_id.to_string()))?;
        if let Some(id) = message_id(row) {
            let path = state.resolve_file(&folder.join(format!("{id}.json")), FileAccess::Write)?;
            if !super::plain_file(&path)? {
                write_atomic(&path, &serde_json::to_vec_pretty(row)?, false)?;
            }
        }
        Ok(Some(folder))
    }

    pub(crate) fn record_message(&self, message: &MessageReceived) -> Result<Value> {
        let row = row(
            message.random_id,
            false,
            &message.text,
            message.ttl,
            message.media_reference.is_some(),
        );
        self.append(
            message.chat_id,
            row.clone(),
            message.media_reference.as_ref(),
        )?;
        Ok(row)
    }

    pub(crate) fn record_service(&self, event: Event) -> Result<()> {
        let Event::ServiceActionReceived {
            chat_id,
            action_name,
            action,
            applied,
        } = event
        else {
            return Ok(());
        };
        if applied {
            match action {
                DecryptedMessageAction::DeleteMessages(action) => {
                    self.delete_saved(chat_id, Some(&action.random_ids))?;
                }
                DecryptedMessageAction::FlushHistory => {
                    self.clear(chat_id)?;
                }
                _ => {}
            }
        }
        self.append(chat_id, json!({"type":"service", "action":action_name, "applied":applied, "date":chrono::Utc::now().to_rfc3339()}), None)
    }
}

pub(super) fn row(id: i64, outgoing: bool, text: &str, ttl: i32, media: bool) -> Value {
    let mut row = json!({"message_id": id, "is_outgoing": outgoing, "can_be_saved": ttl == 0, "date":chrono::Utc::now().to_rfc3339(), "type": if media {"document"} else {"text"}});
    row[if media { "caption" } else { "text" }] = json!(text);
    if ttl > 0 {
        row["self_destructs_after_seconds"] = json!(ttl);
    }
    row
}

pub(super) fn message_id(row: &Value) -> Option<i64> {
    row["message_id"]
        .as_i64()
        .or_else(|| row["message_id"].as_str()?.parse().ok())
}

fn forgotten(settings: &Map<String, Value>, key: &str) -> bool {
    settings.get("forgotten").and_then(|ids| ids.get(key)) == Some(&Value::Bool(true))
}

fn load(path: &Path) -> Result<Map<String, Value>> {
    let Some(bytes) = read_private(path, MAX_BYTES)? else {
        return Ok(Map::new());
    };
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Secret-chat history or media registry is not valid JSON"))
}

fn save(path: &Path, rows: &Map<String, Value>) -> Result<()> {
    let bytes = serde_json::to_vec(rows)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "Secret-chat history exceeds its byte budget"
    );
    write_atomic(path, &bytes, true)
}

pub(super) fn mkdir(state: &State, directory: &Path) -> Result<PathBuf> {
    let path = state.resolve_file(directory, FileAccess::Write)?;
    if metadata(&path)?.is_none() {
        fs::create_dir_all(&path)?;
    }
    let path = state.resolve_file(&path, FileAccess::Write)?;
    ensure!(
        metadata(&path)?.is_some_and(|m| m.is_dir()),
        "Secret-chat archive destination must be a directory"
    );
    Ok(path)
}
