mod participants;

use super::base::*;
use crate::backend::Telegram;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub const NAMES: &[&str] = &[
    "create_group",
    "create_channel",
    "edit_chat_title",
    "edit_chat_photo",
    "replace_chat_photo",
    "edit_chat_about",
    "delete_chat_photo",
    "toggle_slow_mode",
    "leave_chat",
    "invite_to_group",
    "get_participants",
    "set_member_tag",
    "ban_user",
    "unban_user",
    "get_banned_users",
    "get_recent_actions",
    "kick_user",
    "remove_user",
    "get_admins",
    "get_member_admin_status",
];

async fn chat_photo(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let p = peer(api, a, "chat_id").await?;
    ensure!(
        is_channel(&p) || kind(&p) == "inputPeerChat",
        "Chat photos require a group or channel"
    );
    let mut old = Vec::new();
    if name == "replace_chat_photo" {
        let r=api.invoke("messages.search",json!({"peer":p,"q":"","filter":{"_":"inputMessagesFilterChatPhotos"},"min_date":0,"max_date":0,"offset_id":0,"add_offset":0,"limit":100,"max_id":0,"min_id":0,"hash":0})).await?;
        old = r
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| kind(&m["action"]) == "messageActionChatEditPhoto")
            .filter_map(|m| m.get("id").cloned())
            .collect();
    }
    if name == "replace_chat_photo" && a.get("file_path").is_none_or(Value::is_null) {
        if !old.is_empty() {
            old.remove(0);
        }
        if old.is_empty() {
            return Ok(json!({"current_photo_retained":true,"old_photos_deleted":0}));
        }
        let count = old.len();
        let result = if is_channel(&p) {
            api.invoke(
                "channels.deleteMessages",
                json!({"channel":channel(&p)?,"id":old}),
            )
            .await?
        } else {
            api.invoke("messages.deleteMessages", json!({"id":old,"revoke":true}))
                .await?
        };
        return Ok(
            json!({"current_photo_retained":true,"old_photos_deleted":count,"result":result}),
        );
    }
    let photo = if name == "delete_chat_photo" {
        json!({"_":"inputChatPhotoEmpty"})
    } else {
        let path = text(a, "file_path")?;
        let upload = api.upload(Path::new(path)).await?;
        json!({"_":"inputChatUploadedPhoto","file":upload})
    };
    let changed = if is_channel(&p) {
        api.invoke(
            "channels.editPhoto",
            json!({"channel":channel(&p)?,"photo":photo}),
        )
        .await?
    } else {
        api.invoke(
            "messages.editChatPhoto",
            json!({"chat_id":entity_id(&p)?,"photo":photo}),
        )
        .await?
    };
    if !old.is_empty() {
        let cleanup = if is_channel(&p) {
            api.invoke(
                "channels.deleteMessages",
                json!({"channel":channel(&p)?,"id":old}),
            )
            .await
        } else {
            api.invoke("messages.deleteMessages", json!({"id":old,"revoke":true}))
                .await
        };
        // A failed service-message cleanup must never invite a second photo upload.
        return Ok(
            json!({"photo_changed":true,"history_removed":cleanup.is_ok(),"result":changed}),
        );
    }
    Ok(changed)
}

