use super::super::helpers::*;
use crate::{
    args,
    backend::Telegram,
    state::{FileAccess, State},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "download_media" | "save_disappearing_media" | "download_rich_media" => Some(
            async {
                let state = State::global()?;
                run_with_state(api, &state, name, a).await
            }
            .await,
        ),
        _ => None,
    }
}
pub fn download_location(media: &Value) -> Result<Value> {
    let raw = media
        .get("document")
        .or_else(|| media.get("photo"))
        .unwrap_or(media);
    let photo = raw["_"].as_str() == Some("photo") || media.get("photo").is_some();
    let sizes = raw
        .get("sizes")
        .or_else(|| raw.get("thumbs"))
        .and_then(Value::as_array);
    let thumb = if photo {
        sizes
            .and_then(|list| list.iter().rev().find_map(|size| size["type"].as_str()))
            .unwrap_or("w")
    } else {
        ""
    };
    ensure!(
        raw.get("id").is_some()
            && raw.get("access_hash").is_some()
            && raw.get("file_reference").is_some(),
        "This media has no downloadable document/photo reference"
    );
    let mut location = json!({"_":if photo{"inputPhotoFileLocation"}else{"inputDocumentFileLocation"},"id":raw["id"],"access_hash":raw["access_hash"],"file_reference":raw["file_reference"],"thumb_size":thumb});
    if let Some(dc) = raw.get("dc_id") {
        location["_dc_id"] = dc.clone();
    }
    Ok(location)
}
pub async fn download_bytes(
    api: &dyn Telegram,
    location: Value,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    ensure!(
        (1..=512 * 1024 * 1024).contains(&max_bytes),
        "max_bytes must be between1and512MiB"
    );
    let data = api.download_memory(location, max_bytes).await?;
    ensure!(
        data.len() as u64 <= max_bytes,
        "Downloaded media exceeded max_bytes"
    );
    Ok(data)
}

