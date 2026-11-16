//! Bounded custom-emoji media access; fallback glyphs are never rendered as assets.
use super::common::*;
use crate::tools::messaging::media::{download_bytes, download_location, previews};
use crate::{
    args as a,
    backend::Telegram,
    state::{FileAccess, State},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub(super) async fn bytes(
    api: &dyn Telegram,
    doc: &Value,
    thumbnail: bool,
    max: u64,
) -> Result<(Vec<u8>, bool)> {
    if thumbnail
        && let Some(thumb) = doc["thumbs"]
            .as_array()
            .and_then(|v| v.iter().rev().find(|t| t["_"] != "photoSizeEmpty"))
    {
        let result = if thumb.get("bytes").is_some() {
            previews::inline_thumbnail(thumb)
        } else {
            let mut location = download_location(doc)?;
            location["thumb_size"] = thumb["type"].clone();
            download_bytes(api, location, max).await
        };
        if let Ok(data) = result {
            ensure!(
                data.len() as u64 <= max,
                "Emoji thumbnail exceeds max_bytes"
            );
            if !data.is_empty() {
                return Ok((data, true));
            }
        }
    }
    let data = download_bytes(api, download_location(doc)?, max).await?;
    ensure!(
        !data.is_empty(),
        "Telegram returned an empty custom emoji file"
    );
    Ok((data, false))
}
pub(super) fn suffix(data: &[u8]) -> &'static str {
    if data.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if data.starts_with(b"\x89PNG") {
        "png"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "webp"
    } else if data.starts_with(b"\x1f\x8b") {
        "tgs"
    } else if data.starts_with(b"GIF8") {
        "gif"
    } else if data.starts_with(b"\x1aE\xdf\xa3") {
        "webm"
    } else {
        "bin"
    }
}
pub(super) fn save(state: &State, path: &Path, data: &[u8], document_id: i64) -> Result<PathBuf> {
    let mut path = state.resolve_file(path, FileAccess::Write)?;
    if path.is_dir() {
        path.push(format!("custom_emoji_{document_id}.{}", suffix(data)));
    }
    path = state.resolve_file(&path, FileAccess::Write)?;
    ensure!(
        !path.exists(),
        "Emoji destination already exists and was not overwritten"
    );
    let parent = path
        .parent()
        .context("Emoji destination has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    crate::state::storage::restrict(temp.path(), false)?;
    temp.write_all(data)?;
    temp.as_file_mut().sync_all()?;
    ensure!(
        state.resolve_file(&path, FileAccess::Write)? == path,
        "Emoji destination changed while preparing the file"
    );
    temp.persist_noclobber(&path)
        .map_err(|_| anyhow::anyhow!("Could not save emoji without replacing an existing file"))?;
    Ok(path)
}
pub(super) async fn preview(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let wanted = positive(a::required(args, "document_id")?)?;
    let thumbnail = a::boolean(args, "thumbnail", true)?;
    let max = a::integer(args, "max_bytes", 8 * 1024 * 1024)?;
    ensure!(
        (1..=50 * 1024 * 1024).contains(&max),
        "max_bytes must be 1..=50 MiB"
    );
    let dimension = a::integer(args, "max_dimension", 512)?;
    ensure!(
        (64..=1568).contains(&dimension),
        "max_dimension must be 64..=1568"
    );
    let count = a::limit(args, "count", 1, 12)?;
    let destination = if let Some(path) = args.get("save_path").filter(|v| !v.is_null()) {
        let state = State::global()?;
        let path = state.resolve_file(
            Path::new(path.as_str().context("save_path must be a string")?),
            FileAccess::Write,
        )?;
        ensure!(
            !path.exists() || path.is_dir(),
            "Emoji destination already exists and was not overwritten"
        );
        Some((state, path))
    } else {
        None
    };
    let docs = api
        .invoke(
            "messages.getCustomEmojiDocuments",
            json!({"document_id":[wanted.to_string()]}),
        )
        .await?;
    let doc = docs
        .as_array()
        .and_then(|v| v.iter().find(|d| id(&d["id"]).ok() == Some(wanted)))
        .context("Custom emoji document was not found")?;
    let (data, used_thumbnail) = bytes(api, doc, thumbnail, max as u64).await?;
    let data = std::sync::Arc::new(data);
    let saved = if let Some((state, path)) = destination {
        static SAVES: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let permit = std::sync::Arc::clone(
            SAVES.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2))),
        )
        .acquire_owned()
        .await
        .context("Emoji file worker unavailable")?;
        let data = std::sync::Arc::clone(&data);
        Some(
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                save(&state, &path, &data, wanted)
            })
            .await
            .context("Emoji file worker stopped")??,
        )
    } else {
        None
    };
    match previews::render(&data, count, dimension).await {
        Ok(mut out) => {
            out["document_id"] = json!(wanted.to_string());
            out["thumbnail_used"] = json!(used_thumbnail);
            out["saved_path"] = json!(saved);
            Ok(out)
        }
        Err(error) if saved.is_some() => Ok(
            json!({"document_id":wanted.to_string(),"saved_path":saved,"thumbnail_used":used_thumbnail,"bytes":data.len(),"format":suffix(&data),"preview_available":false,"preview_error":error.to_string()}),
        ),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/advanced_emoji_assets.rs"]
mod tests;