pub async fn run(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    match name {
        "create_group" => {
            let title = text(a, "title")?.trim();
            ensure!(!title.is_empty(), "title must not be empty");
            let entries = array(a, "user_ids")?;
            ensure!(
                !entries.is_empty(),
                "A basic group needs at least one other member"
            );
            api.invoke(
                "messages.createChat",
                json!({"title":title,"users":users(api,entries).await?}),
            )
            .await
        }
        "create_channel" => create_channel(api, a).await,
        "edit_chat_title" => {
            let title = text(a, "title")?.trim();
            ensure!(!title.is_empty(), "title must not be empty");
            let p = peer(api, a, "chat_id").await?;
            if is_channel(&p) {
                api.invoke(
                    "channels.editTitle",
                    json!({"channel":channel(&p)?,"title":title}),
                )
                .await
            } else {
                ensure!(
                    kind(&p) == "inputPeerChat",
                    "Target must be a group or channel"
                );
                api.invoke(
                    "messages.editChatTitle",
                    json!({"chat_id":entity_id(&p)?,"title":title}),
                )
                .await
            }
        }
        "edit_chat_photo" | "replace_chat_photo" | "delete_chat_photo" => {
            chat_photo(api, name, a).await
        }
        "edit_chat_about" => {
            let about = text(a, "about")?;
            let p = peer(api, a, "chat_id").await?;
            api.invoke("messages.editChatAbout", json!({"peer":p,"about":about}))
                .await
        }
        "toggle_slow_mode" => {
            let seconds = int(a, "seconds", 0)?;
            ensure!(
                [0, 10, 30, 60, 300, 900, 3600].contains(&seconds),
                "seconds must be 0, 10, 30, 60, 300, 900 or 3600"
            );
            let p = peer(api, a, "chat_id").await?;
            api.invoke(
                "channels.toggleSlowMode",
                json!({"channel":channel(&p)?,"seconds":seconds}),
            )
            .await
        }
        "leave_chat" => {
            let p = peer(api, a, "chat_id").await?;
            if is_channel(&p) {
                api.invoke("channels.leaveChannel", json!({"channel":channel(&p)?}))
                    .await
            } else {
                ensure!(
                    kind(&p) == "inputPeerChat",
                    "Target must be a group or channel"
                );
                api.invoke(
                    "messages.deleteChatUser",
                    json!({"chat_id":entity_id(&p)?,"user_id":{"_":"inputUserSelf"}}),
                )
                .await
            }
        }
        "invite_to_group" => {
            let ids = array(a, "user_ids")?;
            ensure!(!ids.is_empty(), "user_ids must not be empty");
            let p = peer(api, a, "group_id").await?;
            let us = users(api, ids).await?;
            if is_channel(&p) {
                api.invoke(
                    "channels.inviteToChannel",
                    json!({"channel":channel(&p)?,"users":us}),
                )
                .await
            } else {
                ensure!(kind(&p) == "inputPeerChat", "Target must be a group");
                let mut results = Vec::new();
                for u in us {
                    match api.invoke("messages.addChatUser",json!({"chat_id":entity_id(&p)?,"user_id":u,"fwd_limit":100})).await {
                        Ok(r) => {
                            let id = int_value(&entity_id(&u)?, "invited user id")?;
                            let missing = r.get("missing_invitees").and_then(Value::as_array)
                                .into_iter().flatten().find(|entry| int_value(&entry["user_id"], "missing invitee id").ok() == Some(id));
                            let mut result = json!({"user":u,"success":missing.is_none(),"result":r});
                            if let Some(missing) = missing {
                                result["reason"] = "Telegram could not invite this user because of privacy or premium requirements".into();
                                result["missing_invitee"] = missing.clone();
                            }
                            results.push(result);
                        }
                        Err(_) => results.push(json!({"user":u,"success":false,"reason":"Telegram refused this invitation"})),
                    }
                }
                Ok(json!({"results":results}))
            }
        }
        "get_participants" | "get_banned_users" | "get_admins" => {
            participants::list(api, name, a).await
        }
        "get_member_admin_status" => participants::status(api, a).await,
        "set_member_tag" => {
            let tag = text(a, "tag")?;
            ensure!(
                tag.encode_utf16().count() <= 16,
                "tag must contain at most 16 UTF-16 code units"
            );
            let p = peer(api, a, "chat").await?;
            let member = if let Some(v) = a.get("user").filter(|v| !v.is_null()) {
                api.resolve_peer(v).await?
            } else {
                json!({"_":"inputPeerSelf"})
            };
            api.invoke(
                "messages.editChatParticipantRank",
                json!({"peer":p,"participant":member,"rank":tag}),
            )
            .await
        }
        "ban_user" | "unban_user" | "kick_user" | "remove_user" => {
            let p = peer(api, a, "chat_id").await?;
            let u = peer(api, a, "user_id").await?;
            let kicking = matches!(name, "kick_user" | "remove_user");
            if name == "remove_user" {
                ensure!(
                    kind(&u) != "inputPeerSelf",
                    "Use leave_chat to remove the current account"
                );
                let identities = api
                    .invoke("users.getUsers", json!({"id":[user(&u)?]}))
                    .await?;
                ensure!(
                    !identities
                        .as_array()
                        .is_some_and(|rows| rows.iter().any(|user| user["self"] == true)),
                    "Use leave_chat to remove the current account"
                );
                if is_channel(&p) {
                    let found = api
                        .invoke(
                            "channels.getParticipant",
                            json!({"channel":channel(&p)?,"participant":u}),
                        )
                        .await?;
                    let member = &found["participant"];
                    ensure!(
                        kind(member) != "channelParticipantLeft",
                        "The user is not a member of this chat"
                    );
                    ensure!(
                        !(kind(member) == "channelParticipantBanned"
                            && (member["left"] == true
                                || member["banned_rights"]["view_messages"] == true)),
                        "The user is already banned; use unban_user to let them back in"
                    );
                }
            }
            if !is_channel(&p) {
                ensure!(
                    kicking && kind(&p) == "inputPeerChat",
                    "Bans require a supergroup or channel"
                );
                return api
                    .invoke(
                        "messages.deleteChatUser",
                        json!({"chat_id":entity_id(&p)?,"user_id":user(&u)?}),
                    )
                    .await;
            }
            if kicking {
                let channel = channel(&p)?;
                if let Some(owned) = api.clone_handle() {
                    // Keep the native session alive and finish both steps even
                    // when the caller disconnects or its outer timeout fires.
                    return tokio::spawn(async move {
                        eject_and_clear(owned.as_ref(), channel, u).await
                    })
                    .await
                    .context("Member removal worker failed; inspect membership before retrying")?;
                }
                return eject_and_clear(api, channel, u).await;
            }
            let r=api.invoke("channels.editBanned",json!({"channel":channel(&p)?,"participant":u,"banned_rights":{"_":"chatBannedRights","until_date":0,"view_messages":name!="unban_user"}})).await?;
            Ok(r)
        }
        "get_recent_actions" => {
            let maximum = limit(a, "limit", 20, 100)?;
            let mut events = json!({"_":"channelAdminLogEventsFilter"});
            if let Some(types) = a.get("event_types").filter(|v| !v.is_null()) {
                for v in types.as_array().context("event_types must be an array")? {
                    let key = v.as_str().context("event_types must contain strings")?;
                    ensure!(
                        [
                            "join",
                            "leave",
                            "invite",
                            "ban",
                            "unban",
                            "kick",
                            "unkick",
                            "promote",
                            "demote",
                            "info",
                            "settings",
                            "pinned",
                            "edit",
                            "delete",
                            "group_call",
                            "invites",
                            "send",
                            "forums",
                            "sub_extend"
                        ]
                        .contains(&key),
                        "Unsupported admin-log event type: {key}"
                    );
                    events[key] = true.into();
                }
            }
            let p = peer(api, a, "chat_id").await?;
            let mut request = json!({"channel":channel(&p)?,"q":string(a,"query","")?,"max_id":int(a,"max_id",0)?,"min_id":0,"limit":maximum});
            if events.as_object().is_some_and(|e| e.len() > 1) {
                request["events_filter"] = events;
            }
            if let Some(admins) = a.get("admins").filter(|v| !v.is_null()) {
                request["admins"] =
                    json!(users(api, admins.as_array().context("admins must be an array")?).await?);
            }
            api.invoke("channels.getAdminLog", request).await
        }
        _ => unreachable!(),
    }
}

