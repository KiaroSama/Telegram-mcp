use super::upstream_events::{integer_id, message_record};
use crate::server::access::{AccessPolicy, marked_peer};
use crate::{
    args,
    backend::Telegram,
    state::{FileAccess, State},
};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const FORMAT: &str = "telegram-mcp/unread-export-v1";
const EXHAUSTED: &str = "history_changed_or_exhausted_before_snapshot";

fn pretty_size<T: serde::Serialize + ?Sized>(value: &T, extra_indent: usize) -> Result<usize> {
    #[derive(Default)]
    struct Counter {
        bytes: usize,
        newlines: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(bytes.len());
            self.newlines = self
                .newlines
                .saturating_add(bytes.iter().filter(|byte| **byte == b'\n').count());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter::default();
    serde_json::to_writer_pretty(&mut counter, value)?;
    Ok(counter
        .bytes
        .saturating_add(counter.newlines.saturating_mul(extra_indent)))
}

fn chat_row(
    id: &str,
    expected: i64,
    top: i64,
    count: usize,
    cursor: i64,
    complete: bool,
    reason: Option<&str>,
) -> Value {
    json!({"chat_id":id,"unread_count_at_export":expected,"snapshot_top_message":top,"messages_exported":count,"messages":[],"next_after_id":cursor,"complete":complete,"incomplete_reason":reason})
}

async fn authenticated_user(api: &dyn Telegram) -> Result<String> {
    let response = api
        .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
        .await
        .context("Cannot verify authenticated Telegram user identity")?;
    let users = response
        .as_array()
        .context("Telegram self identity is invalid")?;
    if users.len() != 1 || users[0]["_"] != "user" || users[0]["self"] != true {
        bail!("Telegram self identity is missing or ambiguous");
    }
    let id = integer_id(&users[0]["id"]).context("Telegram self identity has no valid user id")?;
    if id <= 0 {
        bail!("Telegram self identity has no valid user id");
    }
    Ok(id.to_string())
}

fn output_lease(state: &State, output: &std::path::Path) -> Result<std::fs::File> {
    // A stable private sidecar serializes cooperating writers without locking
    // the data bytes: Windows byte-range locks also prohibit another read handle.
    let digest = Sha256::digest(output.as_os_str().as_encoded_bytes());
    let path = state.root().join(format!("unread-export-{digest:x}.lock"));
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        bail!("Unread export lease is not a regular file");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let lease = options.open(&path)?;
    crate::state::storage::restrict(&path, false)?;
    FileExt::try_lock_exclusive(&lease).context("This unread export is already in use")?;
    Ok(lease)
}

fn bounded_message_id(value: &Value) -> Result<i64> {
    let id = integer_id(value)?;
    if !(0..=i32::MAX as i64).contains(&id) {
        bail!("Unread export contains an invalid nonnegative message count or id");
    }
    Ok(id)
}

fn validate_previous(value: &Value) -> Result<()> {
    for (chat, row) in value["chats"]
        .as_object()
        .context("Unread export chats must be an object")?
    {
        chat.parse::<i64>()
            .context("Unread export has an invalid chat identity")?;
        let messages = row["messages"]
            .as_array()
            .context("Unread export messages must be an array; existing file was preserved")?;
        row["complete"]
            .as_bool()
            .context("Unread export completion status is invalid")?;
        for field in [
            "unread_count_at_export",
            "snapshot_top_message",
            "next_after_id",
        ] {
            bounded_message_id(&row[field])?;
        }
        let mut seen = std::collections::BTreeSet::new();
        for message in messages {
            let id = bounded_message_id(&message["id"])?;
            if id == 0 || !message.is_object() || !seen.insert(id) {
                bail!(
                    "Unread export contains invalid or duplicate messages; existing file was preserved"
                );
            }
        }
    }
    Ok(())
}

pub async fn export_unread_with_state(
    api: &dyn Telegram,
    state: &State,
    input: &Value,
) -> Result<Value> {
    let chats = args::required(input, "chat_ids")?
        .as_array()
        .context("chat_ids must be an array")?;
    if chats.is_empty() || chats.len() > 100 {
        bail!("chat_ids must contain 1-100 chats");
    }
    let limit = args::limit(input, "limit", 10_000, 10_000)? as usize;
    let account = input["account"].as_str().unwrap_or("default");
    let resume = args::boolean(input, "resume", true)?;
    let media = args::boolean(input, "include_media_metadata", true)?;
    let output = state.resolve_file(
        std::path::Path::new(args::text(input, "output_path")?),
        FileAccess::Write,
    )?;
    if output.is_dir() {
        bail!("The unread export destination must be a file");
    }
    let _lease = output_lease(state, &output)?;
    let mut previous_bytes = None;
    let mut result = if output.exists() {
        if !resume {
            bail!(
                "Output already exists; choose a new path or resume this recognized unread export"
            );
        }
        let source = state.resolve_file(&output, FileAccess::Read)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(source)?;
        if file.metadata()?.len() > MAX_BYTES as u64 {
            bail!("Existing unread export exceeds its byte budget");
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            bail!("Existing unread export exceeds its byte budget");
        }
        let old: Value = serde_json::from_str(
            std::str::from_utf8(&bytes).context("Existing unread export must be UTF-8")?,
        )
        .context("Existing unread export is invalid; nothing was overwritten")?;
        if old["format"] != FORMAT
            || old["account"].as_str() != Some(account)
            || !old["chats"].is_object()
        {
            bail!(
                "The existing file is not this account's recognized unread export; nothing was overwritten"
            );
        }
        validate_previous(&old)?;
        previous_bytes = Some(bytes);
        old
    } else {
        json!({"format":FORMAT,"account":account,"chats":{}})
    };
    let previous_user = if previous_bytes.is_some() {
        let id = integer_id(&result["telegram_user_id"])
            .context("This unread export has no authenticated Telegram user identity; choose a new output path")?;
        if id <= 0 {
            bail!("This unread export has no valid authenticated Telegram user identity");
        }
        Some(id.to_string())
    } else {
        None
    };
    AccessPolicy::from_environment()?
        .check("export_unread_messages", input, api)
        .await?;
    let user = authenticated_user(api).await?;
    if previous_user
        .as_ref()
        .is_some_and(|previous| previous != &user)
    {
        bail!("This unread export belongs to another Telegram user; existing output was preserved");
    }
    result["telegram_user_id"] = user.into();
    let mut exported = 0;
    let mut processed = 0;
    let mut skipped = 0;
    let mut budget = pretty_size(&result, 0)?.saturating_add(1);
    if budget > MAX_BYTES {
        bail!(
            "Existing unread export exceeds its encoded byte budget; existing output was preserved"
        );
    }
    let mut stopped_for_budget = false;
    for raw in chats {
        if exported >= limit || budget >= MAX_BYTES {
            stopped_for_budget = budget >= MAX_BYTES;
            break;
        }
        let peer = api.resolve_peer(raw).await?;
        let id = marked_peer(&peer).context("An export chat has no marked identity")?;
        if resume && result["chats"][&id]["complete"] == json!(true) {
            skipped += 1;
            continue;
        }
        let dialog_result = api
            .invoke(
                "messages.getPeerDialogs",
                json!({"peers":[{"_":"inputDialogPeer","peer":peer}]}),
            )
            .await?;
        let dialog = dialog_result["dialogs"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| marked_peer(&row["peer"]).as_deref() == Some(&id))
            })
            .context("Telegram did not return the requested dialog state")?;
        let unread = bounded_message_id(&dialog["unread_count"])?;
        let read_max = bounded_message_id(&dialog["read_inbox_max_id"])?;
        let top = bounded_message_id(&dialog["top_message"])?;
        let previous = &result["chats"][&id];
        let expected = previous
            .get("unread_count_at_export")
            .map(integer_id)
            .transpose()?
            .unwrap_or(unread);
        let snapshot_top = previous
            .get("snapshot_top_message")
            .map(integer_id)
            .transpose()?
            .unwrap_or(top);
        let mut cursor = previous
            .get("next_after_id")
            .map(integer_id)
            .transpose()?
            .unwrap_or(read_max);
        let mut messages = previous["messages"].as_array().cloned().unwrap_or_default();
        let mut complete = expected == 0;
        let mut reason = None;
        // Measure the actual pretty encoding. A chat row is nested two objects
        // deep; its messages array is three deep. Count old messages once, then
        // admit each new record by its exact embedded size instead of repeatedly
        // serializing a growing multi-megabyte snapshot.
        let old_row = result["chats"].get(&id).cloned();
        let metadata = chat_row(
            &id,
            expected,
            snapshot_top,
            messages.len(),
            cursor,
            complete,
            if complete { None } else { Some(EXHAUSTED) },
        );
        result["chats"][&id] = metadata.clone();
        let outside_row = pretty_size(&result, 0)?.saturating_add(1) - pretty_size(&metadata, 4)?;
        let mut message_bytes = pretty_size(&messages, 6)? - 2;
        let candidate = outside_row + pretty_size(&metadata, 4)? + message_bytes;
        if candidate > MAX_BYTES {
            if let Some(old) = old_row {
                result["chats"][&id] = old;
            } else {
                result["chats"]
                    .as_object_mut()
                    .expect("Validated chats object")
                    .remove(&id);
            }
            stopped_for_budget = true;
            break;
        }
        budget = candidate;
        while !complete && exported < limit && budget < MAX_BYTES {
            if cursor >= i32::MAX as i64 {
                complete = true;
                break;
            }
            let batch = (limit - exported).min(100) as i64;
            let max_id = if snapshot_top < i32::MAX as i64 {
                snapshot_top + 1
            } else {
                0
            };
            let page=api.invoke("messages.getHistory",json!({"peer":peer,"offset_id":cursor+1,"offset_date":0,"add_offset":-batch,"limit":batch,"max_id":max_id,"min_id":cursor,"hash":"0"})).await?;
            let mut fetched = page["messages"]
                .as_array()
                .context("Telegram did not return an unread message page")?
                .clone();
            fetched.sort_by_key(|message| integer_id(&message["id"]).unwrap_or(0));
            let mut progressed = false;
            for message in &fetched {
                let message_id = bounded_message_id(&message["id"])?;
                if message_id <= cursor || snapshot_top > 0 && message_id > snapshot_top {
                    continue;
                }
                if message["_"] == "messageEmpty" || message["out"] == json!(true) {
                    let done = snapshot_top > 0 && message_id >= snapshot_top;
                    let metadata = chat_row(
                        &id,
                        expected,
                        snapshot_top,
                        messages.len(),
                        message_id,
                        done,
                        if done { None } else { Some(EXHAUSTED) },
                    );
                    let candidate = outside_row + pretty_size(&metadata, 4)? + message_bytes;
                    if candidate > MAX_BYTES {
                        reason = Some("byte_budget");
                        break;
                    }
                    budget = candidate;
                    cursor = message_id;
                    progressed = true;
                    complete = done;
                    if complete {
                        break;
                    }
                    continue;
                }
                let record = message_record(message, &page, &peer, media);
                // The first item creates the array's closing indentation. Later
                // items reuse that line and add only a comma/newline separator.
                let bytes = pretty_size(std::slice::from_ref(&record), 6)?
                    - 2
                    - if messages.is_empty() { 0 } else { 6 };
                let done = (messages.len() + 1) as i64 >= expected
                    || snapshot_top > 0 && message_id >= snapshot_top;
                let metadata = chat_row(
                    &id,
                    expected,
                    snapshot_top,
                    messages.len() + 1,
                    message_id,
                    done,
                    if done { None } else { Some(EXHAUSTED) },
                );
                let candidate = outside_row + pretty_size(&metadata, 4)? + message_bytes + bytes;
                if candidate > MAX_BYTES {
                    reason = Some("byte_budget");
                    break;
                }
                budget = candidate;
                message_bytes += bytes;
                messages.push(record);
                exported += 1;
                cursor = message_id;
                progressed = true;
                complete = done;
                if complete || exported >= limit {
                    break;
                }
            }
            complete =
                messages.len() as i64 >= expected || snapshot_top > 0 && cursor >= snapshot_top;
            if reason.is_some() {
                break;
            }
            if !progressed || (fetched.len() as i64) < batch {
                if !complete {
                    reason = Some(EXHAUSTED);
                }
                break;
            }
        }
        if !complete && reason.is_none() {
            reason = Some(if budget >= MAX_BYTES {
                "byte_budget"
            } else {
                "message_limit"
            });
        }
        let mut row = chat_row(
            &id,
            expected,
            snapshot_top,
            messages.len(),
            cursor,
            complete,
            reason,
        );
        budget = outside_row + pretty_size(&row, 4)? + message_bytes;
        row["messages"] = messages.into();
        result["chats"][&id] = row;
        processed += 1;
        if reason == Some("byte_budget") {
            stopped_for_budget = true;
            break;
        }
    }
    let bytes = serde_json::to_vec_pretty(&result)?;
    if bytes.len().saturating_add(1) > MAX_BYTES {
        bail!("Unread export exceeded its final byte budget; existing output was preserved");
    }
    let parent = output
        .parent()
        .context("Unread export destination has no parent")?;
    std::fs::create_dir_all(parent)?;
    if state.resolve_file(&output, FileAccess::Write)? != output {
        bail!("Unread export destination changed during the read; no output was replaced");
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    crate::state::storage::restrict(temporary.path(), false)?;
    temporary.write_all(&bytes)?;
    temporary.write_all(b"\n")?;
    temporary.as_file_mut().sync_all()?;
    if let Some(before) = previous_bytes {
        let current = std::fs::read(&output)?;
        if Sha256::digest(&current) != Sha256::digest(&before) {
            bail!("Unread export changed while being resumed; the external update was preserved");
        }
        temporary
            .persist(&output)
            .map_err(|_| anyhow::anyhow!("Could not atomically update this unread export"))?;
    } else {
        temporary.persist_noclobber(&output).map_err(|_| {
            anyhow::anyhow!("Unread export destination already exists; nothing was overwritten")
        })?;
    }
    let incomplete = result["chats"]
        .as_object()
        .context("Unread export chats are invalid")?
        .values()
        .filter(|chat| chat["complete"] != json!(true))
        .count();
    Ok(
        json!({"status":"ok","output_path":output,"chats_processed":processed,"chats_skipped":skipped,"messages_exported":exported,"complete":incomplete==0&&processed+skipped==chats.len(),"incomplete_chats":incomplete,"incomplete_reason":if stopped_for_budget {Some("byte_budget")} else {None},"read_markers_sent":false}),
    )
}
