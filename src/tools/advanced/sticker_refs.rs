//! Session-scoped references retained before public-response redaction.
use super::common::*;
use crate::backend::Telegram;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

struct Reference {
    hash: i64,
    bytes: Vec<u8>,
}
static REFERENCES: OnceLock<Mutex<HashMap<(String, i64), Reference>>> = OnceLock::new();
pub(super) fn remember(api: &dyn Telegram, result: &Value) -> Result<()> {
    let Some(key) = api.cache_key() else {
        return Ok(());
    };
    let mut cache = REFERENCES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Sticker reference cache unavailable"))?;
    for doc in result["documents"].as_array().into_iter().flatten() {
        if doc["_"] != "document" {
            continue;
        }
        let bytes = doc["file_reference"]
            .as_array()
            .filter(|bytes| bytes.len() <= 4096)
            .and_then(|bytes| {
                bytes
                    .iter()
                    .map(|byte| byte.as_u64().and_then(|n| u8::try_from(n).ok()))
                    .collect::<Option<Vec<_>>>()
            });
        let (Ok(did), Ok(hash), Some(bytes)) =
            (positive(&doc["id"]), id(&doc["access_hash"]), bytes)
        else {
            continue;
        };
        if bytes.len() > 4096 {
            continue;
        }
        let entry = (key.clone(), did);
        if cache.len() >= 1024 && !cache.contains_key(&entry) {
            cache.clear();
        }
        cache.insert(entry, Reference { hash, bytes });
    }
    Ok(())
}
pub(super) async fn resolve(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let wanted = positive(crate::args::required(args, "document_id")?)?;
    let supplied = args
        .get("access_hash")
        .filter(|v| !v.is_null())
        .map(id)
        .transpose()?;
    if let (Some(key), Some(cache)) = (api.cache_key(), REFERENCES.get()) {
        let cache = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Sticker reference cache unavailable"))?;
        if let Some(reference) = cache.get(&(key, wanted))
            && supplied.is_none_or(|hash| hash == reference.hash)
        {
            return Ok(
                json!({"_":"inputDocument","id":wanted.to_string(),"access_hash":reference.hash.to_string(),"file_reference":reference.bytes}),
            );
        }
    }
    if let Some(hash) = supplied {
        return Ok(
            json!({"_":"inputDocument","id":wanted.to_string(),"access_hash":hash.to_string(),"file_reference":[]}),
        );
    }
    if let Some(name) = args
        .get("short_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        let result = api
            .invoke(
                "messages.getStickerSet",
                json!({"stickerset":{"_":"inputStickerSetShortName","short_name":name},"hash":0}),
            )
            .await?;
        remember(api, &result)?;
        if let Some(doc) = result["documents"]
            .as_array()
            .and_then(|docs| docs.iter().find(|doc| id(&doc["id"]).ok() == Some(wanted)))
        {
            return document(doc);
        }
        bail!("Sticker document is not in the selected set; inspect its owning set first");
    }
    bail!(
        "Sticker reference is unavailable in this account; inspect its owning set in this session or provide short_name"
    )
}