async fn eject_and_clear(api: &dyn Telegram, channel: Value, participant: Value) -> Result<Value> {
    use std::time::Duration;
    const RPC_BUDGET: Duration = Duration::from_secs(10);
    // Each phase is bounded. In particular, cleanup may outlive the caller
    // but cannot retain a session lease indefinitely on an unresponsive peer.
    let eject = tokio::time::timeout(RPC_BUDGET, api.invoke("channels.editBanned", json!({"channel":channel,"participant":participant,"banned_rights":{"_":"chatBannedRights","until_date":0,"view_messages":true}}))).await;
    let kicked = match eject {
        Ok(Ok(_)) => true,
        // A concrete RPC refusal is a known rejection. Transport, response-
        // decoding and persistence failures can all occur after the ban took
        // effect; these need the same bounded cleanup as an elapsed deadline.
        Ok(Err(error)) if known_rpc_refusal(&error) => {
            return Err(error);
        }
        Ok(Err(_)) => false,
        Err(_) => false,
    };
    let clear = tokio::time::timeout(RPC_BUDGET, api.invoke("channels.editBanned", json!({"channel":channel,"participant":participant,"banned_rights":{"_":"chatBannedRights","until_date":0}}))).await;
    match clear {
        Ok(Ok(result)) if kicked => Ok(json!({"kicked":true,"ban_lifted":true,"result":result})),
        Ok(Ok(result)) => Ok(
            json!({"kicked":null,"ban_lifted":null,"cleanup_accepted":true,"result":result,"warning":"The removal outcome is unknown after an ambiguous RPC outcome. Telegram accepted a cleanup request, but a delayed removal may still take effect. Inspect membership and use unban_user before retrying"}),
        ),
        other => {
            let rejected = matches!(other, Ok(Err(error)) if known_rpc_refusal(&error));
            Ok(
                json!({"kicked":if kicked {json!(true)} else {Value::Null},"ban_lifted":if rejected {json!(false)} else {Value::Null},"warning":if kicked {"The member was removed but ban cleanup was not verified; the member may remain banned. Use unban_user"} else {"Removal and cleanup outcomes are unknown; inspect membership and use unban_user before retrying"}}),
            )
        }
    }
}

