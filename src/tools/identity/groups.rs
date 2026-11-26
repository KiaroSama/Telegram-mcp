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

async fn photo_history(api: &dyn Telegram, p: &Value) -> Result<Vec<Value>> {
    let mut old = Vec::new();
    let mut offset = 0;
    for _ in 0..100 {
        let r=api.invoke("messages.search",json!({"peer":p,"q":"","filter":{"_":"inputMessagesFilterChatPhotos"},"min_date":0,"max_date":0,"offset_id":offset,"add_offset":0,"limit":100,"max_id":0,"min_id":0,"hash":0})).await?;
        let messages = r
            .get("messages")
            .and_then(Value::as_array)
            .context("Telegram returned no chat photo history; no photo was changed")?;
        old.extend(
            messages
                .iter()
                .filter(|message| kind(&message["action"]) == "messageActionChatEditPhoto")
                .map(|message| req(message, "id").cloned())
                .collect::<Result<Vec<_>>>()?,
        );
        if messages.len() < 100 {
            return Ok(old);
        }
        let next = int_value(
            req(messages.last().context("Missing history cursor")?, "id")?,
            "photo history cursor",
        )?;
        ensure!(
            next > 0 && (offset == 0 || next < offset),
            "Chat photo history stopped advancing; no photo was changed"
        );
        offset = next;
    }
    anyhow::bail!("Chat photo history exceeds the bounded scan; no photo was changed or deleted")
}

async fn delete_photo_messages(
    api: &dyn Telegram,
    p: &Value,
    old: &[Value],
) -> Result<(usize, Option<bool>)> {
    let mut removed = 0;
    for batch in old.chunks(100) {
        let (method, request) = if is_channel(p) {
            (
                "channels.deleteMessages",
                json!({"channel":channel(p)?,"id":batch}),
            )
        } else {
            ("messages.deleteMessages", json!({"id":batch,"revoke":true}))
        };
        match api.invoke(method, request).await {
            Ok(_) => removed += batch.len(),
            Err(error) => {
                return Ok((
                    removed,
                    if known_rpc_refusal(&error) {
                        Some(false)
                    } else {
                        None
                    },
                ));
            }
        }
    }
    Ok((removed, Some(true)))
}

