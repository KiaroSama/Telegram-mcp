//! Optimistic, non-destructive patches of private Telegram folder definitions.
use super::{common::*, folder_plan as plan};
use crate::{args as a, backend::Telegram};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn entities(raw: &Value, text: Option<&str>) -> Result<Vec<Value>> {
    let raw = raw.as_array().context("title_entities must be a list")?;
    ensure!(raw.len() <= 1000, "Too many folder title entities");
    let mut values = Vec::new();
    for entry in raw {
        let mut entry = entry
            .as_object()
            .cloned()
            .context("A title entity must be an object")?;
        let name = entry
            .get("_")
            .and_then(Value::as_str)
            .context("A title entity needs its TL constructor")?;
        let name = if let Some(suffix) = name.strip_prefix("MessageEntity") {
            format!("messageEntity{suffix}")
        } else {
            name.into()
        };
        ensure!(
            name.starts_with("messageEntity")
                && crate::tl::Schema::global().definition(&name)?.result == "MessageEntity",
            "Unsupported folder title entity"
        );
        ensure!(
            entry.values().all(|v| !v.is_object() && !v.is_array()),
            "Folder title entities may not contain nested objects"
        );
        let offset = entry
            .get("offset")
            .and_then(Value::as_i64)
            .context("Title entity offset must be an integer")?;
        let length = entry
            .get("length")
            .and_then(Value::as_i64)
            .context("Title entity length must be an integer")?;
        ensure!(
            offset >= 0 && length > 0 && offset.checked_add(length).is_some(),
            "Title entity has invalid UTF-16 bounds"
        );
        entry.insert("_".into(), json!(name));
        let value = Value::Object(entry);
        crate::tl::Schema::global().encode_object(&value)?;
        values.push(value);
    }
    if let Some(text) = text {
        return crate::tools::messaging::helpers::entities(text, &json!(values));
    }
    Ok(values)
}
fn patch(args: &Value) -> Result<(i64, Value)> {
    let fid = a::required(args, "folder_id")?
        .as_i64()
        .context("folder_id must be an existing private folder integer ID")?;
    ensure!(
        (2..=i32::MAX as i64).contains(&fid),
        "folder_id must be 2..2147483647"
    );
    let patch = a::required(args, "patch")?
        .as_object()
        .context("patch must be an object")?;
    for key in patch.keys() {
        ensure!(
            plan::FLAGS.contains(&key.as_str())
                || plan::PEERS.iter().any(|(name, _)| *name == key)
                || ["title", "title_entities", "emoticon", "color"].contains(&key.as_str()),
            "Patch contains an unsupported folder field"
        );
    }
    if let Some(title) = patch.get("title") {
        nonempty(
            title.as_str().context("title must be a string")?,
            12,
            "Folder title",
        )?;
    }
    for flag in plan::FLAGS {
        if let Some(v) = patch.get(*flag) {
            ensure!(
                v.is_null() || v.is_boolean(),
                "Folder rules must be boolean or null"
            );
        }
    }
    if let Some(color) = patch.get("color").filter(|v| !v.is_null()) {
        ensure!(
            color.as_i64().is_some_and(|n| (-1..=6).contains(&n)),
            "Folder color must be -1..6 or null"
        );
    }
    if let Some(emoticon) = patch.get("emoticon") {
        ensure!(
            emoticon.is_null() || emoticon.is_string(),
            "Folder emoticon must be text or null"
        );
    }
    for (name, _) in plan::PEERS {
        if let Some(raw) = patch.get(*name) {
            let values = raw.as_array().context("Folder peer lists must be lists")?;
            ensure!(
                values.len() <= 100_000,
                "Folder peer list exceeds its budget"
            );
            let ids: Vec<_> = values.iter().map(id).collect::<Result<_>>()?;
            ensure!(
                ids.iter().all(|n| *n != 0),
                "Folder peer IDs must be nonzero"
            );
            unique(&ids)?;
        }
    }
    if let Some(raw) = patch.get("title_entities") {
        entities(raw, patch.get("title").and_then(Value::as_str))?;
    }
    if let Some(revision) = args.get("expected_revision").filter(|v| !v.is_null()) {
        let s = revision
            .as_str()
            .context("expected_revision must be a SHA-256 string")?;
        ensure!(
            s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()),
            "expected_revision must be a SHA-256 string"
        );
    }
    Ok((fid, json!(patch)))
}
fn peer_set(folder: &Value, field: &str, self_id: Option<i64>) -> Result<BTreeSet<i64>> {
    folder[field]
        .as_array()
        .context("Folder peer list is missing")?
        .iter()
        .map(|p| plan::stable(p, self_id))
        .collect()
}
pub(super) async fn update(api: &dyn Telegram, args: &Value) -> Result<Value> {
    let (fid, patch) = patch(args)?;
    let _guard = super::folders::FOLDER_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let result = api.invoke("messages.getDialogFilters", json!({})).await?;
    let target = plan::list(&result)?
        .iter()
        .find(|f| id(&f["id"]).ok() == Some(fid))
        .context("Folder not found")?;
    ensure!(
        target["_"] == "dialogFilter",
        "Shared and system folders cannot be edited with update_folder"
    );
    let mut self_id = plan::self_id(api, std::slice::from_ref(target)).await?;
    let before = plan::state(target, self_id)?;
    if let Some(expected) = args.get("expected_revision").filter(|v| !v.is_null()) {
        ensure!(
            *expected == before["revision"],
            "Folder changed since the snapshot; get a fresh snapshot first"
        );
    }
    let mut updated = target.clone();
    if let Some(title) = patch.get("title") {
        updated["title"] = txt(title.as_str().context("title must be a string")?);
    }
    if let Some(raw) = patch.get("title_entities") {
        updated["title"]["entities"] = json!(entities(raw, updated["title"]["text"].as_str())?);
    }
    for key in plan::FLAGS {
        if let Some(v) = patch.get(*key) {
            updated[*key] = json!(v.as_bool().unwrap_or(false));
        }
    }
    for key in ["emoticon", "color"] {
        if let Some(v) = patch.get(key) {
            updated[key] = v.clone();
        }
    }
    let mut known = BTreeMap::new();
    for (_, field) in plan::PEERS {
        for p in target[*field]
            .as_array()
            .context("Folder peer list is missing")?
        {
            known.insert(plan::stable(p, self_id)?, p.clone());
        }
    }
    for (name, field) in plan::PEERS {
        if let Some(values) = patch.get(*name) {
            let mut peers = Vec::new();
            for wanted in values
                .as_array()
                .context("Folder peer list must be an array")?
            {
                let wanted_id = id(wanted)?;
                let peer = if let Some(p) = known.get(&wanted_id) {
                    p.clone()
                } else {
                    api.resolve_peer(wanted).await?
                };
                if peer["_"] == "inputPeerSelf" && self_id.is_none() {
                    self_id = Some(positive(&plan::me(api).await?["id"])?);
                }
                ensure!(
                    plan::stable(&peer, self_id)? == wanted_id,
                    "Resolved peer identity differs from the requested marked ID"
                );
                peers.push(peer);
            }
            updated[*field] = json!(peers);
        }
    }
    let mut included = peer_set(&updated, "include_peers", self_id)?;
    included.extend(peer_set(&updated, "pinned_peers", self_id)?);
    let excluded = peer_set(&updated, "exclude_peers", self_id)?;
    ensure!(
        !included.is_empty()
            || ["contacts", "non_contacts", "groups", "broadcasts", "bots"]
                .iter()
                .any(|f| updated[*f].as_bool() == Some(true)),
        "Folder needs at least one included peer or inclusion rule"
    );
    ensure!(
        included.is_disjoint(&excluded),
        "Included/pinned and excluded folder peers must not overlap"
    );
    let after = plan::state(&updated, self_id)?;
    if before["revision"] == after["revision"] {
        return Ok(
            json!({"success":true,"folder_id":fid,"changed":false,"revision":before["revision"]}),
        );
    }
    let settings = plan::limits(api).await;
    if let Some(max) = settings["limits"]["chats_per_folder"].as_u64() {
        ensure!(
            included.len() as u64 <= max && excluded.len() as u64 <= max,
            "Explicit peer count exceeds Telegram's per-folder limit"
        );
    }
    if let Some(max) = settings["limits"]["pinned_per_folder"].as_u64() {
        ensure!(
            updated["pinned_peers"]
                .as_array()
                .context("Pinned peer list is missing")?
                .len() as u64
                <= max,
            "Pinned peer count exceeds Telegram's per-folder limit"
        );
    }
    let latest = api.invoke("messages.getDialogFilters", json!({})).await?;
    let latest = plan::list(&latest)?
        .iter()
        .find(|f| id(&f["id"]).ok() == Some(fid))
        .context("Folder changed while preparing the update; get a fresh snapshot")?;
    ensure!(
        latest["_"] == "dialogFilter"
            && plan::state(latest, self_id)?["revision"] == before["revision"],
        "Folder changed while preparing the update; get a fresh snapshot"
    );
    let answer = api
        .invoke(
            "messages.updateDialogFilter",
            json!({"id":fid,"filter":updated}),
        )
        .await?;
    ensure!(answer != json!(false), "Telegram refused the folder update");
    Ok(
        json!({"success":true,"folder_id":fid,"changed":true,"revision":after["revision"],"concurrency":"Optimistic checks only; Telegram offers no atomic compare-and-swap."}),
    )
}
