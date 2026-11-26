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
        return Ok(result["photos"]
            .as_array()
            .map(|photos| photos.iter().take(limit as usize).cloned().collect())
            .unwrap_or_default());
    }
    let mut req = history_request(p.clone(), limit, 0, 0);
    if let Some(fields) = req.as_object_mut() {
        fields.remove("offset_date");
    }
    req["q"] = json!("");
    req["min_date"] = json!(0);
    req["max_date"] = json!(0);
    req["filter"] = json!({"_":if source=="avatars"{"inputMessagesFilterChatPhotos"}else{"inputMessagesFilterPhotos"}});
    let result = api.invoke("messages.search", req).await?;
    let mut photos: Vec<Value> = result_messages(&result)
        .into_iter()
        .take(limit as usize)
        .filter_map(|message| {
            let mut photo = message["media"]
                .get("photo")
                .or_else(|| message["action"].get("photo"))?
                .clone();
            if source == "messages" {
                photo["message_id"] = message["id"].clone();
                photo["caption"] = message["message"].clone();
            }
            Some(photo)
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
fn photo_key(photo: &Value) -> &Value {
    photo.get("message_id").unwrap_or(&photo["id"])
}
fn location(photo: &Value, thumbnail: bool) -> Result<Value> {
    let mut location = downloads::download_location(photo)?;
    if thumbnail {
        let size = photo["sizes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| {
                matches!(s["_"].as_str(), Some("photoSize" | "photoSizeProgressive"))
                    && s["w"].as_u64().is_some_and(|w| (1..=320).contains(&w))
            })
            .max_by_key(|s| s["w"].as_u64())
            .context("No bounded photo thumbnail is available")?;
        location["thumb_size"] = size["type"].clone();
    }
    Ok(location)
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
    let thumbnail = args::boolean(a, "thumbnail", name == "get_photo_sheet")?;
    let photos = if source == "messages"
        && name == "open_photo"
        && a.get("photo_id").is_some_and(|v| !v.is_null())
    {
        let mid = id(a, "photo_id")?;
        let message = fetch(api, &p, mid).await?;
        let mut photo = message["media"]
            .get("photo")
            .filter(|p| p["_"] == "photo")
            .cloned()
            .context("No photo in the requested message")?;
        photo["message_id"] = json!(mid);
        photo["caption"] = message["message"].clone();
        vec![photo]
    } else {
        list(api, &p, &source, bound).await?
    };
    if name == "list_photos" {
        return Ok(
            json!({"source":source,"photos":photos.iter().enumerate().map(|(index,photo)|json!({"id":photo_key(photo),"photo_id":photo_key(photo),"caption":photo["caption"],"date":photo["date"],"is_current":source=="avatars"&&index==0})).collect::<Vec<_>>()}),
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
                    photo_key(photo)
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| photo_key(photo).to_string())
                        == *id
                })
            })
            .context("The requested photo was not found within search_depth")?;
        let data = downloads::download_bytes(api, location(photo, thumbnail)?, cap as u64).await?;
        return previews::image_content(
            &data,
            dimension,
            json!({"photo_id":photo_key(photo),"caption":photo["caption"],"source":source}),
        );
    }
    ensure!(!photos.is_empty(), "No photos were found");
    let mut images = vec![];
    let mut retained = 0usize;
    for photo in &photos {
        let cell: Result<Vec<u8>> = async {
            let bytes =
                downloads::download_bytes(api, location(photo, thumbnail)?, cap as u64).await?;
            let image = crate::tools::visual::images::decode_image(&bytes)?;
            Ok(crate::tools::visual::images::encode_image(&image, "png", 256, 85, false)?.0)
        }
        .await;
        let cell = cell.unwrap_or_default();
        retained = retained
            .checked_add(cell.len())
            .context("Photo sheet byte budget overflow")?;
        ensure!(
            retained <= 32 * 1024 * 1024,
            "Photo sheet cells exceed the 32 MiB aggregate budget"
        );
        images.push((
            cell,
            photo_key(photo)
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| photo_key(photo).to_string()),
        ));
    }
    let columns = if a.get("columns").is_some_and(|v| !v.is_null()) {
        let n = args::integer(a, "columns", 0)?;
        ensure!((1..=24).contains(&n), "columns must be between 1 and 24");
        Some(n as usize)
    } else {
        None
    };
    let (bytes, mut meta) = crate::tools::visual::images::compose_contact_sheet(&images, columns)?;
    if let Some(cells) = meta["cells"].as_array_mut() {
        for (i, c) in cells.iter_mut().enumerate() {
            c["is_current"] = json!(source == "avatars" && i == 0);
            c["failed"] = json!(c["decoded"] != true);
        }
    }
    previews::image_content(
        &bytes,
        1568,
        json!({"source":source,"sheet":meta,"photo_ids":photos.iter().map(photo_key).collect::<Vec<_>>()}),
    )
}
