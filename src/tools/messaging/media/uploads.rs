use super::super::helpers::*;
use crate::{
    args,
    backend::Telegram,
    state::{FileAccess, State},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::io::AsyncReadExt;

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "send_file"
        | "send_album"
        | "send_voice"
        | "send_sticker"
        | "send_disappearing_media"
        | "upload_file"
        | "edit_message_media" => Some(
            async {
                let state = State::global()?;
                run_with_state(api, &state, name, a).await
            }
            .await,
        ),
        _ => None,
    }
}
fn suffix(path: &Path) -> String {
    path.extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
}
pub fn infer_kind(path: &Path) -> String {
    let ext = suffix(path);
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "heic" | "heif" => "photo",
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => "video",
        "mp3" | "m4a" | "flac" | "wav" | "aac" => "audio",
        "ogg" | "oga" | "opus" => {
            // Ogg music carries title/artist tags. A recording without those defaults to voice.
            let data = std::fs::File::open(path).ok().map(|file| {
                use std::io::Read;
                let mut data = vec![];
                let _ = file.take(65536).read_to_end(&mut data);
                data
            });
            if data.is_some_and(|data| {
                data.windows(6)
                    .any(|part| part.eq_ignore_ascii_case(b"TITLE="))
                    || data
                        .windows(7)
                        .any(|part| part.eq_ignore_ascii_case(b"ARTIST="))
            }) {
                "audio"
            } else {
                "voice_note"
            }
        }
        "gif" => "animation",
        "tgs" => "sticker",
        _ => "document",
    }
    .to_owned()
}
pub fn validate_kind(path: &Path, kind: &str, caption: &str) -> Result<()> {
    ensure!(
        matches!(
            kind,
            "photo"
                | "video"
                | "document"
                | "audio"
                | "animation"
                | "sticker"
                | "video_note"
                | "voice_note"
        ),
        "Unsupported media kind"
    );
    ensure!(
        caption.is_empty() || !matches!(kind, "sticker" | "video_note"),
        "Stickers and video notes cannot carry captions"
    );
    let ext = suffix(path);
    let compatible = match kind {
        "photo" => matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "bmp" | "heic" | "heif"
        ),
        "voice_note" => matches!(ext.as_str(), "ogg" | "oga" | "opus"),
        "audio" => matches!(
            ext.as_str(),
            "mp3" | "m4a" | "flac" | "wav" | "aac" | "ogg" | "oga" | "opus"
        ),
        "video" | "video_note" | "animation" => matches!(
            ext.as_str(),
            "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "gif"
        ),
        "sticker" => matches!(
            ext.as_str(),
            "jpg" | "jpeg" | "png" | "webp" | "tgs" | "webm"
        ),
        _ => true,
    };
    ensure!(
        compatible,
        "The selected media kind is incompatible with the file type"
    );
    Ok(())
}
fn mime(path: &Path) -> &'static str {
    match suffix(path).as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "tgs" => "application/x-tgsticker",
        _ => "application/octet-stream",
    }
}
async fn probe(path: &Path) -> Result<Value> {
    tracing::debug!(
        operation = "media_probe",
        "Starting bounded media metadata probe"
    );
    let mut child = super::child("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,width,height,duration:stream_tags=title,artist:format_tags=title,artist",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("ffprobe is required for audio/video metadata; install FFmpeg")?;
    let mut stdout = child
        .stdout
        .take()
        .context("ffprobe output pipe unavailable")?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut data = vec![];
        (&mut stdout).take(65537).read_to_end(&mut data).await?;
        ensure!(
            data.len() <= 65536,
            "Media metadata exceeds the 64 KiB bound"
        );
        ensure!(
            child.wait().await?.success(),
            "ffprobe could not read this media file"
        );
        Ok::<_, anyhow::Error>(serde_json::from_slice::<Value>(&data)?)
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            bail!("Media metadata probe exceeded 10 seconds")
        }
    }
}
pub async fn uploaded(
    api: &dyn Telegram,
    path: &Path,
    kind: &str,
    ttl: Option<i64>,
) -> Result<Value> {
    let file = api.upload(path).await?;
    if kind == "photo" {
        let mut media = json!({"_":"inputMediaUploadedPhoto","file":file});
        if let Some(ttl) = ttl {
            media["ttl_seconds"] = json!(ttl);
        }
        return Ok(media);
    }
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    let mut attributes = vec![json!({"_":"documentAttributeFilename","file_name":filename})];
    if matches!(
        kind,
        "video" | "video_note" | "animation" | "audio" | "voice_note"
    ) {
        let meta = probe(path).await?;
        let streams = meta["streams"].as_array().context("Media has no streams")?;
        if matches!(kind, "audio" | "voice_note") {
            let stream = streams
                .iter()
                .find(|s| s["codec_type"].as_str() == Some("audio"))
                .context("This file has no audio stream")?;
            let duration = stream["duration"]
                .as_str()
                .or(meta["format"]["duration"].as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .context("Audio duration is missing")?;
            ensure!(
                duration.is_finite() && duration >= 0.0 && duration <= i32::MAX as f64,
                "Audio duration is invalid"
            );
            let mut attr = json!({"_":"documentAttributeAudio","duration":duration.ceil() as i64,"voice":kind=="voice_note"});
            if kind == "audio" {
                // Ogg/Vorbis/Opus metadata belongs to the audio stream, while
                // containers such as MP3/M4A may report it at format level.
                copy_optional(
                    &mut attr,
                    &stream["tags"],
                    &[("title", "title"), ("artist", "performer")],
                );
                copy_optional(
                    &mut attr,
                    &meta["format"]["tags"],
                    &[("title", "title"), ("artist", "performer")],
                );
            }
            attributes.push(attr);
        } else {
            let stream = streams
                .iter()
                .find(|s| s["codec_type"].as_str() == Some("video"))
                .context("This file has no video stream")?;
            let duration = stream["duration"]
                .as_str()
                .or(meta["format"]["duration"].as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .context("Video duration is missing")?;
            let width = stream["width"].as_i64().context("Video width is missing")?;
            let height = stream["height"]
                .as_i64()
                .context("Video height is missing")?;
            ensure!(
                duration.is_finite() && duration >= 0.0 && width > 0 && height > 0,
                "Video metadata is invalid"
            );
            attributes.push(json!({"_":"documentAttributeVideo","duration":duration,"w":width,"h":height,"supports_streaming":kind!="video_note","round_message":kind=="video_note"}));
            if kind == "animation" {
                attributes.push(json!({"_":"documentAttributeAnimated"}));
            }
        }
    }
    if kind == "sticker" {
        attributes.push(json!({"_":"documentAttributeSticker","alt":"","stickerset":{"_":"inputStickerSetEmpty"}}));
    }
    let mut media = json!({"_":"inputMediaUploadedDocument","file":file,"mime_type":mime(path),"attributes":attributes,"force_file":kind=="document"});
    if let Some(ttl) = ttl {
        media["ttl_seconds"] = json!(ttl);
    }
    Ok(media)
}
/// The runtime owns the filesystem boundary independently of the Telegram
/// connection; callers can share that boundary without process environment.
pub async fn run_with_state(
    api: &dyn Telegram,
    state: &State,
    name: &str,
    a: &Value,
) -> Result<Value> {
    if name == "upload_file" {
        let path = state.resolve_file(Path::new(args::text(a, "file_path")?), FileAccess::Read)?;
        return api.upload(&path).await;
    }
    let list = if name == "send_album" {
        args::required(a, "file_paths")?
    } else {
        args::required(a, "file_path")?
    };
    let values = list
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![list.clone()]);
    ensure!(
        !values.is_empty() && values.len() <= 10,
        "An album must contain 1 to 10 files"
    );
    ensure!(
        matches!(name, "send_file" | "send_album") || values.len() == 1,
        "This operation accepts exactly one file"
    );
    let album = name == "send_album";
    ensure!(
        !(name == "send_disappearing_media"
            && args::boolean(a, "as_voice", false)?
            && args::boolean(a, "as_video_note", false)?),
        "A disappearing file cannot be both a voice note and a video note"
    );
    let raw_caption = a["caption"].as_str().unwrap_or("");
    let (caption, formatting) =
        if let Some(raw) = a.get("caption_entities").filter(|v| !v.is_null()) {
            ensure!(
                a.get("parse_mode").is_none_or(|v| v.is_null()),
                "caption_entities and parse_mode cannot be combined"
            );
            (
                raw_caption.to_owned(),
                build_entities(api, raw_caption, raw).await?,
            )
        } else {
            crate::backend::parse_text(raw_caption, a["parse_mode"].as_str())?
        };
    ensure!(
        caption.encode_utf16().count() <= 1024,
        "Caption exceeds 1024 UTF-16 units"
    );
    let mut paths: Vec<(PathBuf, String)> = vec![];
    for (index, raw) in values.iter().enumerate() {
        let path = state.resolve_file(
            Path::new(raw.as_str().context("file_path must be a path string")?),
            FileAccess::Read,
        )?;
        let explicit = a
            .get("kind")
            .filter(|v| !v.is_null())
            .and_then(|value| {
                value
                    .as_array()
                    .map(|list| list.get(index))
                    .unwrap_or(Some(value))
            })
            .and_then(Value::as_str);
        let kind = match name {
            "send_voice" => "voice_note".to_owned(),
            "send_sticker" => "sticker".to_owned(),
            "send_disappearing_media" if args::boolean(a, "as_voice", false)? => {
                "voice_note".to_owned()
            }
            "send_disappearing_media" if args::boolean(a, "as_video_note", false)? => {
                "video_note".to_owned()
            }
            _ => explicit
                .map(str::to_owned)
                .unwrap_or_else(|| infer_kind(&path)),
        };
        validate_kind(&path, &kind, if index == 0 { &caption } else { "" })?;
        if album {
            ensure!(
                !matches!(
                    kind.as_str(),
                    "voice_note" | "video_note" | "sticker" | "animation"
                ),
                "This media kind cannot be sent in an album"
            );
        }
        paths.push((path, kind));
    }
    if let Some(kinds) = a["kind"].as_array() {
        ensure!(
            kinds.len() == paths.len(),
            "The kind array must match the file array length"
        );
    }
    if album {
        let group = |kind: &str| {
            if matches!(kind, "photo" | "video") {
                0
            } else if kind == "audio" {
                1
            } else {
                2
            }
        };
        ensure!(
            paths
                .iter()
                .all(|(_, kind)| group(kind) == group(&paths[0].1)),
            "An album cannot mix audio or documents with photos and videos"
        );
    }
    let ttl = if name == "send_disappearing_media" {
        let seconds = args::integer(a, "seconds", 0)?;
        ensure!(
            seconds == 0 || (1..=60).contains(&seconds),
            "seconds must be 0 (view once) or 1 to 60"
        );
        Some(if seconds == 0 { 2147483647 } else { seconds })
    } else {
        None
    };
    if ttl.is_some() {
        ensure!(
            paths.iter().all(|(_, kind)| matches!(
                kind.as_str(),
                "photo" | "video" | "voice_note" | "video_note"
            )),
            "Disappearing media must be a photo, video, voice note or video note"
        );
    }
    let p = args::peer(api, a, "chat_id").await?;
    let kinds: Vec<_> = paths.iter().map(|(_, kind)| kind.clone()).collect();
    let mut media = vec![];
    for (path, kind) in paths {
        media.push(uploaded(api, &path, &kind, ttl).await?);
    }
    if name == "edit_message_media" {
        let mid = id(a, "message_id")?;
        let mut req = json!({"peer":p,"id":mid,"media":media.remove(0)});
        if a.get("caption").is_none_or(|v| v.is_null()) {
            let current = fetch(api, &p, mid).await?;
            req["message"] = current["message"].clone();
            req["entities"] = json!(
                build_entities(
                    api,
                    current["message"].as_str().unwrap_or(""),
                    current.get("entities").unwrap_or(&json!([]))
                )
                .await?
            );
        } else {
            req["message"] = json!(caption);
            req["entities"] = json!(formatting);
        }
        return api.invoke("messages.editMessage", req).await;
    }
    let mut req = json!({"peer":p});
    if let Some(reply) = reply(a, None)? {
        req["reply_to"] = reply;
    }
    if let Some(send_as) = a.get("send_as").filter(|v| !v.is_null()) {
        req["send_as"] = api.resolve_peer(send_as).await?;
    }
    let group = |kind: &str| match kind {
        "photo" | "video" => Some(0),
        "audio" => Some(1),
        "document" => Some(2),
        _ => None,
    };
    let mut completed = vec![];
    let mut index = 0;
    while index < media.len() {
        let start = index;
        index += 1;
        if let Some(kind) = group(&kinds[start]) {
            while index < media.len() && group(&kinds[index]) == Some(kind) {
                index += 1;
            }
        }
        let mut request = req.clone();
        let outcome: Result<Value> = async {
            if album || index - start > 1 {
                let mut singles = vec![];
                for (offset, item) in media[start..index].iter().enumerate() {
                    let uploaded = api.invoke("messages.uploadMedia", json!({"peer":p,"media":item})).await?;
                    let item = if let Some(photo) = uploaded.get("photo") {
                        json!({"_":"inputMediaPhoto","id":{"_":"inputPhoto","id":photo["id"],"access_hash":photo["access_hash"],"file_reference":photo["file_reference"]}})
                    } else {
                        let document = &uploaded["document"];
                        ensure!(!document.is_null(), "Telegram omitted uploaded media references");
                        json!({"_":"inputMediaDocument","id":{"_":"inputDocument","id":document["id"],"access_hash":document["access_hash"],"file_reference":document["file_reference"]}})
                    };
                    singles.push(json!({"_":"inputSingleMedia","media":item,"random_id":args::random_id(),"message":if start+offset==0{caption.as_str()}else{""},"entities":if start+offset==0{formatting.clone()}else{vec![]}}));
                }
                request["multi_media"] = json!(singles);
                api.invoke("messages.sendMultiMedia", request).await
            } else {
                request["media"] = media[start].clone();
                request["message"] = json!(if start == 0 { caption.as_str() } else { "" });
                request["entities"] = json!(if start == 0 { formatting.clone() } else { vec![] });
                request["random_id"] = args::random_id();
                api.invoke("messages.sendMedia", request).await
            }
        }.await;
        match outcome {
            Ok(value) => completed.push(value),
            Err(error) if completed.is_empty() => return Err(error),
            Err(_) => {
                return Ok(
                    json!({"partial":true,"completed":completed,"failed_file_index":start,"outcome":"The current group may have an unknown send outcome; earlier groups completed. Do not retry the entire request."}),
                );
            }
        }
    }
    Ok(if completed.len() == 1 {
        completed.remove(0)
    } else {
        json!({"results":completed})
    })
}
