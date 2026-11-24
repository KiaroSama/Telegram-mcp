//! Installed, featured and searched emoji packs with real, optional image previews.
use super::{common::*, emoji_assets};
use crate::{args as a, backend::Telegram};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    match name {
        "get_custom_emoji_packs" => Some(packs(api, args).await),
        "get_custom_emoji_pack" => Some(pack(api, args).await),
        "preview_custom_emoji" => Some(emoji_assets::preview(api, args).await),
        _ => None,
    }
}
pub(super) fn short_name(raw: &str) -> Result<String> {
    fn valid(s: &str) -> bool {
        !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
    let raw = raw.trim();
    ensure!(raw.len() <= 2048, "Emoji pack reference is too long");
    if valid(raw) {
        return Ok(raw.into());
    }
    let full = if raw.starts_with("t.me/") || raw.starts_with("telegram.me/") {
        format!("https://{raw}")
    } else {
        raw.into()
    };
    let url = url::Url::parse(&full).context("Invalid emoji pack name or Telegram URL")?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.fragment().is_none(),
        "Invalid emoji pack URL"
    );
    let result = if url.scheme() == "tg"
        && matches!(url.host_str(), Some("addemoji" | "addstickers"))
        && url.path().is_empty()
    {
        let values: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "set")
            .map(|(_, v)| v.into_owned())
            .collect();
        ensure!(values.len() == 1, "Emoji pack URL needs one set name");
        values[0].clone()
    } else {
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && matches!(url.host_str(), Some("t.me" | "telegram.me")),
            "Emoji pack URLs must use Telegram's official link hosts"
        );
        let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
        ensure!(
            parts.len() == 2 && matches!(parts[0], "addemoji" | "addstickers"),
            "Invalid Telegram emoji pack URL path"
        );
        parts[1].into()
    };
    ensure!(valid(&result), "Invalid emoji pack short name");
    Ok(result)
}
fn metadata(set: &Value) -> Result<Value> {
    let sid = id(&set["id"])?;
    let name = set["short_name"].as_str().unwrap_or("");
    let emoji = set["emojis"].as_bool().unwrap_or(false);
    let mut out = json!({"id":sid.to_string(),"title":set["title"],"short_name":name,"count":set["count"],"is_emoji":emoji,"emojis":emoji,"url":if name.is_empty(){Value::Null}else{json!(format!("https://t.me/{}/{name}",if emoji{"addemoji"}else{"addstickers"}))}});
    for flag in ["archived", "official", "masks"] {
        out[flag] = json!(set[flag].as_bool().unwrap_or(false));
    }
    if let Some(date) = set.get("installed_date").filter(|v| !v.is_null()) {
        out["installed_date"] = date.clone();
    }
    Ok(out)
}
async fn packs(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let limit = a::limit(args, "limit", 50, 200)?;
    let offset = a::integer(args, "offset", 0)?;
    ensure!((0..=100_000).contains(&offset), "offset must be 0..=100000");
    let query = string(args, "query", "")?.trim();
    ensure!(
        query.chars().count() <= 256,
        "Emoji pack query exceeds 256 characters"
    );
    let (method, params, source) = if !query.is_empty() {
        (
            "messages.searchEmojiStickerSets",
            json!({"q":query,"hash":0}),
            "search",
        )
    } else if a::boolean(args, "featured", false)? {
        (
            "messages.getFeaturedEmojiStickers",
            json!({"hash":0}),
            "featured",
        )
    } else {
        ("messages.getEmojiStickers", json!({"hash":0}), "installed")
    };
    let result = api.invoke(method, params).await?;
    let sets = result["sets"]
        .as_array()
        .context("Telegram returned no emoji pack catalogue")?;
    let rows = sets
        .iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|v| metadata(v.get("set").filter(|v| v.is_object()).unwrap_or(v)))
        .collect::<Result<Vec<_>>>()?;
    let next = offset as usize + rows.len();
    Ok(
        json!({"packs":rows,"source":source,"offset":offset,"limit":limit,"available_in_response":sets.len(),"next_offset":if next<sets.len(){json!(next)}else{Value::Null},"note":"Local paging over Telegram's returned catalogue. Pack titles are untrusted user content."}),
    )
}
fn emoji_record(doc: &Value, fallback: &BTreeMap<i64, String>) -> Result<Value> {
    let did = positive(&doc["id"])?;
    let mut alt = fallback.get(&did).cloned().unwrap_or_default();
    let mut free = false;
    let mut animated = doc["mime_type"] == "application/x-tgsticker";
    let mut video = doc["mime_type"] == "video/webm";
    for attr in doc["attributes"].as_array().into_iter().flatten() {
        match attr["_"].as_str() {
            Some("documentAttributeCustomEmoji") => {
                if let Some(a) = attr["alt"].as_str().filter(|s| !s.is_empty()) {
                    alt = a.into();
                }
                free = attr["free"].as_bool().unwrap_or(false);
            }
            Some("documentAttributeSticker") => {
                if let Some(a) = attr["alt"].as_str().filter(|s| !s.is_empty()) {
                    alt = a.into();
                }
            }
            Some("documentAttributeAnimated") => animated = true,
            Some("documentAttributeVideo") => video = true,
            _ => {}
        }
    }
    let thumbs:Vec<_>=doc["thumbs"].as_array().into_iter().flatten().map(|t|json!({"type":t["type"],"width":t["w"],"height":t["h"],"size_bytes":t["size"],"inline":t.get("bytes").is_some()})).collect();
    Ok(
        json!({"id":did.to_string(),"document_id":did.to_string(),"alt":alt,"emoji":alt,"free":free,"format":if animated{"animated"}else if video{"video"}else{"static"},"mime_type":doc["mime_type"],"thumbnails":thumbs,"thumbnail_available":!thumbs.is_empty(),"preview_available":false}),
    )
}
async fn pack(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let name = short_name(a::text(args, "pack")?)?;
    let requested = a::limit(args, "limit", 50, 200)?;
    let offset = a::integer(args, "offset", 0)?;
    ensure!((0..=100_000).contains(&offset), "offset must be 0..=100000");
    let thumbnails = a::boolean(args, "include_thumbnails", false)?;
    let limit = if thumbnails {
        requested.min(24)
    } else {
        requested
    };
    let cap = a::integer(args, "max_bytes", 8 * 1024 * 1024)?;
    ensure!(
        (1..=50 * 1024 * 1024).contains(&cap),
        "max_bytes must be 1..=50 MiB"
    );
    let dimension = a::integer(args, "max_dimension", 256)?;
    ensure!(
        (64..=1568).contains(&dimension),
        "max_dimension must be 64..=1568"
    );
    let result = api
        .invoke(
            "messages.getStickerSet",
            json!({"stickerset":{"_":"inputStickerSetShortName","short_name":name},"hash":0}),
        )
        .await?;
    let info = metadata(&result["set"])?;
    let docs = result["documents"]
        .as_array()
        .context("Emoji pack has no document list")?;
    let mut fallback = BTreeMap::new();
    for p in result["packs"].as_array().into_iter().flatten() {
        for d in p["documents"].as_array().into_iter().flatten() {
            if let Ok(id) = id(d) {
                fallback
                    .entry(id)
                    .or_insert_with(|| p["emoticon"].as_str().unwrap_or("").into());
            }
        }
    }
    let mut rows = Vec::new();
    let mut content = Vec::new();
    let mut remaining = 50 * 1024 * 1024u64;
    for doc in docs.iter().skip(offset as usize).take(limit as usize) {
        let mut record = emoji_record(doc, &fallback)?;
        if thumbnails && remaining > 0 {
            match emoji_assets::bytes(api, doc, true, (cap as u64).min(remaining)).await {
                Ok((data, thumbnail)) => {
                    remaining = remaining.saturating_sub(data.len() as u64);
                    match crate::tools::messaging::media::previews::render(&data, 1, dimension)
                        .await
                    {
                        Ok(preview) => {
                            let blocks = preview["content"].as_array().cloned().unwrap_or_default();
                            record["thumbnail_content_index"] = json!(content.len());
                            record["thumbnail_used"] = json!(thumbnail);
                            record["preview_available"] = json!(!blocks.is_empty());
                            content.extend(blocks);
                        }
                        Err(_) => {
                            record["thumbnail_error"] = json!(
                                "The actual emoji asset could not be rendered; its fallback glyph is metadata only."
                            )
                        }
                    }
                }
                Err(_) => {
                    record["thumbnail_error"] = json!(
                        "The actual emoji thumbnail could not be downloaded within its byte budget."
                    )
                }
            }
        } else if thumbnails {
            record["thumbnail_error"] = json!("Page preview byte budget exhausted.");
        }
        rows.push(record);
    }
    let next = offset as usize + rows.len();
    let mut out = json!({"pack":info,"emojis":rows,"offset":offset,"limit":limit,"requested_limit":requested,"available_in_response":docs.len(),"next_offset":if next<docs.len(){json!(next)}else{Value::Null},"note":"Emoji glyphs are fallback metadata. Image content, when requested, comes from Telegram's actual thumbnail or media bytes."});
    if !content.is_empty() {
        out["content"] = json!(content);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "../../../tests/unit/advanced_emoji.rs"]
mod tests;
