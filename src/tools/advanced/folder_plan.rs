//! Complete folder planning without exposing access references or chat content.
use super::common::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const FLAGS: &[&str] = &[
    "contacts",
    "non_contacts",
    "groups",
    "broadcasts",
    "bots",
    "exclude_muted",
    "exclude_read",
    "exclude_archived",
    "title_noanimate",
];
pub(super) const PEERS: &[(&str, &str)] = &[
    ("include_chat_ids", "include_peers"),
    ("pinned_chat_ids", "pinned_peers"),
    ("exclude_chat_ids", "exclude_peers"),
];
pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    match name {
        "get_folder_limits" => Some(Ok(limits(api).await)),
        "get_folder_snapshot" => Some(snapshot(api).await),
        "update_folder" => Some(super::folder_update::update(api, args).await),
        _ => None,
    }
}
pub(super) async fn me(api: &dyn Telegram) -> Result<Value> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]})),
    )
    .await
    .context("Account identity lookup timed out")??;
    result
        .as_array()
        .and_then(|v| v.first())
        .cloned()
        .context("Account identity is unavailable")
}
pub(super) async fn limits(api: &dyn Telegram) -> Value {
    let premium = me(api).await.ok().and_then(|u| u["premium"].as_bool());
    let config = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.invoke("help.getAppConfig", json!({"hash":0})),
    )
    .await;
    let entries = match &config {
        Ok(Ok(c)) if c["config"]["_"] == "jsonObject" => c["config"]["value"].as_array(),
        _ => None,
    };
    let available = entries.is_some();
    let tier = premium.map(|p| if p { "premium" } else { "default" });
    let mut out = json!({"premium":premium,"config_available":available,"limits":{},"config_keys":{},"title_limit_utf16":12,"unknown_limits":"Unknown limits defer to Telegram's server validation."});
    for (name, key) in [
        ("folders", "dialog_filters_limit"),
        ("chats_per_folder", "dialog_filters_chats_limit"),
        ("pinned_per_folder", "dialogs_folder_pinned_limit"),
    ] {
        let key = tier.map(|t| format!("{key}_{t}"));
        let number = key
            .as_ref()
            .and_then(|key| entries.and_then(|v| v.iter().find(|v| v["key"] == *key)))
            .and_then(|v| {
                if v["value"]["_"] != "jsonNumber" {
                    return None;
                }
                let n = v["value"]["value"].as_f64()?;
                (n.is_finite() && n > 0.0 && n.fract() == 0.0 && n < i64::MAX as f64)
                    .then_some(n as i64)
            });
        out["limits"][name] = json!(number);
        out["config_keys"][name] = json!(key);
    }
    out
}
pub(super) fn list(result: &Value) -> Result<&Vec<Value>> {
    result["filters"]
        .as_array()
        .context("Telegram returned no folder definitions")
}
pub(super) fn uses_self(filters: &[Value]) -> bool {
    filters.iter().any(|f| {
        PEERS.iter().any(|(_, field)| {
            f[*field]
                .as_array()
                .is_some_and(|v| v.iter().any(|p| p["_"] == "inputPeerSelf"))
        })
    })
}
pub(super) async fn self_id(api: &dyn Telegram, filters: &[Value]) -> Result<Option<i64>> {
    if uses_self(filters) {
        Ok(Some(positive(&me(api).await?["id"])?))
    } else {
        Ok(None)
    }
}
pub(super) fn stable(peer: &Value, self_id: Option<i64>) -> Result<i64> {
    if peer["_"] == "inputPeerSelf" {
        return self_id.context(
            "Saved Messages identity is unavailable; a complete snapshot cannot be produced",
        );
    }
    crate::server::access::marked_peer(peer)
        .context("A folder peer has no stable marked ID")?
        .parse()
        .context("Folder peer ID exceeds the signed 64-bit range")
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(object) => serde_json::to_value(
            object
                .iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect::<BTreeMap<_, _>>(),
        )
        .expect("JSON values serialize"),
        Value::Array(array) => json!(array.iter().map(canonical).collect::<Vec<_>>()),
        _ => value.clone(),
    }
}
pub(super) fn state(folder: &Value, self_id: Option<i64>) -> Result<Value> {
    if folder["_"] == "dialogFilterDefault" {
        return Ok(json!({"id":0,"type":"system","editable":false}));
    }
    let shared = folder["_"] == "dialogFilterChatlist";
    ensure!(
        shared || folder["_"] == "dialogFilter",
        "Telegram returned an unknown folder type; snapshot would be incomplete"
    );
    let title = folder["title"]["text"]
        .as_str()
        .context("Folder title is unavailable; a complete snapshot cannot be produced")?;
    let mut definition = json!({"title":title,"title_entities":folder["title"]["entities"].as_array().cloned().unwrap_or_default(),"emoticon":folder["emoticon"],"color":folder["color"],"title_noanimate":folder["title_noanimate"].as_bool().unwrap_or(false)});
    for (name, field) in PEERS {
        if shared && *field == "exclude_peers" {
            continue;
        }
        let values = folder[*field]
            .as_array()
            .context("Folder peer list is missing; snapshot would be incomplete")?;
        definition[*name] = json!(
            values
                .iter()
                .map(|p| stable(p, self_id).map(|n| n.to_string()))
                .collect::<Result<Vec<_>>>()?
        );
    }
    if !shared {
        for flag in FLAGS {
            definition[*flag] = json!(folder[*flag].as_bool().unwrap_or(false));
        }
    }
    let mut for_hash = definition.clone();
    for field in ["include_chat_ids", "exclude_chat_ids"] {
        if let Some(ids) = for_hash[field].as_array_mut() {
            ids.sort_by_key(|v| id(v).unwrap_or(0));
        }
    }
    let kind = if shared { "shared" } else { "private" };
    let hash = Sha256::digest(serde_json::to_vec(&canonical(
        &json!({"id":folder["id"],"type":kind,"definition":for_hash}),
    ))?);
    let mut out = json!({"id":folder["id"],"type":kind,"editable":!shared,"definition":definition,"revision":format!("{hash:x}")});
    if shared {
        out["has_my_invites"] = json!(folder["has_my_invites"].as_bool().unwrap_or(false));
    }
    Ok(out)
}
async fn snapshot(api: &dyn Telegram) -> Result<Value> {
    let _guard = super::folders::FOLDER_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let result = api.invoke("messages.getDialogFilters", json!({})).await?;
    let filters = list(&result)?;
    let self_id = self_id(api, filters).await?;
    let states = filters
        .iter()
        .map(|f| state(f, self_id))
        .collect::<Result<Vec<_>>>()?;
    let order: Vec<_> = states.iter().map(|f| f["id"].clone()).collect();
    Ok(
        json!({"schema_version":1,"folders":states,"folder_order":order,"tags_enabled":result["tags_enabled"],"scope":"Folder definitions only; shared invite state and chat state are not restorable."}),
    )
}
