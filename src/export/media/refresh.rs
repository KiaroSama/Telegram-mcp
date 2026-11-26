//! Single-retry `FILE_REFERENCE_*` refresh and document-thumbnail download helpers.

use super::{identifier, number, signed_id};
use crate::export::{files, settings, thumbnail};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

fn is_file_reference_error(error: &anyhow::Error) -> bool {
    format!("{error:#}")
        .to_ascii_uppercase()
        .contains("FILE_REFERENCE_")
}

fn find_fresh_media_object<'a>(
    carrier: &'a Value,
    want_photo: bool,
    target_id: &str,
) -> Option<&'a Value> {
    let candidates = [
        if want_photo {
            &carrier["media"]["photo"]
        } else {
            &carrier["media"]["document"]
        },
        &carrier["action"]["photo"],
        &carrier["action"]["new_photo"],
        if want_photo {
            &carrier["action"]["message"]["media"]["photo"]
        } else {
            &carrier["action"]["message"]["media"]["document"]
        },
        if want_photo {
            &carrier["action"]["new_message"]["media"]["photo"]
        } else {
            &carrier["action"]["new_message"]["media"]["document"]
        },
        if want_photo {
            &carrier["action"]["prev_message"]["media"]["photo"]
        } else {
            &carrier["action"]["prev_message"]["media"]["document"]
        },
    ];
    candidates.into_iter().find(|obj| {
        obj.is_object()
            && identifier(&obj["id"]) == target_id
            && obj.get("file_reference").is_some()
    })
}

async fn refresh_file_reference_once(
    api: &dyn crate::backend::Telegram,
    message: &Value,
    want_photo: bool,
    target_id: &str,
) -> Option<Value> {
    if message["custom_emoji_asset"] == true {
        let resp = api
            .invoke(
                "messages.getCustomEmojiDocuments",
                json!({"document_id": [target_id]}),
            )
            .await
            .ok()?;
        return resp
            .as_array()?
            .iter()
            .find(|doc| identifier(&doc["id"]) == target_id && doc.get("file_reference").is_some())
            .cloned();
    }
    let peer = message.get("source_peer").filter(|p| p.is_object());
    let msg_id = signed_id(&message["source_message_id"])
        .or_else(|| signed_id(&message["id"]))
        .filter(|&id| id > 0);

    if message["rich_asset"] == true
        && let (Some(peer), Some(mid)) = (peer, msg_id)
    {
        let resp = api
            .invoke("messages.getRichMessage", json!({"peer": peer, "id": mid}))
            .await
            .ok()?;
        let list_key = if want_photo { "photos" } else { "documents" };
        for m in resp["messages"].as_array().into_iter().flatten() {
            if signed_id(&m["id"]) != Some(mid) {
                continue;
            }
            if let Some(found) = m["rich_message"][list_key]
                .as_array()
                .into_iter()
                .flatten()
                .find(|o| identifier(&o["id"]) == target_id && o.get("file_reference").is_some())
            {
                return Some(found.clone());
            }
        }
        return None;
    }

    if let (Some(peer), Some(mid)) = (peer, msg_id) {
        let fetched = if peer["_"].as_str() == Some("inputPeerChannel") {
            api.invoke(
                "channels.getMessages",
                json!({
                    "channel": {
                        "_": "inputChannel",
                        "channel_id": peer["channel_id"],
                        "access_hash": peer["access_hash"],
                    },
                    "id": [{"_": "inputMessageID", "id": mid}],
                }),
            )
            .await
            .ok()
        } else {
            api.invoke(
                "messages.getMessages",
                json!({"id": [{"_": "inputMessageID", "id": mid}]}),
            )
            .await
            .ok()
        };
        if let Some(resp) = fetched {
            for m in resp["messages"].as_array().into_iter().flatten() {
                if signed_id(&m["id"]) == Some(mid)
                    && let Some(obj) = find_fresh_media_object(m, want_photo, target_id)
                {
                    return Some(obj.clone());
                }
            }
        }
    }

    if let (Some(peer), Some(ev_id)) = (
        peer.filter(|p| p["_"].as_str() == Some("inputPeerChannel")),
        signed_id(&message["source_event_id"]).filter(|&id| id > 0),
    ) {
        let resp = api
            .invoke(
                "channels.getAdminLog",
                json!({
                    "channel": {
                        "_": "inputChannel",
                        "channel_id": peer["channel_id"],
                        "access_hash": peer["access_hash"],
                    },
                    "q": "",
                    "max_id": ev_id.saturating_add(1).to_string(),
                    "min_id": ev_id.saturating_sub(1).to_string(),
                    "limit": 5,
                }),
            )
            .await
            .ok()?;
        for ev in resp["events"].as_array().into_iter().flatten() {
            if signed_id(&ev["id"]) == Some(ev_id)
                && let Some(obj) = find_fresh_media_object(ev, want_photo, target_id)
            {
                return Some(obj.clone());
            }
        }
    }
    None
}

