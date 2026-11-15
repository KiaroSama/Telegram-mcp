use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

fn identifier(value: &Value) -> String {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or(0)
        .to_string()
}

pub fn safe_file_name(source: &str) -> String {
    let name: String = source
        .chars()
        .filter(|ch| {
            !ch.is_control() && !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(140)
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
        name
    }
}

pub async fn download_media(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
) -> Result<bool> {
    let Some(media) = message.get("media") else {
        return Ok(false);
    };
    if message["out"] != true && number(&media["ttl_seconds"]) > 0 {
        message["export_file_skipped"] = "incoming self-destructing media".into();
        return Ok(false);
    }
    let (object, kind, folder, mut filename) = if let Some(photo) = media
        .get("photo")
        .filter(|p| p["_"].as_str() == Some("photo"))
    {
        (
            photo,
            0,
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
        (document, kind, folder, filename)
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
    let mut location = json!({"_":if kind==0 {"inputPhotoFileLocation"}else{"inputDocumentFileLocation"},
        "id":object["id"],"access_hash":object["access_hash"],"file_reference":object["file_reference"],"thumb_size":""});
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
    // Prefixing by the source id makes same-name attachments unambiguous and
    // does not reuse an existing file left by an earlier partial export.
    filename = format!(
        "{}_{}_{}",
        identifier(&message["id"]),
        identifier(&object["id"]),
        filename
    );
    let relative = format!("{folder}/{filename}");
    let state = crate::state::State::from_environment()?;
    let destination =
        state.resolve_file(&directory.join(&relative), crate::state::FileAccess::Write)?;
    std::fs::create_dir_all(
        destination
            .parent()
            .context("Export media path has no parent")?,
    )?;
    match api
        .download(location, &destination, settings.max_bytes)
        .await
    {
        Ok(_) => {
            message["export_file"] = relative.into();
            Ok(true)
        }
        Err(_) => {
            message["export_file_skipped"] =
                "download failed; source reference may have expired".into();
            Ok(false)
        }
    }
}

pub async fn download_rich_media(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
) -> Result<usize> {
    let mut files = serde_json::Map::new();
    let mut count = 0;
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
            let mut source = serde_json::Map::new();
            source.insert(kind.to_owned(), object);
            let mut item = json!({"id":message["id"],"out":message["out"],"media":source});
            if download_media(api, &mut item, settings, directory).await? {
                files.insert(id, item["export_file"].clone());
                count += 1;
            }
        }
    }
    if !files.is_empty() {
        message["rich_files"] = Value::Object(files);
    }
    Ok(count)
}

pub async fn hydrate_custom_emoji(
    api: &dyn crate::backend::Telegram,
    message: &mut Value,
    settings: &super::settings::Settings,
    directory: &Path,
    cache: &mut std::collections::BTreeMap<String, Option<String>>,
) -> Result<usize> {
    if !settings.media[4] {
        return Ok(0);
    }
    let ids: std::collections::BTreeSet<String> = message["entities"]
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
    for chunk in missing.chunks(100) {
        for id in chunk {
            cache.insert(id.clone(), None);
        }
        let Ok(response) = api
            .invoke(
                "messages.getCustomEmojiDocuments",
                json!({"document_id":chunk}),
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
            let mut item =
                json!({"id":message["id"],"custom_emoji_asset":true,"media":{"document":document}});
            if download_media(api, &mut item, settings, directory).await? {
                cache.insert(
                    id,
                    Some(item["export_file"].as_str().unwrap_or_default().to_owned()),
                );
                downloaded += 1;
            }
        }
    }
    let mut files = serde_json::Map::new();
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
