mod search;

use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub const NAMES: &[&str] = &[
    "get_chats",
    "list_chats",
    "get_chat",
    "search_public_chats",
    "resolve_username",
    "get_full_chat",
    "get_common_chats",
    "get_message_link",
    "search_my_chats",
    "pin_chat",
    "unpin_chat",
    "mark_chat_unread",
    "subscribe_public_channel",
    "archive_chat",
    "unarchive_chat",
    "clear_chat_history",
    "delete_chat",
    "lookup_peer",
];

fn peer_key(p: &Value) -> String {
    format!(
        "{}:{}",
        kind(p).replace("inputPeer", "peer"),
        entity_id(p).unwrap_or(Value::Null)
    )
}
fn matching_entity<'a>(r: &'a Value, p: &Value) -> Option<&'a Value> {
    let key = if kind(p).contains("User") {
        "users"
    } else {
        "chats"
    };
    let id = entity_id(p).ok()?;
    r.get(key)?.as_array()?.iter().find(|e| e["id"] == id)
}
pub(super) fn marked(p: &Value) -> Result<String> {
    let id = int_value(&entity_id(p)?, "peer id")?;
    Ok(if kind(p).contains("Channel") {
        format!("{}", -1_000_000_000_000_i64 - id)
    } else if kind(p).contains("Chat") {
        format!("{}", -id)
    } else {
        id.to_string()
    })
}
fn input_from_entity(e: &Value) -> Result<Value> {
    Ok(match kind(e) {
        "user" => {
            json!({"_":"inputPeerUser","user_id":req(e,"id")?,"access_hash":req(e,"access_hash")?})
        }
        "channel" => {
            json!({"_":"inputPeerChannel","channel_id":req(e,"id")?,"access_hash":req(e,"access_hash")?})
        }
        "chat" => json!({"_":"inputPeerChat","chat_id":req(e,"id")?}),
        _ => bail!("Unsupported Telegram peer type"),
    })
}
fn chat_kind(e: &Value) -> &str {
    if kind(e) == "user" {
        if e["bot"] == true { "bot" } else { "user" }
    } else if kind(e) == "chat" {
        "group"
    } else if e["megagroup"] == true {
        "supergroup"
    } else {
        "channel"
    }
}
fn muted(d: &Value) -> bool {
    int(&d["notify_settings"], "mute_until", 0).unwrap_or(0) > now().unwrap_or(i64::MAX)
}
fn summary(e: &Value, d: &Value, p: &Value) -> Result<Value> {
    Ok(
        json!({"id":marked(p)?,"chat_id":marked(p)?,"name":display_name(e),"type":chat_kind(e),"username":e.get("username"),"unread":d.get("unread_count").unwrap_or(&Value::Null),"unread_mark":d["unread_mark"]==true,"archived":d["folder_id"]==1,"pinned":d["pinned"]==true,"muted":muted(d),"silent":d["notify_settings"]["silent"]==true}),
    )
}
async fn dialogs(api: &dyn Telegram, count: i64, folder: Option<i64>) -> Result<Value> {
    let mut p = json!({"offset_date":0,"offset_id":0,"offset_peer":{"_":"inputPeerEmpty"},"limit":count,"hash":0});
    if let Some(f) = folder {
        p["folder_id"] = f.into();
    }
    api.invoke("messages.getDialogs", p).await
}
async fn filters(api: &dyn Telegram) -> Result<Vec<Value>> {
    let r = api.invoke("messages.getDialogFilters", json!({})).await?;
    Ok(r.get("filters")
        .unwrap_or(&r)
        .as_array()
        .context("Telegram returned no folder list")?
        .clone())
}
fn filter_title(f: &Value) -> &str {
    f["title"]
        .as_str()
        .or_else(|| f["title"]["text"].as_str())
        .unwrap_or("")
}
fn find_filter<'a>(items: &'a [Value], name: &str) -> Result<&'a Value> {
    let found = items
        .iter()
        .filter(|f| filter_title(f).to_lowercase() == name.to_lowercase() || f["id"] == name)
        .collect::<Vec<_>>();
    ensure!(
        found.len() == 1,
        "Folder title must match exactly one folder"
    );
    Ok(found[0])
}
fn in_folder(f: &Value, e: &Value, d: &Value, p: &Value) -> bool {
    let named = |key: &str| {
        f.get(key)
            .and_then(Value::as_array)
            .is_some_and(|xs| xs.iter().any(|v| peer_key(v) == peer_key(p)))
    };
    if named("pinned_peers") || named("include_peers") {
        return true;
    }
    if kind(f) == "dialogFilterChatlist" || named("exclude_peers") {
        return false;
    }
    let rule = match chat_kind(e) {
        "bot" => "bots",
        "user" if e["contact"] == true => "contacts",
        "user" => "non_contacts",
        "channel" => "broadcasts",
        _ => "groups",
    };
    f[rule] == true
        && !(f["exclude_archived"] == true && d["folder_id"] == 1)
        && !(f["exclude_muted"] == true && muted(d))
        && !(f["exclude_read"] == true
            && int(d, "unread_count", 0).unwrap_or(0) == 0
            && d["unread_mark"] != true)
}

pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    match name {
        "get_chats" | "list_chats" => {
            let page = int(a, "page", 1)?;
            ensure!(page > 0, "page must be positive");
            let size = limit(
                a,
                if name == "get_chats" {
                    "page_size"
                } else {
                    "limit"
                },
                20,
                200,
            )?;
            let offset = if name == "get_chats" {
                (page - 1)
                    .checked_mul(size)
                    .context("page offset overflow")?
            } else {
                0
            };
            ensure!(
                offset < 100_000,
                "Paging past 100000 records is not supported; use search instead"
            );
            // Traverse in fixed chunks; a large page never requests an unbounded server response.
            let mut request = json!({"offset_date":0,"offset_id":0,"offset_peer":{"_":"inputPeerEmpty"},"limit":100,"hash":0});
            let archived = a
                .get("archived")
                .filter(|v| !v.is_null())
                .map(|v| v.as_bool().context("archived must be boolean"))
                .transpose()?;
            if let Some(archive) = archived {
                request["folder_id"] = json!(if archive { 1 } else { 0 });
            }
            let mut consumed = 0_i64;
            let mut rows = Vec::new();
            let mut has_more = false;
            for _ in 0..1002 {
                request["limit"] = json!((offset + size - consumed).min(100));
                let result = api.invoke("messages.getDialogs", request.clone()).await?;
                let ds = result
                    .get("dialogs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                has_more = ds.len() as i64 >= int(&request, "limit", 100)?;
                for d in &ds {
                    if consumed >= offset
                        && let Some(e) = matching_entity(&result, &d["peer"])
                    {
                        let k = chat_kind(e);
                        let wanted = string(a, "chat_type", "")?;
                        let type_ok = wanted.is_empty()
                            || wanted == k
                            || (wanted == "group" && k == "supergroup")
                            || (wanted == "user" && k == "bot");
                        let unread = int(d, "unread_count", 0)? > 0 || d["unread_mark"] == true;
                        if type_ok
                            && (!boolean(a, "unread_only", false)? || unread)
                            && (!boolean(a, "unmuted_only", false)? || !muted(d))
                            && archived.is_none_or(|archive| (d["folder_id"] == 1) == archive)
                        {
                            let mut row = summary(e, d, &d["peer"])?;
                            if boolean(a, "with_about", false)? {
                                let about = full(api, &input_from_entity(e)?).await;
                                row["about"] = about
                                    .as_ref()
                                    .ok()
                                    .and_then(|r| {
                                        r["full_chat"]
                                            .get("about")
                                            .or_else(|| r["full_user"].get("about"))
                                    })
                                    .cloned()
                                    .unwrap_or(json!("<error fetching description>"));
                            }
                            rows.push(row);
                        }
                    }
                    consumed += 1;
                    if consumed >= offset + size {
                        break;
                    }
                }
                if consumed >= offset + size || ds.is_empty() || !has_more {
                    break;
                }
                let last = ds.last().context("Missing dialog cursor")?;
                let e = matching_entity(&result, &last["peer"])
                    .context("Missing dialog cursor entity")?;
                request["offset_peer"] = input_from_entity(e)?;
                request["offset_id"] = last["top_message"].clone();
                request["offset_date"] = result
                    .get("messages")
                    .and_then(Value::as_array)
                    .and_then(|xs| {
                        xs.iter().find(|m| {
                            m["id"] == last["top_message"]
                                && peer_key(&m["peer_id"]) == peer_key(&last["peer"])
                        })
                    })
                    .and_then(|m| m.get("date"))
                    .cloned()
                    .unwrap_or(json!(0));
                request["exclude_pinned"] = true.into();
            }
            Ok(
                json!({"results":rows,"metadata":{"page":page,"page_size":size,"fetched":(consumed-offset).max(0),"has_more":has_more},"untrusted_content":true}),
            )
        }
        "get_chat" => {
            let p = peer(api, a, "chat_id").await?;
            api.invoke(
                "messages.getPeerDialogs",
                json!({"peers":[{"_":"inputDialogPeer","peer":p}]}),
            )
            .await
        }
        "get_full_chat" => {
            let p = peer(api, a, "chat_id").await?;
            full(api, &p).await
        }
        "lookup_peer" => search::lookup(api, a).await,
        "resolve_username" => {
            let raw = text(a, "username")?.trim().trim_start_matches('@');
            ensure!(!raw.is_empty(), "username must not be empty");
            api.invoke("contacts.resolveUsername", json!({"username":raw}))
                .await
        }
        "search_public_chats" => {
            let k = string(a, "kind", "all")?;
            ensure!(
                ["all", "channels", "bots"].contains(&k.as_str()),
                "kind must be all, channels or bots"
            );
            let mut p = json!({"q":text(a,"query")?,"limit":limit(a,"limit",20,100)?});
            if k == "channels" {
                p["broadcasts"] = true.into();
            }
            if k == "bots" {
                p["bots"] = true.into();
            }
            api.invoke("contacts.search", p).await
        }
        "get_common_chats" => {
            let p = peer(api, a, "user_id").await?;
            api.invoke("messages.getCommonChats",json!({"user_id":user(&p)?,"limit":limit(a,"limit",100,100)?,"max_id":int(a,"max_id",0)?})).await
        }
        "get_message_link" => {
            let message = int(a, "message_id", 0)?;
            ensure!(message > 0, "message_id must be positive");
            let p = peer(api, a, "chat_id").await?;
            api.invoke("channels.exportMessageLink",json!({"channel":channel(&p)?,"id":message,"thread":boolean(a,"thread",false)?,"grouped":false})).await
        }
        "search_my_chats" => search::mine(api, a).await,
        "pin_chat" | "unpin_chat" => {
            let folder = folder_selector(a)?;
            let p = peer(api, a, "chat").await?;
            let pinned = name == "pin_chat";
            let one = api
                .invoke(
                    "messages.getPeerDialogs",
                    json!({"peers":[{"_":"inputDialogPeer","peer":p}]}),
                )
                .await?;
            ensure!(
                one.get("dialogs")
                    .and_then(Value::as_array)
                    .is_some_and(|ds| !ds.is_empty()),
                "That chat is not in your chat list"
            );
            if let Some(folder) = folder {
                let fs = filters(api).await?;
                let mut f = find_filter(&fs, folder)?.clone();
                let mut pins = f
                    .get("pinned_peers")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                pins.retain(|v| peer_key(v) != peer_key(&p));
                if pinned {
                    pins.insert(0, p.clone());
                }
                f["pinned_peers"] = pins.into();
                api.invoke(
                    "messages.updateDialogFilter",
                    json!({"id":f["id"],"filter":f}),
                )
                .await
            } else {
                api.invoke(
                    "messages.toggleDialogPin",
                    json!({"peer":{"_":"inputDialogPeer","peer":p},"pinned":pinned}),
                )
                .await
            }
        }
        "mark_chat_unread" => {
            let p = peer(api, a, "chat").await?;
            api.invoke(
                "messages.markDialogUnread",
                json!({"peer":{"_":"inputDialogPeer","peer":p},"unread":true}),
            )
            .await
        }
        "subscribe_public_channel" => {
            let p = peer(api, a, "channel").await?;
            api.invoke("channels.joinChannel", json!({"channel":channel(&p)?}))
                .await
        }
        "archive_chat" | "unarchive_chat" => {
            let p = peer(api, a, "chat_id").await?;
            api.invoke("folders.editPeerFolders",json!({"folder_peers":[{"_":"inputFolderPeer","peer":p,"folder_id":if name=="archive_chat"{1}else{0}}]})).await
        }
        "clear_chat_history" | "delete_chat" => remove(api, name, a).await,
        _ => unreachable!(),
    }
}

