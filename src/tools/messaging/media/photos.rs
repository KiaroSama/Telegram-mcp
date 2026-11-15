use super::super::helpers::*;
use super::{downloads, previews};
use crate::{args, backend::Telegram};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "list_photos" | "open_photo" | "get_photo_sheet" => Some(run(api, name, a).await),
        _ => None,
    }
}
async fn list(api: &dyn Telegram, p: &Value, source: &str, limit: i64) -> Result<Vec<Value>> {
    if source == "avatars" && matches!(p["_"].as_str(), Some("inputPeerUser" | "inputPeerSelf")) {
        let result = api
            .invoke(
                "photos.getUserPhotos",
                json!({"user_id":input_user(p)?,"offset":0,"max_id":"0","limit":limit}),
            )
            .await?;
        return Ok(result["photos"].as_array().cloned().unwrap_or_default());
    }
    let mut req = history_request(p.clone(), limit, 0, 0);
    req.as_object_mut().unwrap().remove("offset_date");
    req["q"] = json!("");
    req["min_date"] = json!(0);
    req["max_date"] = json!(0);
    req["filter"] = json!({"_":if source=="avatars"{"inputMessagesFilterChatPhotos"}else{"inputMessagesFilterPhotos"}});
    let result = api.invoke("messages.search", req).await?;
    let mut photos: Vec<Value> = result_messages(&result)
        .into_iter()
        .filter_map(|message| {
            message["media"]
                .get("photo")
                .or_else(|| message["action"].get("photo"))
                .cloned()
        })
        .collect();
    if source == "avatars" && photos.is_empty() {
        let result = if let Some(channel) = channel(p) {
            api.invoke("channels.getFullChannel", json!({"channel":channel}))
                .await?
        } else if p["_"].as_str() == Some("inputPeerChat") {
            api.invoke("messages.getFullChat", json!({"chat_id":p["chat_id"]}))
                .await?
        } else {
            bail!("Cannot resolve this peer's avatar history")
        };
        if let Some(photo) = result["full_chat"]
            .get("chat_photo")
            .filter(|p| p["_"].as_str() == Some("photo"))
        {
            photos.push(photo.clone());
        }
    }
    Ok(photos)
}
async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let source = a["source"]
        .as_str()
        .unwrap_or("avatars")
        .trim()
        .to_ascii_lowercase();
    ensure!(
        matches!(source.as_str(), "avatars" | "messages"),
        "source must be avatars or messages"
    );
    let bound = args::limit(
        a,
        if name == "open_photo" {
            "search_depth"
        } else {
            "limit"
        },
        if name == "open_photo" {
            50
        } else if name == "get_photo_sheet" {
            6
        } else {
            20
        },
        if name == "get_photo_sheet" { 24 } else { 100 },
    )?;
    let p = args::peer(api, a, "chat_id").await?;
    let photos = list(api, &p, &source, bound).await?;
    if name == "list_photos" {
        return Ok(
            json!({"source":source,"photos":photos.iter().enumerate().map(|(index,photo)|json!({"id":photo["id"],"date":photo["date"],"is_current":source=="avatars"&&index==0})).collect::<Vec<_>>()}),
        );
    }
    let cap = args::integer(a, "max_bytes", 20 * 1024 * 1024)?;
    ensure!(
        cap > 0 && cap <= 50 * 1024 * 1024,
        "max_bytes must be between 1 and 50 MiB"
    );
    let dimension = args::integer(a, "max_dimension", 1024)?;
    if name == "open_photo" {
        let wanted = a.get("photo_id").filter(|v| !v.is_null()).map(|id| {
            id.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| id.to_string())
        });
        let photo = photos
            .iter()
            .find(|photo| {
                wanted.as_ref().is_none_or(|id| {
                    photo["id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| photo["id"].to_string())
                        == *id
                })
            })
            .context("The requested photo was not found within search_depth")?;
        let data = downloads::download_bytes(api, downloads::download_location(photo)?, cap as u64)
            .await?;
        return previews::image_content(
            &data,
            dimension,
            json!({"photo_id":photo["id"],"source":source}),
        );
    }
    ensure!(!photos.is_empty(), "No photos were found");
    let mut images = vec![];
    for photo in &photos {
        let data = downloads::download_bytes(api, downloads::download_location(photo)?, cap as u64)
            .await?;
        images.push((
            data,
            photo["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| photo["id"].to_string()),
        ));
    }
    let columns = if a.get("columns").is_some_and(|v| !v.is_null()) {
        let n = args::integer(a, "columns", 0)?;
        ensure!((1..=24).contains(&n), "columns must be between 1 and 24");
        Some(n as usize)
    } else {
        None
    };
    let (bytes, meta) = crate::tools::visual::images::compose_contact_sheet(&images, columns)?;
    previews::image_content(
        &bytes,
        1568,
        json!({"source":source,"sheet":meta,"photo_ids":photos.iter().map(|p|p["id"].clone()).collect::<Vec<_>>()}),
    )
}
