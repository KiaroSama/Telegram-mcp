use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub const NAMES: &[&str] = &[
    "mute_chat",
    "unmute_chat",
    "set_chat_sound_disabled",
    "set_chat_tone",
    "list_saved_sounds",
    "remove_saved_sound",
];

async fn update(api: &dyn Telegram, p: Value, changes: Value) -> Result<Value> {
    let notify = json!({"_":"inputNotifyPeer","peer":p});
    let current = api
        .invoke("account.getNotifySettings", json!({"peer":notify}))
        .await?;
    let mut fields = json!({"_":"inputPeerNotifySettings"});
    for key in [
        "show_previews",
        "silent",
        "mute_until",
        "stories_muted",
        "stories_hide_sender",
    ] {
        if let Some(value) = current.get(key).filter(|v| !v.is_null()) {
            fields[key] = value.clone();
        }
    }
    if let Some(sound) = current.get("other_sound").filter(|v| !v.is_null()) {
        fields["sound"] = sound.clone();
    }
    if let Some(sound) = current.get("stories_other_sound").filter(|v| !v.is_null()) {
        fields["stories_sound"] = sound.clone();
    }
    for (key, value) in changes
        .as_object()
        .context("notification changes must be an object")?
    {
        fields[key] = value.clone();
    }
    api.invoke(
        "account.updateNotifySettings",
        json!({"peer":notify,"settings":fields}),
    )
    .await
}
fn title(doc: &Value) -> String {
    doc.get("attributes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|a| kind(a) == "documentAttributeFilename")
        .and_then(|a| a["file_name"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            doc["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| doc["id"].to_string())
        })
}
async fn saved(api: &dyn Telegram) -> Result<Vec<Value>> {
    let r = api
        .invoke("account.getSavedRingtones", json!({"hash":0}))
        .await?;
    Ok(r.get("ringtones")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}
fn find_sound<'a>(docs: &'a [Value], wanted: &str) -> Result<&'a Value> {
    let key = wanted.trim().to_lowercase();
    docs.iter()
        .find(|d| {
            let name = title(d);
            let stem = Path::new(&name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let id = d["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| d["id"].to_string());
            key == id || key == name.to_lowercase() || key == stem.to_lowercase()
        })
        .context("No saved sound matches this id or title; use list_saved_sounds")
}
pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    match name {
        "mute_chat" => {
            let days = int(a, "days", 0)?;
            let hours = int(a, "hours", 0)?;
            let minutes = int(a, "minutes", 0)?;
            ensure!(
                days >= 0 && hours >= 0 && minutes >= 0,
                "days, hours and minutes must be whole numbers, zero or more"
            );
            let span = days
                .checked_mul(86400)
                .and_then(|n| hours.checked_mul(3600).and_then(|h| n.checked_add(h)))
                .and_then(|n| minutes.checked_mul(60).and_then(|m| n.checked_add(m)))
                .context("Mute duration is too large")?;
            let until = if span == 0 {
                i32::MAX as i64
            } else {
                now()?
                    .checked_add(span)
                    .context("Mute duration is too large")?
            };
            ensure!(
                until <= i32::MAX as i64,
                "Mute duration exceeds Telegram's supported timestamp; use all-zero duration for forever"
            );
            let p = peer(api, a, "chat_id").await?;
            update(api, p, json!({"mute_until":until})).await
        }
        "unmute_chat" => {
            let p = peer(api, a, "chat_id").await?;
            update(api, p, json!({"mute_until":0})).await
        }
        "set_chat_sound_disabled" => {
            let disabled = req(a, "disabled")?
                .as_bool()
                .context("disabled must be boolean")?;
            let p = peer(api, a, "chat").await?;
            update(api, p, json!({"silent":disabled})).await
        }
        "list_saved_sounds" => {
            let mut rows = Vec::new();
            for d in saved(api).await? {
                let seconds = d
                    .get("attributes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find(|v| kind(v) == "documentAttributeAudio")
                    .and_then(|v| v.get("duration"))
                    .cloned();
                rows.push(json!({"id":d["id"],"title":title(&d),"duration":seconds}));
            }
            Ok(json!({"results":rows,"untrusted_content":true}))
        }
        "remove_saved_sound" => {
            let wanted = text(a, "sound")?;
            let docs = saved(api).await?;
            let d = find_sound(&docs, wanted)?;
            api.invoke(
                "account.saveRingtone",
                json!({"id":input_document(d)?,"unsave":true}),
            )
            .await
        }
        "set_chat_tone" => {
            let tone = a.get("tone").filter(|v| !v.is_null());
            let file = a.get("file_path").filter(|v| !v.is_null());
            ensure!(
                tone.is_some() != file.is_some(),
                "Give exactly one of tone and file_path"
            );
            let sound = if let Some(file) = file {
                let path = Path::new(file.as_str().context("file_path must be a string")?);
                let extension = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let mime = match extension.as_str() {
                    "mp3" => "audio/mpeg",
                    "ogg" | "opus" => "audio/ogg",
                    _ => anyhow::bail!(
                        "A notification sound must be MP3 or OGG OPUS (.mp3, .ogg, .opus)"
                    ),
                };
                let uploaded = api.upload(path).await?;
                let doc=api.invoke("account.uploadRingtone",json!({"file":uploaded,"file_name":path.file_name().and_then(|s|s.to_str()).context("Invalid sound filename")?,"mime_type":mime})).await?;
                let saved = api
                    .invoke(
                        "account.saveRingtone",
                        json!({"id":input_document(&doc)?,"unsave":false}),
                    )
                    .await?;
                json!({"_":"notificationSoundRingtone","id":saved.get("document").filter(|v|!v.is_null()).unwrap_or(&doc)["id"]})
            } else {
                let wanted = tone
                    .and_then(Value::as_str)
                    .context("tone must be a string")?
                    .trim();
                match wanted.to_ascii_lowercase().as_str() {
                    "default" => json!({"_":"notificationSoundDefault"}),
                    "none" => json!({"_":"notificationSoundNone"}),
                    _ => {
                        let docs = saved(api).await?;
                        let d = find_sound(&docs, wanted)?;
                        json!({"_":"notificationSoundRingtone","id":d["id"]})
                    }
                }
            };
            let p = peer(api, a, "chat").await?;
            update(api, p, json!({"sound":sound})).await
        }
        _ => unreachable!(),
    }
}
