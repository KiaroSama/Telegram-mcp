use super::helpers::*;
use crate::{args, backend::Telegram};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub async fn call(api: &dyn Telegram, name: &str, a: &Value) -> Option<Result<Value>> {
    match name {
        "inspect_custom_emoji" | "replace_custom_emoji" => Some(emoji(api, name, a).await),
        _ => super::keyboard::call(api, name, a).await,
    }
}
async fn emoji(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let mid = id(a, "message_id")?;
    let replacement = if name == "replace_custom_emoji" {
        let raw = args::required(a, "new_document_id")?;
        let s = raw
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| raw.to_string());
        ensure!(
            s.parse::<i64>().is_ok_and(|n| n > 0),
            "new_document_id must be a positive 64-bit decimal id"
        );
        Some(s)
    } else {
        None
    };
    let target = a
        .get("old_document_id")
        .filter(|v| !v.is_null())
        .map(|raw| {
            raw.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| raw.to_string())
        });
    if let Some(target) = &target {
        ensure!(
            target.parse::<i64>().is_ok_and(|n| n > 0),
            "old_document_id must be a positive 64-bit decimal id"
        );
    }
    let p = args::peer(api, a, "chat_id").await?;
    let message = fetch(api, &p, mid).await?;
    let mut entities = message["entities"].as_array().cloned().unwrap_or_default();
    let mut found = vec![];
    let mut changed = 0;
    let text = message["message"].as_str().unwrap_or("");
    let units: Vec<u16> = text.encode_utf16().collect();
    for entity in &mut entities {
        if entity["_"].as_str() != Some("messageEntityCustomEmoji") {
            continue;
        }
        let current = entity["document_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| entity["document_id"].to_string());
        let offset = entity["offset"].as_u64().unwrap_or(0) as usize;
        let length = entity["length"].as_u64().unwrap_or(0) as usize;
        let glyph = units
            .get(offset..offset.saturating_add(length))
            .and_then(|part| String::from_utf16(part).ok());
        found.push(json!({"index":found.len(),"document_id":current,"offset":offset,"length":length,"fallback_glyph":glyph}));
        if let Some(new) = &replacement
            && target.as_ref().is_none_or(|wanted| wanted == &current)
        {
            entity["document_id"] = json!(new);
            changed += 1;
        }
    }
    if replacement.is_none() {
        return Ok(
            json!({"chat_id":a["chat_id"],"message_id":mid,"text":text,"entities":entities,"custom_emoji":found}),
        );
    }
    ensure!(
        changed > 0,
        "No matching custom emoji was found; nothing was changed"
    );
    let entities = build_entities(api, text, &json!(entities)).await?;
    let result = api
        .invoke(
            "messages.editMessage",
            json!({"peer":p,"id":mid,"message":text,"entities":entities}),
        )
        .await?;
    Ok(json!({"changed":changed,"message_id":mid,"result":result}))
}
