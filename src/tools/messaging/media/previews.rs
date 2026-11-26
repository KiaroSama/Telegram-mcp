use super::super::helpers::*;
use super::downloads;
use crate::{args, backend::Telegram, state::State};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "get_media_thumbnail" | "get_media_frames" | "get_custom_emoji" => {
            Some(run(api, name, a).await)
        }
        _ => None,
    }
}
pub fn image_content(data: &[u8], max_dimension: i64, mut metadata: Value) -> Result<Value> {
    let image = crate::tools::visual::images::decode_image(data)?;
    let (bytes, meta) =
        crate::tools::visual::images::encode_image(&image, "png", max_dimension, 90, false)?;
    metadata["image"] = meta;
    Ok(
        json!({"content":[{"type":"image","data":STANDARD.encode(bytes),"mimeType":"image/png"}],"metadata":metadata}),
    )
}
async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let cap = args::integer(
        a,
        "max_bytes",
        if name == "get_media_frames" {
            50 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        },
    )?;
    ensure!(
        cap > 0 && cap <= 50 * 1024 * 1024,
        "max_bytes must be between 1 and 50 MiB"
    );
    let dimension = args::integer(
        a,
        "max_dimension",
        if name == "get_media_frames" {
            900
        } else {
            1568
        },
    )?;
    let count = args::limit(
        a,
        "count",
        if name == "get_custom_emoji" { 1 } else { 4 },
        12,
    )?;
    let media = if name == "get_custom_emoji" {
        let ids = args::required(a, "document_ids")?;
        let ids = ids.as_array().cloned().unwrap_or(vec![ids.clone()]);
        ensure!(
            !ids.is_empty() && ids.len() <= 24,
            "document_ids must contain 1 to 24 ids"
        );
        let mut exact = vec![];
        for id in ids {
            let s = id
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| id.to_string());
            ensure!(
                s.parse::<i64>().is_ok_and(|n| n > 0),
                "document_ids must be positive 64-bit decimal ids"
            );
            exact.push(s);
        }
        let docs = api
            .invoke(
                "messages.getCustomEmojiDocuments",
                json!({"document_id":exact}),
            )
            .await?;
        let docs = docs
            .as_array()
            .context("Telegram returned no custom emoji")?;
        ensure!(
            !docs.is_empty(),
            "Telegram returned no document for these emoji ids"
        );
        let mut content = vec![];
        let mut rows = vec![];
        let mut image_bytes = 0_usize;
        let mut sets = std::collections::BTreeMap::<String, Value>::new();
        for doc in docs.iter().take(24) {
            let set = doc["attributes"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|a| {
                    a.get("stickerset").filter(|s| {
                        s["_"] == "inputStickerSetID" || s["_"] == "inputStickerSetShortName"
                    })
                });
            let mut pack = Value::Null;
            if let Some(set) = set {
                let key = set.to_string();
                pack = match sets.entry(key) {
                    std::collections::btree_map::Entry::Occupied(e) => e.get().clone(),
                    std::collections::btree_map::Entry::Vacant(e) => {
                        let value = api
                            .invoke("messages.getStickerSet", json!({"stickerset":set,"hash":0}))
                            .await
                            .ok()
                            .map(|r| r["set"].clone())
                            .unwrap_or_default();
                        e.insert(value).clone()
                    }
                };
            }
            let preview_result: Result<_> = async {
                ensure!(
                    downloads::declared_size(doc).is_none_or(|size| size <= cap as u64),
                    "Custom emoji exceeds max_bytes"
                );
                let data =
                    downloads::download_bytes(api, downloads::download_location(doc)?, cap as u64)
                        .await?;
                super::emoji_previews::individual(
                    &data,
                    count,
                    dimension,
                    doc["mime_type"].as_str().unwrap_or(""),
                )
                .await
            }
            .await;
            let (images, preview) = match preview_result {
                Ok(value) => value,
                Err(_) => {
                    rows.push(json!({"document_id":doc["id"],"set_short_name":pack["short_name"],"set_title":pack["title"],"error":"This custom emoji could not be downloaded or rendered within its byte and codec limits"}));
                    continue;
                }
            };
            let bytes = images
                .iter()
                .try_fold(0_usize, |total, image| {
                    total.checked_add(image["data"].as_str().map(str::len).unwrap_or(0))
                })
                .context("Custom emoji preview byte count overflow")?;
            if image_bytes
                .checked_add(bytes)
                .is_none_or(|total| total > 32 * 1024 * 1024)
            {
                rows.push(json!({"document_id":doc["id"],"set_short_name":pack["short_name"],"set_title":pack["title"],"error":"This custom emoji would exceed the request preview byte budget"}));
                continue;
            }
            image_bytes += bytes;
            content.extend(images);
            let attributes = doc["attributes"].as_array().cloned().unwrap_or_default();
            let placeholder = attributes
                .iter()
                .find(|attribute| attribute["_"] == "documentAttributeCustomEmoji")
                .and_then(|attribute| attribute["alt"].as_str());
            rows.push(json!({"document_id":doc["id"],"set_short_name":pack["short_name"],"set_title":pack["title"],"mime_type":doc["mime_type"],"attributes":attributes,"placeholder":placeholder,"preview":preview}));
        }
        return Ok(json!({"content":content,"metadata":{"emoji":rows}}));
    } else {
        let mid = id(a, "message_id")?;
        let p = args::peer(api, a, "chat_id").await?;
        let message = fetch(api, &p, mid).await?;
        message
            .get("media")
            .cloned()
            .context("This message has no media")?
    };
    let mut location = downloads::download_location(&media)?;
    if name == "get_media_frames" && args::boolean(a, "premium_effect", false)? {
        let effect = media["document"]["video_thumbs"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["type"].as_str() == Some("f")))
            .context("This sticker has no Premium effect animation")?;
        location["thumb_size"] = effect["type"].clone();
    }
    if name == "get_media_thumbnail" {
        let raw = media
            .get("document")
            .or_else(|| media.get("photo"))
            .unwrap_or(&media);
        let sizes = raw
            .get("thumbs")
            .or_else(|| raw.get("sizes"))
            .and_then(Value::as_array)
            .context("This media has no thumbnail")?;
        ensure!(!sizes.is_empty(), "This media has no thumbnail");
        let index = args::integer(a, "thumb_index", -1)?;
        let index = if index < 0 {
            sizes.len() as i64 + index
        } else {
            index
        };
        ensure!(
            index >= 0 && (index as usize) < sizes.len(),
            "thumb_index is outside the available thumbnails"
        );
        let selected = &sizes[index as usize];
        if selected.get("bytes").is_some() {
            return image_content(
                &inline_thumbnail(selected)?,
                dimension,
                json!({"thumbnail":selected}),
            );
        }
        location["thumb_size"] = selected
            .get("type")
            .cloned()
            .context("Thumbnail type is missing")?;
    }
    let data = downloads::download_bytes(api, location, cap as u64).await?;
    if name == "get_media_thumbnail" {
        image_content(&data, dimension, json!({"kind":"thumbnail"}))
    } else {
        render(&data, count, dimension).await
    }
}
pub async fn render(data: &[u8], count: i64, dimension: i64) -> Result<Value> {
    let animation_suffix = if data.starts_with(b"\x1f\x8b") {
        Some("tgs")
    } else if data.starts_with(b"GIF8") {
        Some("gif")
    } else if data.starts_with(b"RIFF") && data.windows(4).any(|part| part == b"ANIM") {
        Some("webp")
    } else if data.starts_with(b"\x89PNG") && data.windows(4).any(|part| part == b"acTL") {
        Some("png")
    } else {
        None
    };
    if let Some(suffix) = animation_suffix {
        let state = State::global()?;
        let root = state
            .file_roots()
            .first()
            .context("No file roots configured")?;
        let temp = tempfile::Builder::new()
            .prefix(".telegram-animation-")
            .tempdir_in(root)?;
        let path = temp.path().join(format!("animation.{suffix}"));
        std::fs::write(&path, data)?;
        match crate::tools::visual::frames::extract_frames(&path, count, dimension).await {
            Ok(frames) => {
                let images: Vec<(Vec<u8>, String)> = frames
                    .iter()
                    .map(|(data, meta)| (data.clone(), format!("frame {}", meta["frame_index"])))
                    .collect();
                let (bytes, meta) =
                    crate::tools::visual::images::compose_contact_sheet(&images, None)?;
                return image_content(
                    &bytes,
                    dimension,
                    json!({"static":false,"frames":frames.len(),"frame_metadata":frames.iter().map(|(_,meta)|meta.clone()).collect::<Vec<_>>(),"sheet":meta}),
                );
            }
            Err(error) if error.to_string().contains("not animated") => {}
            Err(error) => return Err(error),
        }
    }
    if let Ok(image) = crate::tools::visual::images::decode_image(data) {
        let (bytes, meta) =
            crate::tools::visual::images::encode_image(&image, "png", dimension, 90, false)?;
        return Ok(
            json!({"content":[{"type":"image","data":STANDARD.encode(bytes),"mimeType":"image/png"}],"metadata":{"static":true,"image":meta}}),
        );
    }
    // Share the duration-aware, bounded decoder with animated emoji previews.
    let frames = super::emoji_previews::video(data, count, dimension, "").await?;
    let images = frames
        .iter()
        .map(|(bytes, meta)| (bytes.clone(), format!("{}s", meta["timestamp"])))
        .collect::<Vec<_>>();
    let (bytes, meta) = crate::tools::visual::images::compose_contact_sheet(&images, None)?;
    image_content(
        &bytes,
        dimension,
        json!({"static":false,"frames":frames.len(),"frame_metadata":frames.iter().map(|(_,meta)|meta.clone()).collect::<Vec<_>>(),"sheet":meta}),
    )
}

/// Reuse grammers' official stripped-thumbnail reconstruction rather than
/// interpreting compressed JPEG payloads as complete image files.
pub fn inline_thumbnail(selected: &Value) -> Result<Vec<u8>> {
    use grammers_client::media::Downloadable;
    use grammers_tl_types::Deserializable;
    let photo = json!({"_":"photo","id":"0","access_hash":"0","file_reference":[],"date":0,"sizes":[selected],"dc_id":2});
    let bytes = crate::tl::Schema::global().encode_object(&photo)?;
    let raw = grammers_tl_types::enums::Photo::from_bytes(&bytes)?;
    let photo = grammers_client::media::Photo::from_raw(raw);
    let data = photo
        .thumbs()
        .first()
        .and_then(Downloadable::to_data)
        .context("This thumbnail has no inline image data")?;
    ensure!(!data.is_empty(), "The inline thumbnail payload is invalid");
    Ok(data)
}