fn known_rpc_refusal(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().starts_with("Telegram RPC "))
}

async fn create_channel(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let title = text(a, "title")?.trim();
    ensure!(!title.is_empty(), "title must not be empty");
    let wanted = a
        .get("username")
        .filter(|v| !v.is_null())
        .map(|v| username(v.as_str().context("username must be a string")?, false))
        .transpose()?;
    let mega = boolean(a, "megagroup", false)?;
    if let Some(wanted) = &wanted {
        let free = api
            .invoke(
                "channels.checkUsername",
                json!({"channel":{"_":"inputChannelEmpty"},"username":wanted}),
            )
            .await?;
        ensure!(
            free == true,
            "Username is already taken; nothing was created"
        );
    }
    let r = api
        .invoke(
            "channels.createChannel",
            json!({"title":title,"about":string(a,"about","")?,"megagroup":mega,"broadcast":!mega}),
        )
        .await?;
    let created=r.get("chats").and_then(Value::as_array).and_then(|xs|xs.iter().find(|e|kind(e)=="channel")).context("Telegram created the channel but returned no channel id; inspect recent chats before retrying")?;
    let p = json!({"_":"inputPeerChannel","channel_id":req(created,"id")?,"access_hash":req(created,"access_hash")?});
    let mut record = json!({"created":true,"chat_id":super::chats::marked(&p)?,"public":false,"kind":if mega{"supergroup"}else{"channel"}});
    if let Some(wanted) = wanted {
        match api
            .invoke(
                "channels.updateUsername",
                json!({"channel":channel(&p)?,"username":wanted}),
            )
            .await
        {
            Ok(changed) if changed == true => {
                record["public"] = true.into();
                record["username"] = wanted.clone().into();
                record["link"] = format!("https://t.me/{wanted}").into();
            }
            _ => {
                record["warning"]="The chat WAS created but the username was not assigned. Do not create another one. Use set_channel_username with this chat_id.".into();
            }
        }
    }
    Ok(record)
}