pub(super) async fn download_with_single_refresh(
    api: &dyn crate::backend::Telegram,
    mut location: Value,
    destination: &Path,
    max_bytes: u64,
    message: &mut Value,
    want_photo: bool,
    target_id: &str,
) -> bool {
    match api.download(location.clone(), destination, max_bytes).await {
        Ok(_) => true,
        Err(first_err) => {
            if !is_file_reference_error(&first_err) {
                return false;
            }
            let prev = message["export_refreshed"].as_u64().unwrap_or(0);
            message["export_refreshed"] = json!(prev + 1);
            let Some(fresh_obj) =
                refresh_file_reference_once(api, message, want_photo, target_id).await
            else {
                return false;
            };
            location["file_reference"] = fresh_obj["file_reference"].clone();
            if let Some(ah) = fresh_obj.get("access_hash") {
                location["access_hash"] = ah.clone();
            }
            if let Some(dc) = fresh_obj.get("dc_id") {
                location["_dc_id"] = dc.clone();
            }
            api.download(location, destination, max_bytes).await.is_ok()
        }
    }
}

fn select_best_document_thumb(document: &Value) -> Option<&Value> {
    document["thumbs"]
        .as_array()?
        .iter()
        .filter(|t| {
            let kind = t["_"].as_str().unwrap_or("");
            !matches!(
                kind,
                "photoSizeEmpty" | "photoStrippedSize" | "photoPathSize"
            ) && t["type"].as_str().is_some_and(|s| !s.is_empty())
                && number(&t["w"]) > 0
                && number(&t["h"]) > 0
        })
        .max_by_key(|t| number(&t["w"]).saturating_mul(number(&t["h"])))
}

pub(super) async fn maybe_download_document_thumb(
    api: &dyn crate::backend::Telegram,
    document: &Value,
    relative_doc_path: &str,
    message: &mut Value,
    settings: &settings::Settings,
    directory: &Path,
) -> Result<()> {
    let Some(best) = select_best_document_thumb(document) else {
        return Ok(());
    };
    let thumb_type = best["type"].as_str().unwrap_or_default();
    if thumb_type.is_empty() {
        return Ok(());
    }
    let mut loc = json!({
        "_": "inputDocumentFileLocation",
        "id": document["id"],
        "access_hash": document["access_hash"],
        "file_reference": document["file_reference"],
        "thumb_size": thumb_type,
    });
    if let Some(dc) = document.get("dc_id") {
        loc["_dc_id"] = dc.clone();
    }
    let suggested = format!("{relative_doc_path}_thumb.jpg");
    let state = crate::state::State::from_environment()?;
    let root = state.resolve_file(directory, crate::state::FileAccess::Write)?;
    let rel_raw =
        files::prepare_relative_path(&root, &format!("{relative_doc_path}_rawthumb.tmp"))?;
    let temp_dest = state.resolve_file(&root.join(&rel_raw), crate::state::FileAccess::Write)?;
    if let Some(parent) = temp_dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let doc_id = identifier(&document["id"]);
    let downloaded = download_with_single_refresh(
        api,
        loc,
        &temp_dest,
        settings.max_bytes,
        message,
        false,
        &doc_id,
    )
    .await;
    if !downloaded {
        let _ = std::fs::remove_file(&temp_dest);
        return Ok(());
    }
    let raw_bytes = std::fs::read(&temp_dest).unwrap_or_default();
    let _ = std::fs::remove_file(&temp_dest);
    let (normalized, icc) = thumbnail::normalize_downloaded_thumb(&raw_bytes);
    let final_rel = files::write_new_export_file(directory, &suggested, &normalized)?;
    message["export_thumb"] = Value::String(final_rel);
    message["export_thumb_size"] = json!(normalized.len() as u64);
    if let Some(transform) = icc
        && message.get("export_icc").is_none()
    {
        message["export_icc"] = thumbnail::icc_summary_json(&transform);
    }
    Ok(())
}