fn folder_selector(a: &Value) -> Result<Option<&str>> {
    a.get("folder")
        .filter(|value| !value.is_null())
        .map(|value| value.as_str().context("folder must be a string"))
        .transpose()
}

async fn remove(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let both = req(a, "both_sides")?
        .as_bool()
        .context("both_sides must be an explicit boolean")?;
    let p = peer(api, a, "chat").await?;
    ensure!(
        !both || kind(&p) == "inputPeerUser",
        "Both sides exists only for a private chat or bot"
    );
    if is_channel(&p) {
        if name == "delete_chat" {
            return api
                .invoke("channels.leaveChannel", json!({"channel":channel(&p)?}))
                .await;
        }
        let detail = full(api, &p).await?;
        let id = entity_id(&p)?;
        let group = detail
            .get("chats")
            .and_then(Value::as_array)
            .and_then(|xs| xs.iter().find(|e| e["id"] == id));
        ensure!(
            group.is_some_and(|g| g["megagroup"] == true),
            "Telegram offers no history clearing for a broadcast channel"
        );
        return api
            .invoke(
                "channels.deleteHistory",
                json!({"channel":channel(&p)?,"max_id":0}),
            )
            .await;
    }
    if kind(&p) == "inputPeerChat" && name == "delete_chat" {
        let leave = api
            .invoke(
                "messages.deleteChatUser",
                json!({"chat_id":entity_id(&p)?,"user_id":{"_":"inputUserSelf"}}),
            )
            .await;
        if let Err(error) = leave {
            let already_left = error.chain().any(|cause| {
                let message = cause.to_string();
                let Some(rpc) = message.strip_prefix("Telegram RPC ") else {
                    return false;
                };
                let mut fields = rpc.split_whitespace();
                fields.next() == Some("400") && fields.next() == Some("USER_NOT_PARTICIPANT")
            });
            if !already_left {
                return Err(error);
            }
        }
    }
    for pass in 1..=50 {
        let r = api
            .invoke(
                "messages.deleteHistory",
                json!({"peer":p,"max_id":0,"just_clear":name=="clear_chat_history","revoke":both}),
            )
            .await?;
        if int(&r, "offset", 0)? <= 0 {
            return Ok(
                json!({"complete":true,"passes":pass,"both_sides":both,"chat_retained":name=="clear_chat_history"}),
            );
        }
    }
    Ok(
        json!({"complete":false,"passes":50,"reason":"Telegram still reports messages remaining; do not assume the history was fully removed"}),
    )
}