pub fn declared_size(media: &Value) -> Option<u64> {
    let raw = media
        .get("document")
        .or_else(|| media.get("photo"))
        .unwrap_or(media);
    raw["size"]
        .as_u64()
        .or_else(|| raw["size"].as_str().and_then(|s| s.parse().ok()))
        .or_else(|| {
            raw["sizes"].as_array().and_then(|list| {
                list.iter()
                    .filter_map(|item| {
                        item["size"].as_u64().or_else(|| {
                            item["sizes"]
                                .as_array()
                                .and_then(|sizes| sizes.last())
                                .and_then(Value::as_u64)
                        })
                    })
                    .max()
            })
        })
}
fn filename(media: &Value, mid: i64) -> String {
    let document = media.get("document").unwrap_or(media);
    let supplied = document["attributes"]
        .as_array()
        .and_then(|list| list.iter().find_map(|item| item["file_name"].as_str()));
    let fallback = format!(
        "telegram-{mid}-{}{}",
        args::random_id().as_str().unwrap_or("download"),
        if media.get("photo").is_some() {
            ".jpg"
        } else {
            ".bin"
        }
    );
    let supplied = supplied.unwrap_or(&fallback);
    let clean: String = supplied
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .take(180)
        .collect();
    let clean = clean.trim_matches('.').trim();
    if clean.is_empty() {
        fallback
    } else {
        clean.to_owned()
    }
}
pub fn destination(raw: Option<&str>, media: &Value, mid: i64) -> Result<PathBuf> {
    let state = State::global()?;
    destination_in(&state, raw, media, mid)
}
pub fn destination_in(
    state: &State,
    raw: Option<&str>,
    media: &Value,
    mid: i64,
) -> Result<PathBuf> {
    let path = if let Some(raw) = raw.filter(|s| !s.is_empty()) {
        let path = state.resolve_file(Path::new(raw), FileAccess::Write)?;
        if path.is_dir() {
            path.join(filename(media, mid))
        } else {
            path
        }
    } else {
        let root = state.default_downloads()?;
        std::fs::create_dir_all(&root)?;
        root.join(filename(media, mid))
    };
    let path = state.resolve_file(&path, FileAccess::Write)?;
    ensure!(
        !path.exists(),
        "The download destination already exists; choose another path"
    );
    Ok(path)
}
pub async fn run_with_state(
    api: &dyn Telegram,
    state: &State,
    name: &str,
    a: &Value,
) -> Result<Value> {
    let cap = args::integer(a, "max_bytes", 50 * 1024 * 1024)?;
    ensure!(
        cap > 0 && cap <= 512 * 1024 * 1024,
        "max_bytes must be between 1 and 512 MiB"
    );
    let p = args::peer(api, a, "chat_id").await?;
    let mid = if a.get("story_id").is_some_and(|v| !v.is_null()) {
        id(a, "story_id")?
    } else {
        id(a, "message_id")?
    };
    let media = if a.get("story_id").is_some_and(|v| !v.is_null()) {
        let response = api
            .invoke("stories.getStoriesByID", json!({"peer":p,"id":[mid]}))
            .await?;
        response["stories"]
            .as_array()
            .and_then(|stories| {
                stories
                    .iter()
                    .find(|story| story["id"].as_i64() == Some(mid))
            })
            .and_then(|story| story.get("media"))
            .cloned()
            .context("Story was not found or has no media")?
    } else if name == "download_rich_media" {
        let result = api
            .invoke("messages.getRichMessage", json!({"peer":p,"id":mid}))
            .await?;
        let messages = result_messages(&result);
        let rich = messages
            .first()
            .and_then(|message| message.get("rich_message"))
            .context("This message has no rich body")?;
        let blocks = rich["blocks"]
            .as_array()
            .context("The rich message contains no blocks")?;
        let selected = if a.get("block_index").is_some_and(|v| !v.is_null()) {
            let index = args::integer(a, "block_index", -1)?;
            ensure!(index >= 0, "block_index must be nonnegative");
            blocks
                .get(index as usize)
                .context("block_index is outside the rich body")?
        } else {
            ensure!(
                blocks.len() == 1,
                "Specify block_index to select one rich media block"
            );
            &blocks[0]
        };
        let photo = selected.get("photo_id");
        let doc = selected
            .get("document_id")
            .or_else(|| selected.get("video_id"))
            .or_else(|| selected.get("audio_id"));
        let (wanted, list) = if let Some(photo) = photo {
            (photo, &rich["photos"])
        } else {
            (
                doc.context("The selected rich block has no downloadable media")?,
                &rich["documents"],
            )
        };
        list.as_array()
            .and_then(|list| list.iter().find(|item| item["id"] == *wanted))
            .cloned()
            .context("The rich media reference was not returned by Telegram")?
    } else {
        let message = fetch(api, &p, mid).await?;
        let media = message
            .get("media")
            .cloned()
            .context("This message contains no media")?;
        if name == "save_disappearing_media" {
            ensure!(
                media.get("ttl_seconds").is_some(),
                "This message has no disappearing media"
            );
        }
        media
    };
    if let Some(size) = declared_size(&media) {
        ensure!(
            size <= cap as u64,
            "The media's declared size exceeds max_bytes"
        );
    }
    let path = destination_in(
        state,
        a.get("file_path")
            .filter(|v| !v.is_null())
            .and_then(Value::as_str),
        &media,
        mid,
    )?;
    let result = api
        .download(download_location(&media)?, &path, cap as u64)
        .await?;
    if name == "save_disappearing_media" && args::boolean(a, "preview", true)? {
        let preview = async {
            super::previews::render(
                &std::fs::read(&path)?,
                args::limit(a, "count", 4, 10)?,
                args::integer(a, "max_dimension", 1568)?,
            )
            .await
        }
        .await;
        return Ok(match preview {
            Ok(preview) => {
                json!({"path":path,"message_id":mid,"download":result,"content":preview["content"],"metadata":preview["metadata"]})
            }
            Err(_) => {
                json!({"path":path,"message_id":mid,"download":result,"preview_error":"The file was saved, but its preview could not be rendered"})
            }
        });
    }
    Ok(json!({"path":path,"message_id":mid,"download":result}))
}
