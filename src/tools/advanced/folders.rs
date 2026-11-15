use super::common::*;
use crate::{args as a, backend::Telegram};
use anyhow::{Result, bail, ensure};
use serde_json::{Value, json};
use std::sync::OnceLock;

// Serialize this process's edits. Telegram offers no compare-and-swap, so
// optimistic rereads cannot eliminate races with other Telegram clients.
pub(super) static FOLDER_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
pub async fn call(api: &dyn Telegram, name: &str, args: &Value) -> Option<Result<Value>> {
    if !matches!(
        name,
        "list_folders"
            | "get_folder"
            | "create_folder"
            | "add_chat_to_folder"
            | "remove_chat_from_folder"
            | "delete_folder"
            | "reorder_folders"
            | "get_chatlist_invite"
            | "reorder_pinned_chats"
    ) {
        return None;
    }
    Some(run(api, name, args).await)
}
fn filters(result: &Value) -> Vec<Value> {
    result["filters"].as_array().cloned().unwrap_or_default()
}
fn peer_key(p: &Value) -> String {
    format!(
        "{}:{}",
        p["_"],
        p.get("user_id")
            .or_else(|| p.get("channel_id"))
            .or_else(|| p.get("chat_id"))
            .unwrap_or(&Value::Null)
    )
}
async fn run(api: &dyn Telegram, name: &str, args: &Value) -> Result<Value> {
    if name == "get_chatlist_invite" {
        return api
            .invoke(
                "chatlists.checkChatlistInvite",
                json!({"slug":slug(a::text(args,"link")?,"addlist")?}),
            )
            .await;
    }
    if name == "reorder_pinned_chats" {
        let order = a::required(args, "order")?
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("order must be a list"))?;
        ensure!(order.len() <= 100, "At most 100 pinned chats are allowed");
        let mut inputs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for id in order {
            let p = api.resolve_peer(id).await?;
            ensure!(seen.insert(peer_key(&p)), "Duplicate pinned chat");
            inputs.push(json!({"_":"inputDialogPeer","peer":p}));
        }
        let folder = a::integer(args, "folder_id", 0)?;
        ensure!(folder >= 0, "folder_id must be nonnegative");
        return api
            .invoke(
                "messages.reorderPinnedDialogs",
                json!({"folder_id":folder,"order":inputs,"force":a::boolean(args,"force",false)?}),
            )
            .await;
    }
    let _guard = FOLDER_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let result = api.invoke("messages.getDialogFilters", json!({})).await?;
    let list = filters(&result);
    if name == "list_folders" {
        return Ok(result);
    }
    if name == "create_folder" {
        let title = a::text(args, "title")?;
        nonempty(title, 12, "Folder title")?;
        let existing: std::collections::HashSet<_> =
            list.iter().filter_map(|v| id(&v["id"]).ok()).collect();
        let new_id = (2..=i32::MAX as i64)
            .find(|n| !existing.contains(n))
            .ok_or_else(|| anyhow::anyhow!("No available folder ID"))?;
        let settings = super::folder_plan::limits(api).await;
        if let Some(max) = settings["limits"]["folders"].as_u64() {
            ensure!(
                (list
                    .iter()
                    .filter(|f| f["_"] != "dialogFilterDefault")
                    .count() as u64)
                    < max,
                "Cannot create folder: the account has reached its configured folder limit"
            );
        }
        let mut peers = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Some(chats) = args.get("chat_ids").filter(|v| !v.is_null()) {
            let chats = chats
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("chat_ids must be a list"))?;
            ensure!(chats.len() <= 200, "At most 200 explicit chats per folder");
            for chat in chats {
                let p = api.resolve_peer(chat).await?;
                if seen.insert(peer_key(&p)) {
                    peers.push(p);
                }
            }
        }
        let mut f = json!({"_":"dialogFilter","id":new_id,"title":txt(title),"pinned_peers":[],"include_peers":peers,"exclude_peers":[]});
        for field in [
            "contacts",
            "non_contacts",
            "groups",
            "broadcasts",
            "bots",
            "exclude_muted",
            "exclude_read",
            "exclude_archived",
        ] {
            f[field] = json!(a::boolean(args, field, field == "exclude_archived")?);
        }
        copy_optional(args, &mut f, &["emoticon"]);
        let latest = api.invoke("messages.getDialogFilters", json!({})).await?;
        ensure!(
            !super::folder_plan::list(&latest)?
                .iter()
                .any(|f| id(&f["id"]).ok() == Some(new_id)),
            "Allocated folder ID was occupied by another client; list folders and retry creation"
        );
        api.invoke(
            "messages.updateDialogFilter",
            json!({"id":new_id,"filter":f}),
        )
        .await?;
        return Ok(
            json!({"success":true,"folder_id":new_id,"title":title,"concurrency":"Optimistic ID check only; Telegram offers no atomic create-if-absent."}),
        );
    }
    if name == "reorder_folders" {
        let raw = a::required(args, "folder_ids")?
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("folder_ids must be a list"))?;
        ensure!(
            !raw.is_empty() && raw.len() <= 100,
            "Expected 1..=100 folder IDs"
        );
        let order = raw.iter().map(id).collect::<Result<Vec<_>>>()?;
        ensure!(
            order.iter().all(|id| (0..=i32::MAX as i64).contains(id)),
            "Folder IDs must be nonnegative 32-bit integers"
        );
        unique(&order)?;
        let existing: std::collections::BTreeSet<_> = list
            .iter()
            .filter_map(|v| {
                if v["_"] == "dialogFilterDefault" {
                    Some(0)
                } else {
                    id(&v["id"]).ok()
                }
            })
            .collect();
        ensure!(
            order
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                == existing,
            "Every existing folder ID must appear exactly once"
        );
        return api
            .invoke("messages.updateDialogFiltersOrder", json!({"order":order}))
            .await;
    }
    let fid = a::integer(args, "folder_id", 0)?;
    let target = list.iter().find(|f| id(&f["id"]).ok() == Some(fid));
    if name == "get_folder" {
        return target
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Folder not found; use list_folders"));
    }
    ensure!(fid >= 1, "Invalid folder ID");
    if name == "delete_folder" {
        ensure!(fid >= 2, "System folders cannot be deleted");
        if target.is_none() {
            return Ok(json!({"deleted":false,"already_absent":true,"folder_id":fid}));
        }
        api.invoke("messages.updateDialogFilter", json!({"id":fid}))
            .await?;
        return Ok(json!({"deleted":true,"folder_id":fid,"chats_preserved":true}));
    }
    let mut f = target
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Folder not found; use list_folders"))?;
    ensure!(
        f["_"] == "dialogFilter",
        "Shared and system folder membership cannot be edited with this tool"
    );
    let mut p = a::peer(api, args, "chat_id").await?;
    if p["_"] == "inputPeerSelf"
        || ["include_peers", "pinned_peers", "exclude_peers"]
            .iter()
            .any(|k| {
                f[*k]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|p| p["_"] == "inputPeerSelf"))
            })
    {
        let me = api
            .invoke("users.getUsers", json!({"id":[{"_":"inputUserSelf"}]}))
            .await?;
        let user = me
            .as_array()
            .and_then(|v| v.first())
            .ok_or_else(|| anyhow::anyhow!("Account identity unavailable"))?;
        let concrete = json!({"_":"inputPeerUser","user_id":user["id"],"access_hash":user.get("access_hash").cloned().unwrap_or(json!("0"))});
        if p["_"] == "inputPeerSelf" {
            p = concrete.clone();
        }
        for key in ["include_peers", "pinned_peers", "exclude_peers"] {
            if let Some(values) = f[key].as_array_mut() {
                for stored in values {
                    if stored["_"] == "inputPeerSelf" {
                        *stored = concrete.clone();
                    }
                }
            }
        }
    }
    let key = peer_key(&p);
    let mut changed = false;
    for field in ["include_peers", "pinned_peers"] {
        let mut peers = f[field].as_array().cloned().unwrap_or_default();
        if name == "remove_chat_from_folder" {
            let old = peers.len();
            peers.retain(|v| peer_key(v) != key);
            changed |= old != peers.len();
        } else if name == "add_chat_to_folder" {
            if (field == "include_peers" || a::boolean(args, "pinned", false)?)
                && !peers.iter().any(|v| peer_key(v) == key)
            {
                peers.push(p.clone());
                changed = true;
            }
        } else {
            bail!("Unknown folder operation");
        }
        f[field] = json!(peers);
    }
    let mut excluded = f["exclude_peers"].as_array().cloned().unwrap_or_default();
    if name == "remove_chat_from_folder" {
        if !excluded.iter().any(|v| peer_key(v) == key) {
            excluded.push(p);
            changed = true;
        }
    } else {
        let before = excluded.len();
        excluded.retain(|v| peer_key(v) != key);
        changed |= before != excluded.len();
    }
    f["exclude_peers"] = json!(excluded);
    if changed {
        api.invoke("messages.updateDialogFilter", json!({"id":fid,"filter":f}))
            .await?;
    }
    Ok(json!({"changed":changed,"folder_id":fid}))
}