async fn chat_photo(api: &dyn Telegram, name: &str, a: &Value) -> Result<Value> {
    let p = peer(api, a, "chat_id").await?;
    ensure!(
        is_channel(&p) || kind(&p) == "inputPeerChat",
        "Chat photos require a group or channel"
    );
    let mut old = if name == "replace_chat_photo" {
        photo_history(api, &p).await?
    } else {
        Vec::new()
    };
    if name == "replace_chat_photo" && a.get("file_path").is_none_or(Value::is_null) {
        if !old.is_empty() {
            old.remove(0);
        }
        let (removed, complete) = delete_photo_messages(api, &p, &old).await?;
        return Ok(
            json!({"current_photo_retained":true,"old_photos_deleted":removed,"history_removed":complete,"warning":if complete==Some(true){None}else{Some("Photo history cleanup is incomplete or unverified; inspect the remaining photo history")}}),
        );
    }
    let photo = if name == "delete_chat_photo" {
        json!({"_":"inputChatPhotoEmpty"})
    } else {
        let upload = api.upload(Path::new(text(a, "file_path")?)).await?;
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
    if name != "replace_chat_photo" {
        return Ok(changed);
    }
    let (removed, complete) = delete_photo_messages(api, &p, &old).await?;
    Ok(
        json!({"photo_changed":true,"history_removed":complete,"old_photos_deleted":removed,"result":changed,"warning":if complete==Some(true){None}else{Some("The new photo is set, but prior history cleanup is incomplete or unverified; do not upload again")}}),
    )
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
            ensure!(
                !matches!(
                    kind(&p),
                    "inputPeerUser" | "inputPeerUserFromMessage" | "inputPeerSelf"
                ),
                "Private chats have no membership to leave. Use delete_chat to remove the conversation from your chat list."
            );
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
                let result = api
                    .invoke(
                        "channels.inviteToChannel",
                        json!({"channel":channel(&p)?,"users":us}),
                    )
                    .await?;
                let missing = result["missing_invitees"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|m| json!({"user_id":m["user_id"],"reason":invite_reason(m)}))
                    .collect::<Vec<_>>();
                let group_id = int_value(&entity_id(&p)?, "group_id")?;
                let mut added = observed_additions(&result, "channel_id", group_id);
                added.retain(|id| {
                    us.iter()
                        .any(|u| int_value(&u["user_id"], "user_id").ok() == Some(*id))
                        && !missing
                            .iter()
                            .any(|m| int_value(&m["user_id"], "user_id").ok() == Some(*id))
                });
                let added = added.into_iter().map(|i| i.to_string()).collect::<Vec<_>>();
                Ok(
                    json!({"requested":us.len(),"added":added.len(),"confirmed_added":added,"missing_invitees":missing}),
                )
            } else {
                ensure!(kind(&p) == "inputPeerChat", "Target must be a group");
                let mut results = Vec::new();
                for u in us {
                    match api.invoke("messages.addChatUser",json!({"chat_id":entity_id(&p)?,"user_id":u,"fwd_limit":100})).await {
                        Ok(r) => {
                            let id = int_value(&entity_id(&u)?, "invited user id")?;
                            let missing = r.get("missing_invitees").and_then(Value::as_array)
                                .into_iter().flatten().find(|entry| int_value(&entry["user_id"], "missing invitee id").ok() == Some(id));
                            let proof=observed_additions(&r,"chat_id",int_value(&entity_id(&p)?,"group_id")?);let success=if missing.is_some(){Some(false)}else if proof.contains(&id){Some(true)}else{None};let mut result=json!({"user_id":id.to_string(),"success":success});
                            if let Some(missing) = missing {
                                result["reason"] = invite_reason(missing).into();
                                result["missing_invitee"] = json!({"user_id":missing["user_id"],"premium_would_allow_invite":missing["premium_would_allow_invite"]==true,"premium_required_for_pm":missing["premium_required_for_pm"]==true});
                            }
                            results.push(result);
                        }
                        Err(error) if known_rpc_refusal(&error) => results.push(json!({"user_id":u["user_id"],"success":false,"reason":"Telegram refused this invitation"})),
                        Err(_) => results.push(json!({"user_id":u["user_id"],"success":null,"reason":"Invitation outcome is unknown; check membership before retrying"})),
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
                let operation_guard = api.operation_guard();
                if let Some(owned) = api.clone_handle() {
                    // Keep the native session alive and finish both steps even
                    // when the caller disconnects or its outer timeout fires.
                    return tokio::spawn(async move {
                        let _operation_guard = operation_guard;
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
                record["link"] = crate::permalinks::public_link(&wanted, None)?.into();
            }
            _ => {
                record["warning"]="The chat WAS created but the username was not assigned. Do not create another one. Use set_channel_username with this chat_id.".into();
            }
        }
    }
    Ok(record)
}

fn invite_reason(m: &Value) -> &'static str {
    if m["premium_would_allow_invite"] == true {
        "This user's privacy settings allow invitations from Premium users only"
    } else if m["premium_required_for_pm"] == true {
        "Premium is required to send this user an invite link"
    } else {
        "This user's privacy settings prevent the invitation"
    }
}

fn observed_additions(response: &Value, key: &str, id: i64) -> std::collections::BTreeSet<i64> {
    response["updates"]["updates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|u| &u["message"])
        .filter(|m| {
            int_value(&m["peer_id"][key], key).ok() == Some(id)
                && m["action"]["_"] == "messageActionChatAddUser"
        })
        .flat_map(|m| m["action"]["users"].as_array().into_iter().flatten())
        .filter_map(|v| int_value(v, "user_id").ok())
        .collect()
}
