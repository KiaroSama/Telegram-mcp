use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub const NAMES: &[&str] = &[
    "list_my_communities",
    "get_community_chats",
    "create_community",
    "rename_community",
    "set_community_photo",
    "delete_community_photo",
    "set_community_who_can_add",
    "delete_community",
    "add_chat_to_community",
    "remove_chat_from_community",
    "get_community_link_requests",
    "approve_community_link_request",
    "reject_community_link_request",
    "ban_community_member",
    "unban_community_member",
];

pub(super) async fn resolve(api: &dyn Telegram, value: &Value) -> Result<(Value, Value)> {
    let raw = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    let trimmed = raw.trim();
    let bare = trimmed.strip_prefix("-100").unwrap_or(trimmed);
    let id: i64 = bare
        .parse()
        .context("Not a community id; list_my_communities shows joined communities")?;
    ensure!(id > 0, "Community id must be positive");
    let joined = api
        .invoke("communities.getJoinedCommunities", json!({}))
        .await?;
    let community = joined
        .get("chats")
        .and_then(Value::as_array)
        .and_then(|xs| {
            xs.iter().find(|c| {
                kind(c) == "community" && int_value(&c["id"], "community id").ok() == Some(id)
            })
        })
        .cloned()
        .context("This community is not among the account's communities; no change was sent")?;
    let input = json!({"_":"inputChannel","channel_id":community["id"],"access_hash":req(&community,"access_hash")?});
    Ok((community, input))
}
pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    if name == "list_my_communities" {
        return api
            .invoke("communities.getJoinedCommunities", json!({}))
            .await;
    }
    if name == "create_community" {
        let title = text(a, "title")?.trim();
        ensure!(!title.is_empty(), "title must not be empty");
        let p = peer(api, a, "first_chat").await?;
        let mut request = json!({"title":title,"peer":p,"hidden":boolean(a,"hidden",false)?});
        copy_present(a, &mut request, &["about"]);
        return api.invoke("communities.create", request).await;
    }
    if name == "add_chat_to_community" {
        ensure!(
            ["visible", "hidden"].contains(&text(a, "visibility")?),
            "visibility must be visible or hidden; this choice is permanent"
        );
    }
    if name == "set_community_who_can_add" {
        ensure!(
            ["admins", "all", "only admins", "all members"].contains(&text(a, "who")?),
            "who must be admins or all"
        );
    }
    if name == "approve_community_link_request" || name == "reject_community_link_request" {
        let one = a.get("chat").is_some_and(|v| !v.is_null());
        let all = boolean(a, "all", false)?;
        ensure!(
            one != all,
            "Name exactly one target: a chat, or all=true for every pending request"
        );
    }
    let (info, c) = resolve(api, req(a, "community")?).await?;
    match name {
        "get_community_chats" => {
            api.invoke("channels.getFullChannel", json!({"channel":c}))
                .await
        }
        "rename_community" => {
            let title = text(a, "title")?.trim();
            ensure!(!title.is_empty(), "title must not be empty");
            api.invoke("channels.editTitle", json!({"channel":c,"title":title}))
                .await
        }
        "set_community_photo" | "delete_community_photo" => {
            let photo = if name == "delete_community_photo" {
                json!({"_":"inputChatPhotoEmpty"})
            } else {
                json!({"_":"inputChatUploadedPhoto","file":api.upload(Path::new(text(a,"file_path")?)).await?})
            };
            api.invoke("channels.editPhoto", json!({"channel":c,"photo":photo}))
                .await
        }
        "set_community_who_can_add" => {
            let who = text(a, "who")?;
            let mut rights = info
                .get("default_banned_rights")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| json!({"_":"chatBannedRights","until_date":0}));
            rights["manage_linked_peers"] = json!(who == "admins" || who == "only admins");
            api.invoke("messages.editChatDefaultBannedRights",json!({"peer":{"_":"inputPeerChannel","channel_id":c["channel_id"],"access_hash":c["access_hash"]},"banned_rights":rights})).await
        }
        "delete_community" => {
            api.invoke("channels.deleteChannel", json!({"channel":c}))
                .await
        }
        "add_chat_to_community" | "remove_chat_from_community" => {
            let p = peer(api, a, "chat").await?;
            let mut request = json!({"community":c,"peer":p});
            request[if name == "remove_chat_from_community" {
                "deleted"
            } else {
                text(a, "visibility")?
            }] = true.into();
            api.invoke("communities.togglePeerLink", request).await
        }
        "get_community_link_requests" => {
            api.invoke(
                "communities.getPeerLinkRequests",
                json!({"community":c,"offset":"","limit":limit(a,"limit",50,100)?}),
            )
            .await
        }
        "approve_community_link_request" | "reject_community_link_request" => {
            let reject = name == "reject_community_link_request";
            if boolean(a, "all", false)? {
                api.invoke(
                    "communities.toggleAllPeerLinkRequestApproval",
                    json!({"community":c,"reject":reject}),
                )
                .await
            } else {
                let p = peer(api, a, "chat").await?;
                api.invoke(
                    "communities.togglePeerLinkRequestApproval",
                    json!({"community":c,"peer":p,"reject":reject}),
                )
                .await
            }
        }
        "unban_community_member" => {
            let p = peer(api, a, "user").await?;
            user(&p)?;
            api.invoke(
                "communities.toggleParticipantBanned",
                json!({"community":c,"participant":p,"unban":true}),
            )
            .await
        }
        "ban_community_member" => {
            let p = peer(api, a, "user").await?;
            user(&p)?;
            let also = boolean(a, "also_from_chats", false)?;
            let joined = if also {
                api.invoke(
                    "communities.getParticipantJoinedChats",
                    json!({"community":c,"participant":p}),
                )
                .await?
            } else {
                json!({})
            };
            let banned = api
                .invoke(
                    "communities.toggleParticipantBanned",
                    json!({"community":c,"participant":p}),
                )
                .await?;
            if banned == false {
                return Ok(json!({"banned":false,"removed_from_chats":[]}));
            }
            let ids = joined
                .get("joined_chat_ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut removals = Vec::new();
            for chat in joined
                .get("chats")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|chat| ids.contains(&chat["id"]))
            {
                let result = if kind(chat) == "channel" {
                    api.invoke("channels.editBanned",json!({"channel":{"_":"inputChannel","channel_id":chat["id"],"access_hash":req(chat,"access_hash")?},"participant":p,"banned_rights":{"_":"chatBannedRights","until_date":0,"view_messages":true}})).await
                } else {
                    ensure!(
                        kind(chat) == "chat",
                        "Unknown joined chat type; remaining removals were not attempted"
                    );
                    api.invoke(
                        "messages.deleteChatUser",
                        json!({"chat_id":chat["id"],"user_id":user(&p)?}),
                    )
                    .await
                };
                removals.push(json!({"chat_id":chat["id"],"removed":result.is_ok()}));
            }
            Ok(json!({"banned":true,"removed_from_chats":removals,"also_from_chats":also}))
        }
        _ => unreachable!(),
    }
}
