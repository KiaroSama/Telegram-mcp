mod refresh;

use anyhow::{Context, Result};
use refresh::{download_with_single_refresh, maybe_download_document_thumb};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

fn signed_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn identifier(value: &Value) -> String {
    signed_id(value).unwrap_or(0).to_string()
}

fn bounded_component(name: &str, max_bytes: usize, max_units: usize) -> String {
    let (stem, extension) = name
        .rsplit_once('.')
        .filter(|(_, extension)| !extension.is_empty() && extension.len() <= 32)
        .map(|(stem, extension)| (stem, format!(".{extension}")))
        .unwrap_or((name, String::new()));
    let mut result = String::new();
    let mut units = 0;
    for ch in stem.chars() {
        if result.len() + ch.len_utf8() + extension.len() > max_bytes
            || units + ch.len_utf16() + extension.encode_utf16().count() > max_units
        {
            break;
        }
        result.push(ch);
        units += ch.len_utf16();
    }
    result.push_str(&extension);
    result
}

pub fn safe_file_name(source: &str) -> String {
    let name: String = source
        .chars()
        .filter(|ch| {
            !ch.is_control()
                && !crate::text_fidelity::is_unsafe_char(*ch)
                && !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect();
    let name = name.trim_matches(|ch| ch == '.' || ch == ' ').to_owned();
    if name.is_empty()
        || [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(
            &name
                .split('.')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase()
                .as_str(),
        )
    {
        "file".into()
    } else {
        bounded_component(&name, 255, 255)
    }
}

pub async fn download_service_photo(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
) -> Result<bool> {
    let kind = message["action"]["_"].as_str().unwrap_or_default();
    if !matches!(
        kind,
        "messageActionChatEditPhoto" | "messageActionSuggestProfilePhoto"
    ) || message["action"]["photo"]["_"].as_str() != Some("photo")
    {
        return Ok(false);
    }
    let mut carrier = json!({
        "id": message["id"],
        "out": true,
        "source_peer": message["source_peer"],
        "source_message_id": message["source_message_id"],
        "source_event_id": message["source_event_id"],
        "media": {"_": "messageMediaPhoto", "photo": message["action"]["photo"]},
    });
    let saved = download_media(api, &mut carrier, settings, directory).await?;
    if let Some(r) = carrier.get("export_refreshed").and_then(Value::as_u64) {
        let prev = message["export_refreshed"].as_u64().unwrap_or(0);
        message["export_refreshed"] = json!(prev + r);
    }
    if saved {
        if let Some(rel) = carrier.get("export_file").cloned() {
            message["export_service_photo"] = rel;
        }
        if let Some(icc) = carrier.get("export_icc").cloned() {
            message["export_icc"] = icc;
        }
    } else if carrier["export_file_failed"] == true {
        message["export_file_failed"] = json!(true);
        if let Some(skip) = carrier.get("export_file_skipped").cloned() {
            message["export_file_skipped"] = skip;
        }
    }
    Ok(saved)
}

pub async fn download_media(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
) -> Result<bool> {
    let Some(media) = message.get("media").cloned() else {
        return Ok(false);
    };
    if message["out"] != true && number(&media["ttl_seconds"]) > 0 {
        message["export_file_skipped"] = "incoming self-destructing media".into();
        return Ok(false);
    }
    if media["_"].as_str() == Some("messageMediaContact") {
        let Some(vcard) = media["vcard"].as_str().filter(|s| !s.trim().is_empty()) else {
            return Ok(false);
        };
        if !settings.media[6] && !settings.media[0] {
            message["export_file_skipped"] = "not selected".into();
            return Ok(false);
        }
        if (vcard.len() as u64) > settings.max_bytes {
            message["export_file_skipped"] = "file exceeds size_limit_mb".into();
            return Ok(false);
        }
        let idx =
            usize::try_from(signed_id(&message["id"]).unwrap_or(1).unsigned_abs()).unwrap_or(1);
        if let Some((rel, sz)) =
            super::files::write_contact_vcard(directory, idx, vcard, settings.max_bytes)?
        {
            message["contact_vcard"] = Value::String(rel);
            message["contact_vcard_file_size"] = json!(sz);
            return Ok(true);
        }
        return Ok(false);
    }

    let (object, kind, folder, mut filename) = if let Some(photo) = media
        .get("photo")
        .filter(|p| p["_"].as_str() == Some("photo"))
    {
        (
            photo.clone(),
            0_usize,
            "photos",
            format!("photo_{}.jpg", identifier(&photo["id"])),
        )
    } else if let Some(document) = media
        .get("document")
        .filter(|d| d["_"].as_str() == Some("document"))
    {
        let attrs = document["attributes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let has = |name: &str| attrs.iter().any(|a| a["_"].as_str() == Some(name));
        let audio = attrs
            .iter()
            .find(|a| a["_"].as_str() == Some("documentAttributeAudio"));
        let video = attrs
            .iter()
            .find(|a| a["_"].as_str() == Some("documentAttributeVideo"));
        let (kind, folder) = if message["custom_emoji_asset"] == true
            || has("documentAttributeSticker")
            || has("documentAttributeCustomEmoji")
        {
            (4, "stickers")
        } else if has("documentAttributeAnimated") {
            (5, "animations")
        } else if audio.is_some_and(|a| a["voice"] == true) {
            (2, "voice_messages")
        } else if video.is_some_and(|a| a["round_message"] == true) {
            (3, "round_video_messages")
        } else if video.is_some() {
            (1, "video_files")
        } else {
            (6, "files")
        };
        let filename = attrs
            .iter()
            .find(|a| a["_"].as_str() == Some("documentAttributeFilename"))
            .and_then(|a| a["file_name"].as_str())
            .map(safe_file_name)
            .unwrap_or_else(|| format!("file_{}", identifier(&document["id"])));
        (document.clone(), kind, folder, filename)
    } else {
        return Ok(false);
    };

    if !settings.media[kind] {
        message["export_file_skipped"] = "not selected".into();
        return Ok(false);
    }
    let size = if kind == 0 {
        object["sizes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                number(&s["size"]).max(
                    s["sizes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(number)
                        .max()
                        .unwrap_or(0),
                )
            })
            .max()
            .unwrap_or(0)
    } else {
        number(&object["size"])
    };
    if size > settings.max_bytes {
        message["export_file_skipped"] = "file exceeds size_limit_mb".into();
        return Ok(false);
    }

    let object_id = identifier(&object["id"]);
    let mut location = json!({
        "_": if kind == 0 { "inputPhotoFileLocation" } else { "inputDocumentFileLocation" },
        "id": object["id"],
        "access_hash": object["access_hash"],
        "file_reference": object["file_reference"],
        "thumb_size": "",
    });
    if let Some(dc) = object.get("dc_id") {
        location["_dc_id"] = dc.clone();
    }
    if kind == 0 {
        let thumb = object["sizes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["type"].as_str().is_some())
            .max_by_key(|s| number(&s["w"]) * number(&s["h"]))
            .and_then(|s| s["type"].as_str())
            .unwrap_or("w");
        location["thumb_size"] = thumb.into();
    }

    let prefix = format!("{}_{object_id}_", identifier(&message["id"]));
    filename = format!(
        "{prefix}{}",
        bounded_component(
            &filename,
            255_usize.saturating_sub(prefix.len()),
            255_usize.saturating_sub(prefix.encode_utf16().count())
        )
    );
    let state = crate::state::State::from_environment()?;
    let root = state.resolve_file(directory, crate::state::FileAccess::Write)?;
    let relative = super::files::prepare_relative_path(&root, &format!("{folder}/{filename}"))?;
    let destination = state.resolve_file(&root.join(&relative), crate::state::FileAccess::Write)?;
    std::fs::create_dir_all(
        destination
            .parent()
            .context("Export media path has no parent")?,
    )?;

    if download_with_single_refresh(
        api,
        location,
        &destination,
        settings.max_bytes,
        message,
        kind == 0,
        &object_id,
    )
    .await
    {
        message["export_file"] = relative.clone().into();
        if (kind == 0 || (kind == 4 && message["custom_emoji_asset"] != true))
            && let Ok(bytes) = std::fs::read(&destination)
        {
            let (thumb_info, icc) =
                super::thumbnail::write_media_thumbnail(directory, &relative, &bytes, kind == 4);
            if let Some(transform) = icc {
                message["export_icc"] = super::thumbnail::icc_summary_json(&transform);
            }
            if let Some(wt) = thumb_info {
                message["export_thumb"] = Value::String(wt.relative_path);
                message["export_thumb_width"] = json!(wt.width);
                message["export_thumb_height"] = json!(wt.height);
                message["export_thumb_size"] = json!(wt.byte_size);
            }
        } else if kind != 0 && message["custom_emoji_asset"] != true {
            maybe_download_document_thumb(api, &object, &relative, message, settings, directory)
                .await?;
        }
        Ok(true)
    } else {
        message["export_file_skipped"] =
            "download failed; source reference may have expired".into();
        message["export_file_failed"] = json!(true);
        Ok(false)
    }
}

pub async fn download_rich_media(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
) -> Result<usize> {
    let mut files = Map::new();
    let mut thumbs = Map::new();
    let mut count = 0;
    let mut refreshed_add = 0_u64;
    let mut failed_any = false;
    for (key, kind) in [("photos", "photo"), ("documents", "document")] {
        let objects = message["rich_message"][key]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for object in objects {
            let id = object["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| object["id"].to_string());
            if files.contains_key(&id) {
                continue;
            }
            let mut source = Map::new();
            source.insert(kind.to_owned(), object);
            let mut item = json!({
                "id": message["id"],
                "out": message["out"],
                "rich_asset": true,
                "source_peer": message["source_peer"],
                "source_message_id": message["source_message_id"],
                "media": source,
            });
            if download_media(api, &mut item, settings, directory).await? {
                files.insert(id.clone(), item["export_file"].clone());
                if let Some(t) = item.get("export_thumb").cloned() {
                    thumbs.insert(id, t);
                }
                count += 1;
            } else if item["export_file_failed"] == true {
                failed_any = true;
            }
            refreshed_add += item["export_refreshed"].as_u64().unwrap_or(0);
        }
    }
    if refreshed_add > 0 {
        let prev = message["export_refreshed"].as_u64().unwrap_or(0);
        message["export_refreshed"] = json!(prev + refreshed_add);
    }
    if failed_any {
        message["export_file_failed"] = json!(true);
    }
    if !files.is_empty() {
        message["rich_files"] = Value::Object(files);
    }
    if !thumbs.is_empty() {
        message["rich_thumbs"] = Value::Object(thumbs);
    }
    Ok(count)
}

pub async fn hydrate_custom_emoji(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
    cache: &mut BTreeMap<String, Option<String>>,
) -> Result<usize> {
    if !settings.media[4] {
        return Ok(0);
    }
    let ids: BTreeSet<String> = message["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entity| entity["_"] == "messageEntityCustomEmoji")
        .filter_map(|entity| entity.get("document_id"))
        .map(identifier)
        .filter(|id| id != "0")
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let missing: Vec<String> = ids
        .iter()
        .filter(|id| !cache.contains_key(*id))
        .cloned()
        .collect();
    let mut downloaded = 0;
    let mut refreshed_add = 0_u64;
    for chunk in missing.chunks(100) {
        for id in chunk {
            cache.insert(id.clone(), None);
        }
        let Ok(response) = api
            .invoke(
                "messages.getCustomEmojiDocuments",
                json!({"document_id": chunk}),
            )
            .await
        else {
            continue;
        };
        for document in response.as_array().into_iter().flatten() {
            let id = identifier(&document["id"]);
            if !chunk.contains(&id) {
                continue;
            }
            let mut item = json!({
                "id": message["id"],
                "custom_emoji_asset": true,
                "media": {"document": document},
            });
            if download_media(api, &mut item, settings, directory).await? {
                cache.insert(
                    id,
                    Some(item["export_file"].as_str().unwrap_or_default().to_owned()),
                );
                downloaded += 1;
            }
            refreshed_add += item["export_refreshed"].as_u64().unwrap_or(0);
        }
    }
    if refreshed_add > 0 {
        let prev = message["export_refreshed"].as_u64().unwrap_or(0);
        message["export_refreshed"] = json!(prev + refreshed_add);
    }
    let mut files = Map::new();
    let mut unavailable = Vec::new();
    for id in ids {
        match cache.get(&id).and_then(|path| path.as_ref()) {
            Some(path) => {
                files.insert(id, Value::String(path.clone()));
            }
            None => unavailable.push(id),
        }
    }
    if !files.is_empty() {
        message["custom_emoji_files"] = Value::Object(files);
    }
    if !unavailable.is_empty() {
        message["custom_emoji_unavailable"] = json!(unavailable);
    }
    Ok(downloaded)
}
