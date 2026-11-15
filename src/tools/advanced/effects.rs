use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
    time::{Duration, Instant},
};

struct Catalogue {
    data: Value,
    checked: Instant,
    unknown: HashSet<i64>,
}
static CACHE: OnceLock<tokio::sync::Mutex<HashMap<String, Catalogue>>> = OnceLock::new();
fn has(data: &Value, id_value: i64) -> bool {
    data["effects"]
        .as_array()
        .is_some_and(|v| v.iter().any(|e| id(&e["id"]).ok() == Some(id_value)))
}
async fn catalogue(
    api: &dyn Telegram,
    _args: &Value,
    wanted: Option<i64>,
    hard: bool,
) -> Result<Value> {
    let Some(k) = api.cache_key() else {
        return api
            .invoke("messages.getAvailableEffects", json!({"hash":0}))
            .await;
    };
    let mut cache = CACHE
        .get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
        .lock()
        .await;
    let old = cache.get(&k);
    let cold = old.is_none();
    let due = old.is_none_or(|c| c.checked.elapsed() >= Duration::from_secs(3600));
    let missing =
        old.is_some_and(|c| wanted.is_some_and(|i| !has(&c.data, i) && !c.unknown.contains(&i)));
    if hard || due || missing {
        let hash = if hard || cold {
            json!(0)
        } else {
            old.unwrap().data["hash"].clone()
        };
        let data = api
            .invoke("messages.getAvailableEffects", json!({"hash":hash}))
            .await?;
        if data["_"] == "messages.availableEffectsNotModified" {
            let old = cache.get_mut(&k).ok_or_else(|| {
                anyhow::anyhow!("Telegram returned NotModified without a cached catalogue")
            })?;
            old.checked = Instant::now();
        } else {
            ensure!(
                data["effects"].is_array() && data["documents"].is_array(),
                "Telegram returned an invalid effect catalogue"
            );
            if cache.len() >= 64 && !cache.contains_key(&k) {
                cache.clear();
            }
            cache.insert(
                k.clone(),
                Catalogue {
                    data,
                    checked: Instant::now(),
                    unknown: HashSet::new(),
                },
            );
        }
    }
    let current = cache
        .get_mut(&k)
        .ok_or_else(|| anyhow::anyhow!("Effect catalogue unavailable"))?;
    if let Some(i) = wanted
        && !has(&current.data, i)
    {
        if current.unknown.len() >= 1024 {
            current.unknown.clear();
        }
        current.unknown.insert(i);
    }
    Ok(current.data.clone())
}
fn find_doc(data: &Value, did: &Value) -> Result<Value> {
    data["documents"]
        .as_array()
        .and_then(|v| v.iter().find(|d| id(&d["id"]).ok() == id(did).ok()))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Effect catalogue references a missing document"))
}
fn metadata(data: &Value, e: &Value) -> Value {
    let mut result = e.clone();
    let mut icon = json!("emoticon");
    for (key, out) in [
        ("static_icon_id", "static_icon"),
        ("effect_sticker_id", "preview_sticker"),
        ("effect_animation_id", "effect_animation"),
    ] {
        if let Some(d) = e.get(key).filter(|v| !v.is_null()) {
            match find_doc(data, d) {
                Ok(doc) => {
                    result[out] = doc;
                    if key == "static_icon_id" {
                        icon = json!("static_icon");
                    }
                }
                Err(_) => {
                    result[out] = json!({"document_id":d,"unresolved":true});
                    if key == "static_icon_id" {
                        icon = json!("unresolved_reference");
                    }
                }
            }
        }
    }
    result["effect_id"] = e["id"].clone();
    result["icon_source"] = icon;
    if e.get("effect_animation_id").is_some_and(|v| !v.is_null()) {
        result["animation_source"] = json!(if result["effect_animation"]["unresolved"] == true {
            "unresolved_reference"
        } else {
            "effect_animation"
        });
    } else if let Ok(sticker) = find_doc(data, &e["effect_sticker_id"]) {
        if let Some(thumb) = sticker["video_thumbs"]
            .as_array()
            .and_then(|v| v.iter().find(|v| v["type"] == "f"))
        {
            result["animation_source"] = json!("premium_effect_of_preview_sticker");
            result["effect_animation"] = json!({"asset":"premium_effect_of_preview_sticker","document_id":sticker["id"],"thumb_type":"f","size_bytes":thumb["size"],"width":thumb["w"],"height":thumb["h"]});
        } else {
            result["animation_source"] = json!("none");
        }
    } else {
        result["animation_source"] = json!("unresolved_reference");
    }
    result["note"] = json!(
        "A message effect is separate from a premium sticker's own effect. Asset frames alone do not show the final chat composition."
    );
    result
}
fn location(data: &Value, e: &Value, asset: &str) -> Result<Value> {
    let key = if asset == "icon" {
        "static_icon_id"
    } else if asset == "sticker" {
        "effect_sticker_id"
    } else {
        "effect_animation_id"
    };
    if let Some(did) = e.get(key).filter(|v| !v.is_null()) {
        return crate::tools::messaging::media::download_location(&find_doc(data, did)?);
    }
    if asset == "animation" {
        let sticker = find_doc(data, &e["effect_sticker_id"])?;
        let f = sticker["video_thumbs"]
            .as_array()
            .and_then(|v| v.iter().find(|v| v["type"] == "f"));
        ensure!(
            f.is_some(),
            "Effect has no animation document or premium-effect thumbnail"
        );
        let mut location = crate::tools::messaging::media::download_location(&sticker)?;
        location["thumb_size"] = json!("f");
        return Ok(location);
    }
    anyhow::bail!("Effect has no requested asset; metadata reports the fallback emoticon")
}
pub(super) async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    let wanted = if name == "get_message_effect" {
        Some(positive(a::required(args, "effect_id")?)?)
    } else {
        None
    };
    let asset = string(args, "asset", "metadata")?;
    ensure!(
        matches!(asset, "metadata" | "icon" | "sticker" | "animation"),
        "asset must be metadata, icon, sticker or animation"
    );
    let max = a::integer(args, "max_bytes", 5 * 1024 * 1024)?;
    ensure!(
        (1..=50 * 1024 * 1024).contains(&max),
        "max_bytes must be 1..=50 MiB"
    );
    let count = a::limit(args, "count", 3, 12)?;
    let dimension = a::integer(args, "max_dimension", 512)?;
    ensure!(
        (64..=2048).contains(&dimension),
        "max_dimension must be 64..=2048"
    );
    let pagination = if name == "list_message_effects" {
        let n = a::limit(args, "limit", 50, 200)?;
        let offset = a::integer(args, "offset", 0)?;
        ensure!(offset >= 0, "offset must be nonnegative");
        Some((n, offset))
    } else {
        None
    };
    let data = catalogue(api, args, wanted, false).await?;
    let mut list = data["effects"].as_array().cloned().unwrap_or_default();
    list.sort_by_key(|e| id(&e["id"]).unwrap_or(0));
    if name == "list_message_effects" {
        let (n, offset) = pagination.unwrap();
        let emoji = args.get("emoticon").and_then(Value::as_str);
        let premium = a::boolean(args, "premium_only", false)?;
        let filtered: Vec<_> = list
            .into_iter()
            .filter(|e| {
                emoji.is_none_or(|s| e["emoticon"] == s)
                    && (!premium || e["premium_required"].as_bool() == Some(true))
            })
            .collect();
        let rows: Vec<_> = filtered
            .iter()
            .skip(offset as usize)
            .take(n as usize)
            .map(|e| metadata(&data, e))
            .collect();
        return Ok(
            json!({"records":rows,"total":filtered.len(),"offset":offset,"limit":n,"next_offset":if offset as usize+rows.len()<filtered.len(){json!(offset+rows.len()as i64)}else{Value::Null}}),
        );
    }
    let effect = wanted.unwrap();
    let info = list
        .iter()
        .find(|e| id(&e["id"]).ok() == Some(effect))
        .ok_or_else(|| anyhow::anyhow!("Effect is absent from Telegram's current catalogue"))?;
    if asset == "metadata" {
        return Ok(metadata(&data, info));
    }
    let loc = location(&data, info, asset)?;
    let bytes = match crate::tools::messaging::media::download_bytes(api, loc, max as u64).await {
        Ok(bytes) => bytes,
        Err(error)
            if [
                "FILE_REFERENCE_EXPIRED",
                "FILE_REFERENCE_EMPTY",
                "FILE_REFERENCE_INVALID",
            ]
            .iter()
            .any(|s| error.to_string().contains(s)) =>
        {
            let fresh = catalogue(api, args, wanted, true).await?;
            let e = fresh["effects"]
                .as_array()
                .and_then(|v| v.iter().find(|e| id(&e["id"]).ok() == Some(effect)))
                .ok_or_else(|| anyhow::anyhow!("Effect was removed during reference refresh"))?;
            crate::tools::messaging::media::download_bytes(
                api,
                location(&fresh, e, asset)?,
                max as u64,
            )
            .await?
        }
        Err(error) => return Err(error),
    };
    let mut rendered = crate::tools::messaging::media::previews::render(
        &bytes,
        if asset == "icon" { 1 } else { count },
        dimension,
    )
    .await?;
    rendered["effect"] = metadata(&data, info);
    rendered["asset"] = json!(asset);
    Ok(rendered)
}
